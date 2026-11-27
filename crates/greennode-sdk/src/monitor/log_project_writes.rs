use super::mutation::{GuardedBody, Mutation, before_write, maybe_sent, not_sent, screen_details};
use super::settlement::{NAME_PAGE_CAP, Names, Wait, name_failure, uncertain};
use super::wire::{check_path_id, input_error, requires_failure};
use super::{Client, CreateLogProjectInput, LogProject, ResponseField};
use crate::wire::product::{evidence::Evidence, options::PathIds, redirect::Retry};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};
use http::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::time::Duration;

const ORDER: Mutation = Mutation {
    success: &[StatusCode::OK, StatusCode::CREATED],
    private: false,
    ..Mutation::standard(
        "monitor.CreateLogProject",
        "billing-api/v2/log/quotas",
        &Method::POST,
    )
};
const DELETE: Mutation = Mutation {
    success: &[StatusCode::OK, StatusCode::NO_CONTENT],
    retry: Retry::Repeatable,
    private: false,
    ..Mutation::standard(
        "monitor.DeleteLogProject",
        "billing-api/v1/log/quotas/{}",
        &Method::DELETE,
    )
};
const PURGE: Mutation = Mutation {
    route: "billing-api/v1/trash/log/quotas/{}",
    ..DELETE
};
const ORDER_BODY: Shape<'static> = Shape::Object(&[ShapeField {
    name: "orderId",
    shape: Shape::Any,
}]);
const POLL: Duration = Duration::from_secs(2);

/// Result of ordering a log project.
#[derive(Clone, Debug, Serialize)]
pub struct CreateLogProjectOutput {
    /// Project confirmed by settlement, or an empty project otherwise.
    #[serde(rename = "LogProject")]
    pub log_project: LogProject,
    /// Order identifier when supplied by the billing API.
    #[serde(rename = "OrderID")]
    pub order_id: String,
}

/// Selects a log project to trash or permanently purge.
#[derive(Clone, Debug)]
pub struct DeleteLogProjectInput {
    /// Log project identifier.
    pub log_project_id: String,
    /// Also purges the project from trash.
    pub purge: bool,
    /// Skips the baseline lookup and post-delete settlement.
    pub no_wait: bool,
}
impl DeleteLogProjectInput {
    /// Selects a project for soft deletion.
    pub fn new(log_project_id: impl Into<String>) -> Self {
        Self {
            log_project_id: log_project_id.into(),
            purge: false,
            no_wait: false,
        }
    }
}
/// Acknowledgement that the selected project was deleted or purged.
#[derive(Clone, Debug, Serialize)]
pub struct DeleteLogProjectOutput {}

impl Client {
    /// Prices a new log project and orders it once when the quote fits the price limit.
    pub async fn create_log_project(
        &self,
        ctx: &RequestContext,
        input: CreateLogProjectInput,
    ) -> Result<CreateLogProjectOutput, Failure<CreateLogProjectOutput>> {
        if input.name.is_empty() {
            return Err(before_write(requires_failure(ORDER.id, "Name")));
        }
        if !input.max_price.is_finite() || input.max_price < 0.0 {
            return Err(not_sent(input_error(format!(
                "vngcloud: invalid input: {}: MaxPrice must be a non-negative, finite number",
                ORDER.id
            ))));
        }
        let mut evidence = self
            .write_evidence(ctx, &[&input.name, &input.description, &input.class])
            .map_err(not_sent)?;
        if self
            .find_log_project(ctx, &ORDER, &input.name, &mut evidence)
            .await
            .map_err(not_sent)?
            .is_some()
        {
            return Err(not_sent(input_error(format!(
                "vngcloud: invalid input: {}: a log project with that name already exists",
                ORDER.id
            ))));
        }
        let terms = self
            .prepare_log_order(ctx, ORDER.id, &input, &mut evidence)
            .await
            .map_err(not_sent)?;
        let quote = self
            .price_log_order(ctx, ORDER.id, &terms, &mut evidence)
            .await
            .map_err(not_sent)?;
        let ResponseField::Value(quoted) = quote.optimum_price else {
            return Err(not_sent(ORDER.invalid(200, &evidence)));
        };
        let formatted_quote = format!("{quoted:.0}");
        let formatted_cap = format!("{:.0}", input.max_price);
        if evidence.text_reflects(&formatted_quote) || evidence.text_reflects(&formatted_cap) {
            return Err(not_sent(ORDER.invalid(200, &evidence)));
        }
        if quoted > input.max_price {
            return Err(not_sent(Error::new(ErrorClass::PriceAboveMax).with_outer_detail(format!("vngcloud: quoted price is above MaxPrice: {}: quote {formatted_quote} VND exceeds MaxPrice {formatted_cap} VND",ORDER.id))));
        }
        let body = GuardedBody {
            value: &terms.body,
            ids: &[&terms.class_id, &terms.package_id],
        };
        let reply = self
            .mutate(ctx, &ORDER, &[], Some(&body), &mut evidence)
            .await
            .map_err(order_advice)?;
        #[derive(Deserialize)]
        struct Acknowledgement {
            #[serde(rename = "orderId")]
            order_id: Option<Box<RawValue>>,
        }
        let ack: Acknowledgement = reply
            .decode(&ORDER, &ORDER_BODY, &evidence)
            .map_err(|error| order_advice(maybe_sent(error)))?;
        let order_id = ack
            .order_id
            .as_deref()
            .and_then(order_id)
            .unwrap_or_default();
        if !order_id.is_empty()
            && (!PathIds::WithUnderscore.allows(&order_id) || evidence.id_reflects(&order_id))
        {
            return Err(maybe_sent(ORDER.invalid(reply.status, &evidence)));
        }
        let mut output = CreateLogProjectOutput {
            log_project: LogProject::default(),
            order_id,
        };
        if input.no_wait {
            return Ok(output);
        }
        let result = self
            .wait_project_active(ctx, &input.name, &mut output.log_project, &mut evidence)
            .await;
        if evidence.id_reflects(&output.order_id) {
            return Err(maybe_sent(ORDER.invalid(reply.status, &evidence)));
        }
        if let Err(cause) = result {
            let safe = evidence.can_retain_partial()
                && super::log_projects::safe_project_guard(&output.log_project, &evidence);
            return Err(Failure::new(
                screen_details(
                    uncertain(&ORDER, "log project order was accepted", cause),
                    &evidence,
                ),
                safe.then_some(output),
                WriteDispatch::MayHaveBeenSent,
            ));
        }
        Ok(output)
    }
    /// Trashes the selected log project and optionally purges it from trash.
    pub async fn delete_log_project(
        &self,
        ctx: &RequestContext,
        input: DeleteLogProjectInput,
    ) -> Result<DeleteLogProjectOutput, Failure<DeleteLogProjectOutput>> {
        check_path_id(DELETE.id, "LogProjectID", &input.log_project_id).map_err(before_write)?;
        let id = &input.log_project_id;
        let mut evidence = self.write_evidence(ctx, &[id]).map_err(not_sent)?;
        let before = if input.no_wait {
            None
        } else {
            match self
                .read_guarded_log_project(ctx, &DELETE, id, &mut evidence)
                .await
            {
                Ok(project) => Some(project),
                Err(error) if input.purge && error.has_class(ErrorClass::NotFound) => None,
                Err(error) => return Err(not_sent(error)),
            }
        };
        let delete = self
            .mutate::<DeleteLogProjectOutput>(ctx, &DELETE, &[id], None, &mut evidence)
            .await;
        let mut missing = false;
        if let Err(failure) = delete {
            if !input.purge || !failure.error().has_class(ErrorClass::NotFound) {
                return Err(failure);
            }
            missing = true;
        }
        if input.purge
            && let Err(failure) = self
                .mutate::<DeleteLogProjectOutput>(ctx, &PURGE, &[id], None, &mut evidence)
                .await
            && (missing || !failure.error().has_class(ErrorClass::NotFound))
        {
            return Err(maybe_sent(failure.into_parts().0));
        }
        if let Some(before) = before {
            let result = self
                .wait_project_trashed(ctx, id, &before, &mut evidence)
                .await;
            if let Err(error) = result {
                return Err(Failure::new(
                    screen_details(
                        uncertain(&DELETE, "log project deletion was accepted", error),
                        &evidence,
                    ),
                    Some(DeleteLogProjectOutput {}),
                    WriteDispatch::MayHaveBeenSent,
                ));
            }
        }
        Ok(DeleteLogProjectOutput {})
    }

    async fn find_log_project(
        &self,
        ctx: &RequestContext,
        mutation: &Mutation,
        name: &str,
        evidence: &mut Evidence,
    ) -> Result<Option<LogProject>, Error> {
        let mut names = Names::new(0);
        for page in 0..NAME_PAGE_CAP {
            let values = self
                .log_project_page_guard(ctx, mutation, name, page, evidence)
                .await?;
            if let Some(end) = ctx.check(self.session.clock()) {
                return Err(end.into());
            }
            if names.page(
                page,
                values,
                name,
                |project| (&project.id, &project.project_name),
                mutation,
                evidence,
            )? {
                let found = names.result();
                if found.as_ref().is_some_and(|project| {
                    !super::log_projects::safe_project_guard(project, evidence)
                }) {
                    return Err(mutation.invalid(200, evidence));
                }
                return Ok(found);
            }
        }
        Err(name_failure(mutation, evidence))
    }
    async fn wait_project_active(
        &self,
        parent: &RequestContext,
        name: &str,
        output: &mut LogProject,
        evidence: &mut Evidence,
    ) -> Result<(), Error> {
        let mut wait = Wait::new(parent, self.session.clock(), POLL, Duration::from_secs(120));
        loop {
            let ctx = wait.next().await?;
            let found = self.find_log_project(ctx, &ORDER, name, evidence).await?;
            wait.check()?;
            if let Some(project) = found {
                *output = project;
                if output.status == "ACTIVE" {
                    return Ok(());
                }
            }
        }
    }
    async fn wait_project_trashed(
        &self,
        parent: &RequestContext,
        id: &str,
        before: &LogProject,
        evidence: &mut Evidence,
    ) -> Result<(), Error> {
        let mut wait = Wait::new(parent, self.session.clock(), POLL, Duration::from_secs(60));
        loop {
            let ctx = wait.next().await?;
            let found = self
                .read_guarded_log_project(ctx, &DELETE, id, evidence)
                .await;
            wait.check()?;
            match found {
                Ok(found)
                    if found.status != before.status
                        || found.billing_status != before.billing_status =>
                {
                    return Ok(());
                }
                Ok(_) => (),
                Err(error) if error.has_class(ErrorClass::NotFound) => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }
}

fn order_id(raw: &RawValue) -> Option<String> {
    let value = raw.get();
    match value.as_bytes().first() {
        Some(b'"') => serde_json::from_str(value).ok(),
        Some(b'-' | b'0'..=b'9') => Some(value.into()),
        _ => None,
    }
}
fn order_advice<T>(failure: Failure<T>) -> Failure<T> {
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
    Failure::new(error.with_outer_detail("monitor.CreateLogProject: order may have already reached the server; list log projects by name before ordering again".to_owned()),partial,dispatch)
}
