mod model;

pub use model::*;

use http::{Method, StatusCode};
use serde::Deserialize;
use serde_json::value::RawValue;
use std::time::Duration;

use super::inventory::{PAGE, Page, plain};
use super::log_alarm_body::{
    body, number, usage, validate_channels, validate_filter, validate_request, validate_threshold,
};
use super::mutation::{Mutation, Reply, before_write, maybe_sent, not_sent, screen_details};
use super::settlement::{NAME_PAGE_CAP, Names, Wait, name_failure, uncertain};
use super::wire::{check_path_id, decode, requires_failure};
use super::{Alarm, Client, LogProject};
use crate::model::wire::WireField;
use crate::wire::product::{evidence::Evidence, options::PathIds, redirect::Retry};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, PagedList, RequestContext, WriteDispatch};

macro_rules! object {
    ($($name:literal $(=> $shape:expr)?),+ $(,)?)=>{Shape::Object(&[$(ShapeField {name:$name,shape:object!(@shape $($shape)?) }),+])};
    (@shape $shape:expr)=>{$shape};(@shape)=>{Shape::Any};
}

const LOGS: &str = "vmonitor-api/api/v1/alarms/logs";
const LOG: &str = "vmonitor-api/api/v1/alarms/logs/{}";
const CREATE: Mutation = Mutation {
    success: &[StatusCode::OK, StatusCode::CREATED],
    retry: Retry::NonIdempotent,
    private: false,
    ..Mutation::standard("monitor.CreateLogAlarm", LOGS, &Method::POST)
};
const UPDATE: Mutation = Mutation {
    retry: Retry::Repeatable,
    private: false,
    ..Mutation::standard("monitor.UpdateLogAlarm", LOG, &Method::PUT)
};
const DELETE: Mutation = Mutation {
    success: &[StatusCode::OK, StatusCode::NO_CONTENT],
    retry: Retry::Repeatable,
    private: false,
    ..Mutation::standard("monitor.DeleteLogAlarm", LOG, &Method::DELETE)
};
const TWO_SECONDS: Duration = Duration::from_secs(2);

impl Client {
    /// Creates a log alarm and waits for settlement unless requested to skip it.
    pub async fn create_log_alarm(
        &self,
        ctx: &RequestContext,
        input: CreateLogAlarmInput,
    ) -> Result<CreateLogAlarmOutput, Failure<CreateLogAlarmOutput>> {
        if input.name.is_empty() {
            return Err(before_write(requires_failure(CREATE.id, "Name")));
        }
        check_path_id(CREATE.id, "LogProjectID", &input.log_project_id).map_err(before_write)?;
        validate_threshold(CREATE.id, input.threshold_value)?;
        validate_channels(CREATE.id, "InAlarm", &input.in_alarm)?;
        validate_channels(CREATE.id, "OK", &input.ok)?;
        validate_filter(CREATE.id, &input.query_string, input.filter.0.as_deref())
            .map_err(not_sent)?;
        let mut evidence = self
            .write_evidence(
                ctx,
                &[
                    &input.name,
                    &input.log_project_id,
                    &input.description,
                    &input.query_string,
                ],
            )
            .map_err(not_sent)?;
        let (alarm, mut detail, no_wait) = input.resolved().map_err(not_sent)?;
        let preliminary = body(&alarm, &detail, false, true)
            .ok_or_else(|| not_sent(CREATE.invalid(200, &evidence)))?;
        validate_request(&CREATE, &alarm, &detail, &preliminary, &evidence).map_err(not_sent)?;
        if self
            .find_log_alarm(ctx, &CREATE, &alarm.name, &mut evidence)
            .await
            .map_err(not_sent)?
            .is_some()
        {
            return Err(not_sent(usage(
                CREATE.id,
                "a log alarm with that name already exists",
            )));
        }
        let project = self
            .read_guarded_log_project(ctx, &CREATE, &detail.log_project_id, &mut evidence)
            .await
            .map_err(not_sent)?;
        detail.log_project_name = project.project_name;
        let wire = body(&alarm, &detail, false, true)
            .ok_or_else(|| not_sent(CREATE.invalid(200, &evidence)))?;
        validate_request(&CREATE, &alarm, &detail, &wire, &evidence).map_err(not_sent)?;
        let reply=self.mutate(ctx,&CREATE,&[],Some(&wire),&mut evidence).await.map_err(|failure|with_advice(failure,"monitor.CreateLogAlarm: create may have already reached the server; list log alarms by name before creating again"))?;
        let alarm_id = ack_id(&reply, &CREATE, &evidence).map_err(maybe_sent)?;
        if no_wait {
            return Ok(CreateLogAlarmOutput {
                alarm_id,
                alarm: Alarm::default(),
            });
        }
        let mut last = Alarm::default();
        let identity = if alarm_id.is_empty() {
            AlarmIdentity::Name(&alarm.name)
        } else {
            AlarmIdentity::Id(&alarm_id)
        };
        let result = self
            .wait_log_alarm(ctx, &CREATE, identity, &mut evidence, &mut last)
            .await;
        let output = CreateLogAlarmOutput {
            alarm_id: if last.id.is_empty() {
                alarm_id
            } else {
                last.id.clone()
            },
            alarm: last,
        };
        finish(result, &CREATE, output, &evidence, |output| {
            (&output.alarm, &output.alarm_id)
        })
    }

    /// Replaces supplied log alarm fields and keeps the rest of the guarded configuration.
    pub async fn update_log_alarm(
        &self,
        ctx: &RequestContext,
        input: UpdateLogAlarmInput,
    ) -> Result<UpdateLogAlarmOutput, Failure<UpdateLogAlarmOutput>> {
        check_path_id(UPDATE.id, "AlarmID", &input.alarm_id).map_err(before_write)?;
        if let Some(id) = &input.log_project_id {
            check_path_id(UPDATE.id, "LogProjectID", id).map_err(before_write)?;
        }
        if let Some(value) = input.threshold_value {
            validate_threshold(UPDATE.id, value)?;
        }
        if let Some(values) = &input.in_alarm {
            validate_channels(UPDATE.id, "InAlarm", values)?;
        }
        if let Some(values) = &input.ok {
            validate_channels(UPDATE.id, "OK", values)?;
        }
        if input.query_string.is_some() != input.filter.is_some() {
            return Err(not_sent(usage(
                UPDATE.id,
                "QueryString and Filter must both be set or both left nil",
            )));
        }
        if let (Some(query), Some(filter)) = (&input.query_string, &input.filter) {
            validate_filter(UPDATE.id, query, filter.0.as_deref()).map_err(not_sent)?;
        }
        let mut evidence = self
            .write_evidence(
                ctx,
                &[
                    &input.alarm_id,
                    input.name.as_deref().unwrap_or_default(),
                    input.description.as_deref().unwrap_or_default(),
                    input.query_string.as_deref().unwrap_or_default(),
                    input.log_project_id.as_deref().unwrap_or_default(),
                ],
            )
            .map_err(not_sent)?;
        let id = input.alarm_id.clone();
        let mut alarm = self
            .log_alarm_guard(ctx, &UPDATE, &id, &mut evidence)
            .await
            .map_err(not_sent)?;
        if alarm.status != "ACTIVE" {
            return Err(not_sent(usage(
                UPDATE.id,
                "alarm is still settling; retry once it reads ACTIVE",
            )));
        }
        let mut detail = alarm.log.take().ok_or_else(|| {
            not_sent(usage(
                UPDATE.id,
                "alarm read carries no log alarm detail to update from",
            ))
        })?;
        if [
            &alarm.name,
            &alarm.severity,
            &detail.id,
            &detail.log_project_id,
            &detail.log_project_name,
            &detail.threshold_type,
            &detail.condition,
        ]
        .into_iter()
        .any(|value| value.is_empty())
            || number(detail.time_frame) == Some(0)
        {
            return Err(not_sent(usage(
                UPDATE.id,
                "alarm read is missing a field the create body always sends; refusing a full-replace update",
            )));
        }
        let project = if let Some(id) = &input.log_project_id {
            Some(
                self.read_guarded_log_project(ctx, &UPDATE, id, &mut evidence)
                    .await
                    .map_err(not_sent)?,
            )
        } else {
            None
        };
        let no_wait = input.no_wait;
        let send_resend = input.resend.is_some() || number(detail.resend.enabled) == Some(true);
        input
            .apply(&mut alarm, &mut detail, project)
            .map_err(not_sent)?;
        let wire = body(&alarm, &detail, true, send_resend)
            .ok_or_else(|| not_sent(UPDATE.invalid(200, &evidence)))?;
        validate_request(&UPDATE, &alarm, &detail, &wire, &evidence).map_err(not_sent)?;
        self.mutate(ctx, &UPDATE, &[&id], Some(&wire), &mut evidence)
            .await?;
        let mut last = Alarm::default();
        if no_wait {
            return Ok(UpdateLogAlarmOutput { alarm: last });
        }
        let result = self
            .wait_log_alarm(
                ctx,
                &UPDATE,
                AlarmIdentity::Id(&id),
                &mut evidence,
                &mut last,
            )
            .await;
        finish(
            result,
            &UPDATE,
            UpdateLogAlarmOutput { alarm: last },
            &evidence,
            |output| (&output.alarm, ""),
        )
    }

    /// Deletes an alarm after verifying that it is a log alarm.
    pub async fn delete_log_alarm(
        &self,
        ctx: &RequestContext,
        input: DeleteLogAlarmInput,
    ) -> Result<DeleteLogAlarmOutput, Failure<DeleteLogAlarmOutput>> {
        check_path_id(DELETE.id, "AlarmID", &input.alarm_id).map_err(before_write)?;
        let id = &input.alarm_id;
        let mut evidence = self.write_evidence(ctx, &[id]).map_err(not_sent)?;
        self.log_alarm_guard(ctx, &DELETE, id, &mut evidence)
            .await
            .map_err(not_sent)?;
        if let Err(failure) = self
            .mutate::<DeleteLogAlarmOutput>(ctx, &DELETE, &[id], None, &mut evidence)
            .await
        {
            let potentially_absent = failure
                .error()
                .api_error()
                .and_then(crate::ApiError::status)
                .is_some_and(|status| status == 400 || status >= 500);
            if potentially_absent
                && self
                    .wait_alarm_absent(ctx, id, &mut evidence)
                    .await
                    .unwrap_or(false)
            {
                let message = format!("vngcloud: not found: {}: alarm {id}", DELETE.id);
                let error = screen_details(
                    Error::new(ErrorClass::NotFound).with_outer_detail(message),
                    &evidence,
                );
                return Err(Failure::new(error, None, failure.write_dispatch()));
            }
            let (error, _, dispatch) = failure.into_parts();
            return Err(Failure::new(
                screen_details(error, &evidence),
                None,
                dispatch,
            ));
        }
        Ok(DeleteLogAlarmOutput {})
    }

    pub(super) async fn read_guarded_log_project(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<LogProject, Error> {
        const PROJECT: Shape<'static> = object!(
            "id",
            "name",
            "description",
            "status",
            "billingStatus",
            "projectType",
            "createdAt"
        );
        let read = plain(mutation.id, "log-api/v1/projects/{}");
        let raw = self.read(ctx, &read, &[id], &[], evidence).await?;
        let project: LogProject = decode(&raw, &PROJECT, &read, evidence)?;
        if project.id != id
            || evidence.id_reflects(&project.id)
            || [
                &project.project_name,
                &project.project_description,
                &project.status,
                &project.billing_status,
                &project.project_type,
                &project.created_at,
            ]
            .into_iter()
            .any(|text| evidence.text_reflects(text))
        {
            return Err(mutation.invalid(200, evidence));
        }
        Ok(project)
    }

    async fn log_alarm_guard(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<Alarm, Error> {
        let read = plain(mutation.id, "vmonitor-api/api/v1/alarms/{}");
        let raw = self.read(ctx, &read, &[id], &[], evidence).await?;
        #[derive(Deserialize)]
        struct Envelope {
            data: Alarm,
        }
        let found: Envelope = decode(&raw, &ALARM_ENVELOPE, &read, evidence)?;
        if found.data.id != id {
            return Err(mutation.invalid(200, evidence));
        }
        check_alarm_output(&found.data, mutation, evidence)?;
        if found.data.kind != "Log" {
            return Err(usage(mutation.id, "alarm is not a log alarm"));
        }
        Ok(found.data)
    }

    async fn find_log_alarm(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        name: &str,
        evidence: &mut Evidence,
    ) -> Result<Option<Alarm>, Error> {
        let mut names = Names::new(1);
        for page in 1..=NAME_PAGE_CAP {
            let values = self
                .log_alarm_page(ctx, mutation, name, page, evidence)
                .await?;
            if let Some(end) = ctx.check(self.session.clock()) {
                return Err(end.into());
            }
            if names.page(
                page,
                values,
                name,
                |alarm| (&alarm.id, &alarm.name),
                mutation,
                evidence,
            )? {
                let found = names.result();
                if let Some(alarm) = &found {
                    check_alarm_output(alarm, mutation, evidence)?;
                }
                return Ok(found);
            }
        }
        Err(name_failure(mutation, evidence))
    }

    async fn log_alarm_page(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        name: &str,
        page: i64,
        evidence: &mut Evidence,
    ) -> Result<PagedList<Alarm>, Error> {
        let read = plain(mutation.id, "vmonitor-api/api/v1/alarms/list");
        let page = page.to_string();
        let query = [
            ("type-alarm", "Log"),
            ("name", name),
            ("status", ""),
            ("severity", ""),
            ("page", page.as_str()),
            ("size", "10000"),
        ];
        let raw = self.read(ctx, &read, &[], &query, evidence).await?;
        let values: Page<Alarm> = decode(&raw, &PAGE, &read, evidence)?;
        let mut values = values.into_page(true, &read, evidence)?;
        for alarm in &mut values.items {
            alarm.kind = "Log".into();
            alarm.metric_mapping_id.clear();
            check_alarm_output(alarm, mutation, evidence)?;
        }
        Ok(values)
    }

    async fn wait_log_alarm(
        &self,
        parent: &RequestContext,
        mutation: &Mutation,
        identity: AlarmIdentity<'_>,
        evidence: &mut Evidence,
        last: &mut Alarm,
    ) -> Result<(), Error> {
        let mut wait = Wait::new(
            parent,
            self.session.clock(),
            TWO_SECONDS,
            Duration::from_secs(120),
        );
        loop {
            let ctx = wait.next().await?;
            let value = match identity {
                AlarmIdentity::Id(id) => {
                    Some(self.log_alarm_guard(ctx, mutation, id, evidence).await?)
                }
                AlarmIdentity::Name(name) => {
                    self.find_log_alarm(ctx, mutation, name, evidence).await?
                }
            };
            wait.check()?;
            if let Some(alarm) = value {
                *last = alarm;
                match last.status.as_str() {
                    "ACTIVE" => return Ok(()),
                    "ERROR" | "FAILED" => {
                        return Err(Error::new(ErrorClass::Failed).with_outer_detail(format!(
                            "{}: log alarm reached status {}",
                            mutation.id, last.status
                        )));
                    }
                    _ => (),
                }
            }
        }
    }

    async fn wait_alarm_absent(
        &self,
        parent: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<bool, Error> {
        let mut wait = Wait::new(
            parent,
            self.session.clock(),
            TWO_SECONDS,
            Duration::from_secs(30),
        );
        loop {
            let ctx = wait.next().await?;
            let mut seen = 0_i64;
            for page in 1..=50 {
                let values = self
                    .log_alarm_page(ctx, &DELETE, "", page, evidence)
                    .await?;
                if values.items.iter().any(|alarm| alarm.id == id) {
                    break;
                }
                seen = seen.saturating_add(i64::try_from(values.items.len()).unwrap_or(i64::MAX));
                if values.items.is_empty() || seen >= values.total_item {
                    wait.check()?;
                    return Ok(true);
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum AlarmIdentity<'a> {
    Id(&'a str),
    Name(&'a str),
}
fn check_alarm_output(
    alarm: &Alarm,
    mutation: &Mutation,
    evidence: &Evidence,
) -> Result<(), Error> {
    let ids = [alarm.id.as_str(), alarm.metric_mapping_id.as_str()];
    let nested = alarm.log.as_ref().is_some_and(|detail| {
        [&detail.id, &detail.log_project_id]
            .into_iter()
            .chain(&detail.in_alarm)
            .chain(&detail.ok)
            .any(|id| evidence.id_reflects(id))
            || evidence.text_reflects(&detail.raw_log_search_query)
    });
    let mut copy = alarm.clone();
    let reflected = serde_json::to_value(alarm).map_or(true, |value| evidence.reflected_in(&value));
    if ids.iter().any(|id| evidence.id_reflects(id))
        || nested
        || reflected
        || !copy.redact(evidence)
    {
        return Err(mutation.invalid(200, evidence));
    }
    Ok(())
}
fn finish<T>(
    result: Result<(), Error>,
    mutation: &Mutation,
    output: T,
    evidence: &Evidence,
    retained: impl FnOnce(&T) -> (&Alarm, &str),
) -> Result<T, Failure<T>> {
    if let Err(cause) = result {
        let (alarm, id) = retained(&output);
        let safe = evidence.can_retain_partial()
            && !evidence.id_reflects(id)
            && check_alarm_output(alarm, mutation, evidence).is_ok();
        let error = uncertain(mutation, "log alarm was accepted", cause);
        return Err(Failure::new(
            screen_details(error, evidence),
            safe.then_some(output),
            WriteDispatch::MayHaveBeenSent,
        ));
    }
    Ok(output)
}

fn with_advice<T>(failure: Failure<T>, message: &str) -> Failure<T> {
    if failure.write_dispatch() != WriteDispatch::MayHaveBeenSent
        || failure
            .error()
            .api_error()
            .and_then(crate::ApiError::status)
            .is_some_and(|status| (400..500).contains(&status))
    {
        return failure;
    }
    let (error, partial, dispatch) = failure.into_parts();
    Failure::new(
        error.with_outer_detail(message.to_owned()),
        partial,
        dispatch,
    )
}

fn ack_id(reply: &Reply, mutation: &Mutation, evidence: &Evidence) -> Result<String, Error> {
    #[derive(Default, Deserialize)]
    struct Ack {
        id: Option<Box<RawValue>>,
        data: Option<Box<RawValue>>,
    }
    let raw = match reply.decode::<Box<RawValue>>(mutation, &Shape::Any, evidence) {
        Ok(value) => value,
        Err(error)
            if error
                .api_error()
                .is_some_and(|api| api.code() == "InvalidResponse") =>
        {
            return Err(error);
        }
        Err(_) => return Ok(String::new()),
    };
    fn read(
        raw: &RawValue,
        mutation: &Mutation,
        reply: &Reply,
        evidence: &Evidence,
    ) -> Result<Ack, Error> {
        let text = raw.get();
        if !text.starts_with('{') {
            return Ok(Ack::default());
        }
        crate::wire::strict_json::validate_canonical_shape(text.as_bytes(), &ACK, text.len())
            .map_err(|_| mutation.invalid(reply.status, evidence))?;
        serde_json::from_str(text).map_err(|_| mutation.invalid(reply.status, evidence))
    }
    let ack = read(&raw, mutation, reply, evidence)?;
    let outer = ack.id.as_deref().and_then(scalar_id);
    let inner = if let Some(data) = ack.data {
        read(&data, mutation, reply, evidence)?
            .id
            .as_deref()
            .and_then(scalar_id)
    } else {
        None
    };
    if outer
        .as_ref()
        .zip(inner.as_ref())
        .is_some_and(|(left, right)| left != right)
    {
        return Err(mutation.invalid(reply.status, evidence));
    }
    let id = outer.or(inner).unwrap_or_default();
    if !id.is_empty() && (!PathIds::Strict.allows(&id) || evidence.id_reflects(&id)) {
        return Err(mutation.invalid(reply.status, evidence));
    }
    Ok(id)
}
fn scalar_id(raw: &RawValue) -> Option<String> {
    let value = raw.get();
    match value.as_bytes().first() {
        Some(b'"') => serde_json::from_str::<String>(value)
            .ok()
            .filter(|value| !value.is_empty()),
        Some(b'-' | b'0'..=b'9') => Some(value.into()),
        _ => None,
    }
}

const ACK: Shape<'static> = object!("id", "data");
const ALARM_ENVELOPE: Shape<'static> = object!("data"=>object!("id","name","description","type","status","progressStatus","severity","metricMappingId","inAlarm","ok","alarmLog"));
