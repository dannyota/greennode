mod privacy;
mod toggle;

use std::{
    collections::{BTreeMap, HashMap},
    future::{Future, poll_fn},
    pin::pin,
    sync::{Arc, Mutex, Weak},
    task::Poll,
};

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{Value, json};

use super::inventory::{get_input, plain};
use super::mutation::{Mutation, SendGuard, before_write, maybe_sent, not_sent};
use super::wire::{check_path_id, decode, input_error, requires_failure};
use super::{
    Assertion, Check, CheckConfig, CheckNotifications, CheckOptions, CheckRequest, Client,
    ResponseField,
};
use crate::model::wire::Field;
use crate::wire::product::{evidence::Evidence, redirect::Retry};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RequestContext};
use privacy::Privacy;

pub use toggle::*;

const CHECK_ROUTE: &str = "vmonitor-uptime-manager/v1/uptimes/{}";
const CREATE: Mutation = Mutation {
    success: &[StatusCode::CREATED],
    retry: Retry::NonIdempotent,
    ..Mutation::standard(
        "monitor.CreateCheck",
        "vmonitor-uptime-manager/v1/uptimes",
        &Method::POST,
    )
};
const UPDATE: Mutation = Mutation {
    retry: Retry::Repeatable,
    ..Mutation::standard("monitor.UpdateCheck", CHECK_ROUTE, &Method::PUT)
};
const DELETE: Mutation = Mutation {
    success: &[StatusCode::NO_CONTENT],
    retry: Retry::Repeatable,
    private: false,
    ..Mutation::standard("monitor.DeleteCheck", CHECK_ROUTE, &Method::DELETE)
};

/// Input for configuring a new synthetic check.
#[derive(Clone)]
pub struct CreateCheckInput {
    /// Check name.
    pub name: String,
    /// Probe destination.
    pub url: String,
    /// Locations where the check runs.
    pub locations: Vec<String>,
    /// HTTP method. An empty value selects GET.
    pub method: String,
    /// Probe headers.
    pub headers: BTreeMap<String, String>,
    /// Probe query parameters.
    pub query: BTreeMap<String, String>,
    /// Probe request body.
    pub body: String,
    /// Timeout in seconds. Zero selects 10.
    pub timeout: i64,
    /// Test frequency in minutes. Zero selects one.
    pub test_frequency: i64,
    /// Tests per location. Zero selects one.
    pub tests: i64,
    /// Number of failing locations that cause an alarm. Zero selects all.
    pub failed_locations: i64,
    /// Response assertions. An empty list selects the default status-code rule.
    pub assertions: Vec<Assertion>,
    /// Notification destinations for each alarm transition.
    pub notifications: CheckNotifications,
}
impl CreateCheckInput {
    /// Supplies the required check name, URL and locations.
    pub fn new(name: impl Into<String>, url: impl Into<String>, locations: Vec<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            locations,
            method: String::new(),
            headers: BTreeMap::new(),
            query: BTreeMap::new(),
            body: String::new(),
            timeout: 0,
            test_frequency: 0,
            tests: 0,
            failed_locations: 0,
            assertions: Vec::new(),
            notifications: CheckNotifications::default(),
        }
    }
}

/// Replacement fields for an existing synthetic check.
#[derive(Clone)]
pub struct UpdateCheckInput {
    /// Check identifier.
    pub check_id: String,
    /// Replacement check name.
    pub name: Option<String>,
    /// Replacement probe URL.
    pub url: Option<String>,
    /// Replacement probe method.
    pub method: Option<String>,
    /// Replacement probe headers.
    pub headers: Option<BTreeMap<String, String>>,
    /// Replacement probe query parameters.
    pub query: Option<BTreeMap<String, String>>,
    /// Replacement probe body.
    pub body: Option<String>,
    /// Replacement timeout in seconds.
    pub timeout: Option<i64>,
    /// Replacement frequency in minutes.
    pub test_frequency: Option<i64>,
    /// Replacement tests per location.
    pub tests: Option<i64>,
    /// Replacement number of failed locations.
    pub failed_locations: Option<i64>,
    /// Replacement locations.
    pub locations: Option<Vec<String>>,
    /// Replacement response assertions.
    pub assertions: Option<Vec<Assertion>>,
    /// Replacement notification lists.
    pub notifications: Option<CheckNotifications>,
}
impl UpdateCheckInput {
    /// Selects a check to update while leaving every field unset.
    pub fn new(check_id: impl Into<String>) -> Self {
        Self {
            check_id: check_id.into(),
            name: None,
            url: None,
            method: None,
            headers: None,
            query: None,
            body: None,
            timeout: None,
            test_frequency: None,
            tests: None,
            failed_locations: None,
            locations: None,
            assertions: None,
            notifications: None,
        }
    }
}
get_input!(
    /// Selects a synthetic check to delete.
    DeleteCheckInput,
    /// Check identifier.
    check_id
);

macro_rules! result_check {
    ($(#[$meta:meta])* $name:ident) => { $(#[$meta])* #[derive(Clone,Debug,Serialize)] pub struct $name {
        /// Check returned by the provider.
        #[serde(rename = "Check")]
        pub check: Check,
    }};
}
result_check!(
    /// The created synthetic check.
    CreateCheckOutput
);
result_check!(
    /// The acknowledged updated check.
    UpdateCheckOutput
);

/// Acknowledgement that a check was deleted.
#[derive(Clone, Debug, Serialize)]
pub struct DeleteCheckOutput {}

#[derive(Default)]
pub(super) struct CheckGates(Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>);
impl Client {
    async fn check_lock(
        &self,
        ctx: &RequestContext,
        id: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, Error> {
        let clock = self.session.clock();
        if let Some(end) = ctx.check(clock) {
            return Err(end.into());
        }
        let gate = {
            let mut gates = self
                .check_gates
                .0
                .lock()
                .map_err(|_| Error::new(ErrorClass::Api))?;
            gates.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = gates.get(id).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(tokio::sync::Mutex::new(()));
                gates.insert(id.into(), Arc::downgrade(&gate));
                gate
            }
        };
        let mut stopped = pin!(ctx.wait(clock));
        let mut locked = pin!(gate.lock_owned());
        let guard = poll_fn(|cx| {
            if let Poll::Ready(end) = stopped.as_mut().poll(cx) {
                return Poll::Ready(Err(Error::from(end)));
            }
            locked.as_mut().poll(cx).map(Ok)
        })
        .await?;
        if let Some(end) = ctx.check(clock) {
            return Err(end.into());
        }
        Ok(guard)
    }

    /// Creates an HTTP check with the console defaults for unset fields.
    pub async fn create_check(
        &self,
        ctx: &RequestContext,
        input: CreateCheckInput,
    ) -> Result<CreateCheckOutput, Failure<CreateCheckOutput>> {
        if input.name.is_empty() {
            return Err(before_write(requires_failure(CREATE.id, "Name")));
        }
        if input.url.is_empty() {
            return Err(before_write(requires_failure(CREATE.id, "URL")));
        }
        if input.locations.is_empty() {
            return Err(not_sent(invalid(
                CREATE.id,
                "requires at least one Locations entry",
            )));
        }
        let check = input.into_check();
        let mut evidence = self.write_evidence(ctx, &[]).map_err(not_sent)?;
        let privacy = Privacy::from_request(&check.config.request);
        privacy
            .check_limit(CREATE.id, &evidence)
            .map_err(not_sent)?;
        let body = check_body(&check);
        accept_request(&CREATE, &check, &body, &evidence).map_err(not_sent)?;
        let reply = self
            .mutate_guarded(
                ctx,
                &CREATE,
                &[],
                Some(&body),
                &mut evidence,
                SendGuard {
                    privacy: Some(privacy.diagnostics()),
                    ..SendGuard::default()
                },
            )
            .await
            .map_err(|error| privacy.screen_failure(error))?;
        let Field(check): Field<Check> = reply
            .decode(&CREATE, &CHECK, &evidence)
            .map_err(maybe_sent)?;
        privacy
            .validate(&CREATE, reply.status, &check, &evidence)
            .map_err(maybe_sent)?;
        if check.id.is_empty() {
            return Err(maybe_sent(CREATE.error(
                Some(reply.status),
                "",
                "create response had no id",
                &evidence,
            )));
        }
        Ok(CreateCheckOutput { check })
    }

    /// Updates a snapshot of an existing API/HTTP check and preserves its run state.
    pub async fn update_check(
        &self,
        ctx: &RequestContext,
        input: UpdateCheckInput,
    ) -> Result<UpdateCheckOutput, Failure<UpdateCheckOutput>> {
        check_path_id(UPDATE.id, "CheckID", &input.check_id).map_err(before_write)?;
        if !input.changes_any() {
            return Err(not_sent(invalid(
                UPDATE.id,
                "requires at least one field to change",
            )));
        }
        if input.locations.as_ref().is_some_and(Vec::is_empty) {
            return Err(not_sent(invalid(
                UPDATE.id,
                "Locations, if set, requires at least one entry",
            )));
        }
        let mut evidence = self
            .write_evidence(ctx, &[&input.check_id])
            .map_err(not_sent)?;
        let mut privacy = Privacy::from_request(&input.request_fields());
        privacy
            .check_limit(UPDATE.id, &evidence)
            .map_err(not_sent)?;
        let id = input.check_id.clone();
        let _guard = self.check_lock(ctx, &id).await.map_err(not_sent)?;
        let mut current = self
            .read_check_guard(
                ctx,
                UPDATE.id,
                &id,
                &mut evidence,
                Some(privacy.diagnostics()),
            )
            .await
            .map_err(|error| not_sent(privacy.screen_error(error)))?;
        privacy.add_request(&current.config.request);
        privacy
            .validate(&UPDATE, 200, &current, &evidence)
            .map_err(not_sent)?;
        if current.id != id {
            return Err(not_sent(UPDATE.error(
                Some(200),
                "",
                "pre-update read returned no usable check; refusing to overwrite it",
                &evidence,
            )));
        }
        if current.r#type != "API" || current.subtype != "HTTP" {
            return Err(not_sent(invalid(
                UPDATE.id,
                "only API/HTTP checks are supported",
            )));
        }
        input.apply(&mut current);
        let body = check_body(&current);
        accept_request(&UPDATE, &current, &body, &evidence).map_err(not_sent)?;
        let reply = self
            .mutate_guarded(
                ctx,
                &UPDATE,
                &[&id],
                Some(&body),
                &mut evidence,
                SendGuard {
                    privacy: Some(privacy.diagnostics()),
                    ..SendGuard::default()
                },
            )
            .await
            .map_err(|error| privacy.screen_failure(error))?;
        let Field(mut check): Field<Check> = reply
            .decode(&UPDATE, &CHECK, &evidence)
            .map_err(maybe_sent)?;
        if check.id.is_empty() {
            check = self
                .read_check_guard(
                    ctx,
                    UPDATE.id,
                    &id,
                    &mut evidence,
                    Some(privacy.diagnostics()),
                )
                .await
                .map_err(|error| maybe_sent(privacy.screen_error(error)))?;
        }
        privacy
            .validate(&UPDATE, reply.status, &check, &evidence)
            .map_err(maybe_sent)?;
        if check.id != id {
            return Err(maybe_sent(UPDATE.error(
                Some(200),
                "InvalidResponse",
                "check confirmation did not match the requested identity",
                &evidence,
            )));
        }
        Ok(UpdateCheckOutput { check })
    }

    /// Deletes a check and its history.
    pub async fn delete_check(
        &self,
        ctx: &RequestContext,
        input: DeleteCheckInput,
    ) -> Result<DeleteCheckOutput, Failure<DeleteCheckOutput>> {
        check_path_id(DELETE.id, "CheckID", &input.check_id).map_err(before_write)?;
        let mut evidence = self
            .write_evidence(ctx, &[&input.check_id])
            .map_err(not_sent)?;
        self.mutate(ctx, &DELETE, &[&input.check_id], None, &mut evidence)
            .await?;
        Ok(DeleteCheckOutput {})
    }

    async fn read_check_guard(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        id: &str,
        evidence: &mut Evidence,
        privacy: Option<&Evidence>,
    ) -> Result<Check, Error> {
        let read = plain(op, CHECK_ROUTE);
        let raw = self
            .read_guarded(
                ctx,
                &read,
                &[id],
                &[],
                evidence,
                privacy.map(|evidence| (op, evidence)),
            )
            .await?;
        Ok(decode::<Field<Check>>(&raw, &CHECK, &read, evidence)?.0)
    }
}

impl CreateCheckInput {
    fn into_check(self) -> Check {
        let failed = if self.failed_locations == 0 {
            i64::try_from(self.locations.len()).unwrap_or(i64::MAX)
        } else {
            self.failed_locations
        };
        let assertions = if self.assertions.is_empty() {
            vec![Assertion {
                r#type: "status_code".into(),
                operator: "does_not_match_regex".into(),
                target: "[4-5][0-9][0-9]".into(),
            }]
        } else {
            self.assertions
        };
        let default =
            |value, default| ResponseField::Value(if value == 0 { default } else { value });
        Check {
            r#type: "API".into(),
            subtype: "HTTP".into(),
            name: self.name,
            locations: self.locations,
            notifications: self.notifications,
            config: CheckConfig {
                request: CheckRequest {
                    url: self.url,
                    method: if self.method.is_empty() {
                        "GET".into()
                    } else {
                        self.method
                    },
                    headers: self.headers,
                    query: self.query,
                    body: self.body,
                    timeout: default(self.timeout, 10),
                    verified_ssl: ResponseField::Value(true),
                },
                assertions,
            },
            options: CheckOptions {
                test_frequency: default(self.test_frequency, 1),
                tests: default(self.tests, 1),
                failed_locations: ResponseField::Value(failed),
            },
            ..Default::default()
        }
    }
}
impl UpdateCheckInput {
    fn changes_any(&self) -> bool {
        [
            self.name.is_some(),
            self.url.is_some(),
            self.method.is_some(),
            self.headers.is_some(),
            self.query.is_some(),
            self.body.is_some(),
            self.timeout.is_some(),
            self.test_frequency.is_some(),
            self.tests.is_some(),
            self.failed_locations.is_some(),
            self.locations.is_some(),
            self.assertions.is_some(),
            self.notifications.is_some(),
        ]
        .into_iter()
        .any(|value| value)
    }
    fn request_fields(&self) -> CheckRequest {
        CheckRequest {
            url: self.url.clone().unwrap_or_default(),
            headers: self.headers.clone().unwrap_or_default(),
            query: self.query.clone().unwrap_or_default(),
            body: self.body.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
    fn apply(self, check: &mut Check) {
        let req = &mut check.config.request;
        macro_rules! replace { ($($field:ident => $target:expr),+ $(,)?) => {$(if let Some(value)=self.$field {$target=value;})+}; }
        replace!(name=>check.name,url=>req.url,method=>req.method,headers=>req.headers,query=>req.query,body=>req.body,locations=>check.locations,assertions=>check.config.assertions,notifications=>check.notifications);
        if let Some(value) = self.timeout {
            req.timeout = ResponseField::Value(value);
        }
        if let Some(value) = self.test_frequency {
            check.options.test_frequency = ResponseField::Value(value);
        }
        if let Some(value) = self.tests {
            check.options.tests = ResponseField::Value(value);
        }
        if let Some(value) = self.failed_locations {
            check.options.failed_locations = ResponseField::Value(value);
        }
        req.verified_ssl = ResponseField::Value(true);
    }
}

fn check_body(check: &Check) -> Value {
    let request = &check.config.request;
    let assertions:Vec<_>=check.config.assertions.iter().map(|assertion|json!({"type":assertion.r#type,"operator":assertion.operator,"target":assertion.target})).collect();
    json!({
        "type":"API","subtype":"HTTP","name":check.name,"locations":check.locations,
        "config":{"request":{"url":request.url,"method":request.method,"headers":request.headers,"query":request.query,"body":request.body,"timeout":request.timeout,"verified_ssl":true},"assertions":assertions},
        "options":{"test_frequency":check.options.test_frequency,"tests":check.options.tests,"failed_locations":check.options.failed_locations},
        "notifications":{"In-alarm":check.notifications.in_alarm,"Up":check.notifications.up,"Undetermined":check.notifications.undetermined},
    })
}

fn accept_request(
    mutation: &Mutation,
    check: &Check,
    body: &Value,
    evidence: &Evidence,
) -> Result<(), Error> {
    if evidence.reflected_in(body) || check_ids(check).any(|id| evidence.id_reflects(id)) {
        return Err(invalid(mutation.id, "request operand withheld"));
    }
    Ok(())
}
fn invalid(op: &str, message: &str) -> Error {
    input_error(format!("vngcloud: invalid input: {op} {message}"))
}
fn check_ids(check: &Check) -> impl Iterator<Item = &str> {
    [&check.id, &check.history_alarm_id]
        .into_iter()
        .chain(&check.locations)
        .chain(&check.notifications.in_alarm)
        .chain(&check.notifications.up)
        .chain(&check.notifications.undetermined)
        .map(String::as_str)
}

macro_rules! object {
    ($($key:literal $(=> $shape:expr)?),+ $(,)?) => { Shape::Object(&[$(ShapeField { name:$key, shape:object!(@shape $($shape)?) }),+]) };
    (@shape $shape:expr) => { $shape };
    (@shape) => { Shape::Any };
}
const REQUEST: Shape<'static> = object!(
    "url",
    "method",
    "headers",
    "query",
    "body",
    "timeout",
    "verified_ssl"
);
const ASSERTION: Shape<'static> = object!("type", "operator", "target");
const CHECK: Shape<'static> = object!("id","name","type","subtype","status","locations","created_at","updated_at","monitor_status","history_alarm",
    "config"=>object!("request"=>REQUEST,"assertions"=>Shape::Array(&ASSERTION)),
    "options"=>object!("test_frequency","tests","failed_locations"),"notifications"=>object!("In-alarm","Up","Undetermined"));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, ConfigOptions, Secret};

    #[tokio::test]
    async fn check_gate_serializes_one_id_and_cancels_waiters() {
        let config = Config::new(
            ConfigOptions::default()
                .with_region("hcm-3".into())
                .with_static_token(Secret::new("toy-key")),
        )
        .unwrap();
        let client = Client::new(&config);
        let (first, _) = RequestContext::new();
        let _one = client.check_lock(&first, "check-1").await.unwrap();
        let _other = client.check_lock(&first, "check-2").await.unwrap();
        let (second, cancel) = RequestContext::new();
        let mut wait = pin!(client.check_lock(&second, "check-1"));
        poll_fn(|cx| {
            assert!(wait.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        cancel.cancel();
        let result = wait.await.unwrap_err();
        assert_eq!(result.class(), ErrorClass::Cancelled);
    }
}
