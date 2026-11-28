use std::time::Duration;

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::{json, value::RawValue};

use crate::model::wire::go_json;
use crate::wire::product::{
    evidence::{Evidence, SentCredentials},
    options::{AttemptCheck, Options},
};
use crate::{ApiError, Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::endpoint_quote::{self, check_input};
use super::endpoint_quote_scope::{EndpointQuote, Offer, hex24, uuid};
use super::mutations::{
    Destination, Mutation, api, check_context, check_quote, input_error, not_sent, safe_output,
    screen, tick, typed_error,
};
use super::nat_catalog::{numeric_code, reflects};
use super::wire::Fields;
use super::{Client, QuoteCreateEndpointInput, ResourceInUse};

const CREATE: Mutation = Mutation::new(
    "network.CreateEndpoint",
    "vnetwork/v1/{}/{}/endpoints",
    Method::POST,
    &[StatusCode::CREATED],
)
.protected(true, "endpoint response withheld");
const DELETE: Mutation = Mutation::new(
    "network.DeleteEndpoint",
    "vnetwork/v1/{}/{}/endpoints/{}",
    Method::DELETE,
    &[StatusCode::OK],
)
.protected(true, "endpoint response withheld");
const RECOVERY: &str = "Run network list-endpoints and network get-endpoint in the same scope, then inspect billing before another mutation. Do not repeat this write.";

/// Selects an endpoint offer and caps the fresh monthly price.
pub type CreateEndpointInput = QuoteCreateEndpointInput;

/// The result of an endpoint order.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreateEndpointOutput {
    /// Unambiguous endpoint identifier returned by the order.
    #[serde(rename = "EndpointID")]
    pub endpoint_id: String,
    /// Last state verified in endpoint inventory.
    #[serde(rename = "Status")]
    pub status: String,
    /// Monthly quote from immediately before the order.
    #[serde(rename = "MonthlyPrice")]
    pub monthly_price: f64,
    /// Quote currency.
    #[serde(rename = "Currency")]
    pub currency: String,
}

/// Selects the endpoint and its owning VPC for deletion.
#[derive(Clone, Debug)]
pub struct DeleteEndpointInput {
    /// Endpoint identifier in the form enp-UUID.
    pub endpoint_id: String,
    /// Owning VPC identifier in the form net-UUID.
    pub vpc_id: String,
}

impl DeleteEndpointInput {
    /// Requires consent for an endpoint in the named VPC.
    pub fn new(endpoint_id: impl Into<String>, vpc_id: impl Into<String>) -> Self {
        Self {
            endpoint_id: endpoint_id.into(),
            vpc_id: vpc_id.into(),
        }
    }
}

/// The result of an endpoint delete.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeleteEndpointOutput {
    /// Endpoint selected for deletion.
    #[serde(rename = "EndpointID")]
    pub endpoint_id: String,
    /// Whether complete inventory confirmed absence.
    #[serde(rename = "Deleted")]
    pub deleted: bool,
}

struct EndpointRow {
    id: String,
    name: String,
    vpc: String,
    subnet: String,
    service: String,
    service_name: String,
    package: String,
    zone: String,
    status: String,
    gateway: bool,
    dns: bool,
    attached: Option<bool>,
}

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Quotes and orders an endpoint, then confirms it in complete scoped inventory.
    pub async fn create_endpoint(
        &self,
        ctx: &RequestContext,
        input: CreateEndpointInput,
    ) -> Result<CreateEndpointOutput, Failure<CreateEndpointOutput>> {
        let mut evidence = self.request_evidence(ctx).map_err(not_sent)?;
        check_input(&input, &evidence)
            .map_err(|error| not_sent(scope_error(CREATE.id, error, &evidence)))?;
        let mut scope = match EndpointQuote::new(self, ctx, &mut evidence, true).await {
            Ok(scope) => scope,
            Err(error) => return Err(not_sent(scope_error(CREATE.id, error, &evidence))),
        };
        let offer = scope
            .offer(&input)
            .await
            .map_err(|error| not_sent(scope_error(CREATE.id, error, scope.evidence)))?;
        scope
            .check_placement(&input, &offer)
            .await
            .map_err(|error| not_sent(scope_error(CREATE.id, error, scope.evidence)))?;
        let before = endpoint_rows(&mut scope, CREATE.id, true)
            .await
            .map_err(not_sent)?;
        if before
            .iter()
            .any(|row| row.name == input.name && row.vpc == input.vpc_id)
        {
            return Err(not_sent(input_error(
                CREATE.id,
                ": an endpoint with this name exists in the VPC",
            )));
        }
        let quote = scope
            .endpoint_price(&input, &offer)
            .await
            .map_err(|error| not_sent(scope_error(CREATE.id, error, scope.evidence)))?;
        let price = quote.monthly_price;
        check_quote(
            CREATE.id,
            "quote",
            price,
            input.max_price,
            scope.evidence.credentials(),
            &[price],
        )
        .map_err(not_sent)?;
        let final_check = |credentials: &SentCredentials| {
            check_quote(
                CREATE.id,
                "quote",
                price,
                input.max_price,
                credentials,
                &[price],
            )
            .err()
            .map(Box::new)
        };
        let mut body = scope.purchase_body(&input, &offer);
        if let Some(fields) = body.as_object_mut() {
            fields.insert("tagDetails".into(), json!([]));
        }
        let path = [&*scope.zone, &*scope.project];
        let target = Destination {
            base: &scope.base,
            path: &path,
            options: Options {
                attempt_check: Some(AttemptCheck(&final_check)),
                ..Options::DEFAULT
            },
        };
        let result = self
            .mutate(ctx, &CREATE, target, Some(&body), scope.evidence)
            .await;
        let mut output = CreateEndpointOutput {
            endpoint_id: String::new(),
            status: String::new(),
            monthly_price: price,
            currency: quote.currency,
        };
        let result = match result {
            Ok(raw) => raw,
            Err(failure) if definite(&failure) => return Err(failure),
            Err(failure) => {
                return Err(endpoint_failure(
                    CREATE.id,
                    Some(failure.into_parts().0),
                    output,
                    scope.evidence,
                ));
            }
        };
        let response_text = go_json(&result);
        let accepted = response_data(CREATE.id, &response_text, scope.evidence).and_then(|row| {
            let id: String = row
                .required("uuid")
                .map_err(|()| invalid(CREATE.id, scope.evidence))?;
            let name: String = row
                .required("name")
                .map_err(|()| invalid(CREATE.id, scope.evidence))?;
            if !qualified(&id, "enp-")
                || scope.evidence.id_reflects(&id)
                || name != input.name
                || before.iter().any(|row| row.id == id)
            {
                return Err(invalid(CREATE.id, scope.evidence));
            }
            Ok(id)
        });
        output.endpoint_id = match accepted {
            Ok(id) => id,
            Err(error) => {
                return Err(endpoint_failure(
                    CREATE.id,
                    Some(error),
                    output,
                    scope.evidence,
                ));
            }
        };
        let wait = ctx
            .child_with_deadline(self.session.clock().monotonic_now() + Duration::from_secs(900));
        scope.ctx = &wait;
        if let Err(error) = wait_for_create(&mut scope, &input, &offer, &mut output).await {
            return Err(endpoint_failure(
                CREATE.id,
                Some(error),
                output,
                scope.evidence,
            ));
        }
        Ok(output)
    }

    /// Deletes an unattached ACTIVE Gateway endpoint and confirms its absence.
    pub async fn delete_endpoint(
        &self,
        ctx: &RequestContext,
        input: DeleteEndpointInput,
    ) -> Result<DeleteEndpointOutput, Failure<DeleteEndpointOutput>> {
        for (field, id, prefix) in [
            ("EndpointID", &input.endpoint_id, "enp-"),
            ("VPCID", &input.vpc_id, "net-"),
        ] {
            if id.is_empty() {
                return Err(not_sent(input_error(
                    DELETE.id,
                    &format!(" requires {field}"),
                )));
            }
            if !qualified(id, prefix) {
                return Err(not_sent(input_error(
                    DELETE.id,
                    &format!(" requires {field} in the form {prefix}<uuid>"),
                )));
            }
        }
        let mut evidence = self.request_evidence(ctx).map_err(not_sent)?;
        super::mutations::check_input(
            DELETE.id,
            &evidence,
            &[],
            &[&input.endpoint_id, &input.vpc_id],
            None,
        )
        .map_err(not_sent)?;
        let mut scope = match EndpointQuote::new(self, ctx, &mut evidence, true).await {
            Ok(scope) => scope,
            Err(error) => return Err(not_sent(scope_error(DELETE.id, error, &evidence))),
        };
        let before = endpoint_rows(&mut scope, DELETE.id, false)
            .await
            .map_err(not_sent)?;
        let Some(row) = before.iter().find(|row| row.id == input.endpoint_id) else {
            return Err(not_sent(typed_error(
                ErrorClass::NotFound,
                "NotFound",
                "vngcloud: resource not found: network.DeleteEndpoint",
                scope.evidence,
            )));
        };
        if row.vpc != input.vpc_id || row.status != "ACTIVE" || !row.gateway || row.dns {
            return Err(not_sent(input_error(
                DELETE.id,
                ": endpoint is not an eligible ACTIVE Gateway endpoint of the named VPC",
            )));
        }
        match row.attached {
            None => return Err(not_sent(invalid(DELETE.id, scope.evidence))),
            Some(true) => return Err(not_sent(typed_error(ErrorClass::Api, "ResourceInUse", "network: resource in use: network.DeleteEndpoint: endpoint has a resource or domain attached", scope.evidence).with_detail(ResourceInUse))),
            Some(false) => {}
        }
        let body = json!({"endpointUuid": row.id, "vpcUuid": row.vpc, "endpointServiceUuid": row.service, "regionUuid": scope.zone, "projectUuid": scope.project});
        let path = [&*scope.zone, &*scope.project, &*row.id];
        let target = Destination {
            base: &scope.base,
            path: &path,
            options: Options::DEFAULT,
        };
        let result = self
            .mutate(ctx, &DELETE, target, Some(&body), scope.evidence)
            .await;
        let mut output = DeleteEndpointOutput {
            endpoint_id: input.endpoint_id,
            deleted: false,
        };
        match result {
            Ok(raw) => {
                if let Err(error) = verify_response(DELETE.id, &raw, scope.evidence) {
                    return Err(endpoint_failure(
                        DELETE.id,
                        Some(error),
                        output,
                        scope.evidence,
                    ));
                }
            }
            Err(failure) if definite(&failure) => return Err(failure),
            Err(failure) => {
                return Err(endpoint_failure(
                    DELETE.id,
                    Some(failure.into_parts().0),
                    output,
                    scope.evidence,
                ));
            }
        }
        let clock = self.session.clock();
        let wait = ctx.child_with_deadline(clock.monotonic_now() + Duration::from_secs(600));
        scope.ctx = &wait;
        let result = async {
            loop {
                check_context(&wait, clock)?;
                let rows = endpoint_rows(&mut scope, DELETE.id, false).await?;
                check_context(&wait, clock)?;
                let Some(row) = rows.iter().find(|row| row.id == output.endpoint_id) else {
                    return Ok(());
                };
                if !matches!(row.status.as_str(), "ACTIVE" | "DELETING") {
                    return Err(invalid(DELETE.id, scope.evidence));
                }
                tick(&wait, clock, Duration::from_secs(5)).await?;
            }
        }
        .await;
        if let Err(error) = result {
            return Err(endpoint_failure(
                DELETE.id,
                Some(error),
                output,
                scope.evidence,
            ));
        }
        output.deleted = true;
        if !safe_output(&output, scope.evidence) {
            return Err(endpoint_failure(DELETE.id, None, output, scope.evidence));
        }
        Ok(output)
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
async fn wait_for_create(
    scope: &mut EndpointQuote<'_>,
    input: &CreateEndpointInput,
    offer: &Offer,
    output: &mut CreateEndpointOutput,
) -> Result<(), Error> {
    let ctx = scope.ctx;
    let clock = scope.client.session.clock();
    loop {
        check_context(ctx, clock)?;
        let rows = endpoint_rows(scope, CREATE.id, true).await?;
        check_context(ctx, clock)?;
        if let Some(row) = rows.into_iter().find(|row| row.id == output.endpoint_id) {
            if row.vpc != input.vpc_id
                || row.subnet != input.subnet_id
                || row.name != input.name
                || row.zone != offer.zone
                || row.service != offer.service
                || row.service_name != offer.service_name
                || row.package != offer.package
                || !row.gateway
                || row.dns
            {
                return Err(invalid(CREATE.id, scope.evidence));
            }
            output.status = row.status;
            match output.status.as_str() {
                "ACTIVE" if safe_output(output, scope.evidence) => return Ok(()),
                "PROVISIONING" => {}
                _ => return Err(invalid(CREATE.id, scope.evidence)),
            }
        }
        tick(ctx, clock, Duration::from_secs(5)).await?;
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
async fn endpoint_rows(
    scope: &mut EndpointQuote<'_>,
    op: &'static str,
    create: bool,
) -> Result<Vec<EndpointRow>, Error> {
    scope
        .endpoint_inventory(op)
        .await?
        .iter()
        .map(|raw| {
            parse_row(raw, &scope.project, create, scope.evidence)
                .map_err(|()| invalid(op, scope.evidence))
        })
        .collect()
}

fn parse_row(
    raw: &RawValue,
    project: &str,
    create: bool,
    evidence: &Evidence,
) -> Result<EndpointRow, ()> {
    let row = Fields::decode(raw.get())?;
    let id: String = row.required("uuid")?;
    let vpc: String = row.required("vpcId")?;
    let service: Fields<'_> = row.required("service")?;
    let service_id: String = service.required("uuid")?;
    let status: String = row.required("status")?;
    let project_id: String = row
        .required::<Fields<'_>>("project")?
        .required("VServerProjectId")?;
    let endpoint_type = if create {
        row.optional::<Option<String>>("endpointType")?.flatten()
    } else {
        Some(row.required("endpointType")?)
    };
    let mut output = EndpointRow {
        id,
        vpc,
        service: service_id,
        status,
        gateway: endpoint_type.as_deref() == Some("Gateway"),
        dns: row.required("enableDnsName")?,
        attached: None,
        name: String::new(),
        service_name: String::new(),
        subnet: String::new(),
        package: String::new(),
        zone: String::new(),
    };
    if !qualified(&output.id, "enp-")
        || !qualified(&output.vpc, "net-")
        || !uuid(&output.service)
        || project_id != project
        || output.status.is_empty()
    {
        return Err(());
    }
    let resource = row.raw("endpointResource")?;
    let domains = row.raw("endpointDomains")?;
    if let (Some(resource), Some(domains)) = (resource, domains)
        && let Ok(domains) = serde_json::from_str::<Vec<&RawValue>>(domains.get())
    {
        output.attached = Some(resource.get() != "null" || !domains.is_empty());
    }
    if create {
        output.name = row.required("endpointName")?;
        output.subnet = row.required::<Fields<'_>>("subnet")?.required("uuid")?;
        output.package = row.required::<Fields<'_>>("packageId")?.required("uuid")?;
        output.service_name = service.optional("name")?.unwrap_or_default();
        output.zone = row.required("zoneUuid")?;
        let catalog: String = row.required("endpointServiceId")?;
        if output.name.is_empty()
            || !hex24(&catalog)
            || evidence.id_reflects(&catalog)
            || !qualified(&output.subnet, "sub-")
            || !uuid(&output.package)
        {
            return Err(());
        }
    }
    if [
        &output.id,
        &output.vpc,
        &output.service,
        &output.subnet,
        &output.package,
        &output.zone,
        &project_id,
    ]
    .into_iter()
    .any(|id| evidence.id_reflects(id))
    {
        return Err(());
    }
    Ok(output)
}

fn qualified(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(uuid)
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn response_data<'a>(
    op: &'static str,
    text: &'a str,
    evidence: &Evidence,
) -> Result<Fields<'a>, Error> {
    let envelope = checked_response(op, text, evidence)?;
    envelope
        .required::<Fields<'a>>("data")
        .map_err(|()| invalid(op, evidence))
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn verify_response(op: &'static str, raw: &[u8], evidence: &Evidence) -> Result<(), Error> {
    checked_response(op, &go_json(raw), evidence).map(|_| ())
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn checked_response<'a>(
    op: &'static str,
    text: &'a str,
    evidence: &Evidence,
) -> Result<Fields<'a>, Error> {
    let rejected = |()| invalid(op, evidence);
    if reflects(text, evidence) {
        return Err(rejected(()));
    }
    let fields = Fields::decode(text).map_err(rejected)?;
    fields.check_ack_paging().map_err(rejected)?;
    let code: &RawValue = fields.required("code").map_err(rejected)?;
    if !fields.required::<bool>("success").map_err(rejected)? || !numeric_code(code.get(), 0) {
        return Err(rejected(()));
    }
    Ok(fields)
}

fn invalid(op: &'static str, evidence: &Evidence) -> Error {
    api(
        op,
        Some(200),
        "InvalidResponse",
        "endpoint response had invalid structure",
        evidence,
    )
}

fn scope_error(op: &'static str, error: Error, evidence: &Evidence) -> Error {
    let Some(detail) = error.api_error() else {
        return screen(error, evidence);
    };
    let message = detail.message().replace(endpoint_quote::OP, op);
    let operation = if detail.operation().is_empty() {
        ""
    } else {
        op
    };
    let mapped = api(
        operation,
        detail.status(),
        detail.code(),
        &message,
        evidence,
    );
    if error.class() == ErrorClass::Api {
        mapped
    } else {
        Error::new(error.class()).with_cause(mapped)
    }
}

fn definite<T>(failure: &Failure<T>) -> bool {
    failure.write_dispatch() == WriteDispatch::NotSent
        || failure
            .error()
            .api_error()
            .and_then(ApiError::status)
            .is_some_and(|status| (400..500).contains(&status) && status != 408)
}

fn endpoint_failure<T: Serialize>(
    op: &'static str,
    cause: Option<Error>,
    output: T,
    evidence: &Evidence,
) -> Failure<T> {
    let message = format!("{op}: write outcome is unconfirmed; {RECOVERY}");
    let mut error = typed_error(ErrorClass::NotSettled, "NotSettled", &message, evidence);
    if let Some(cause) = cause.filter(|cause| !cause.has_class(ErrorClass::NotFound)) {
        error = error.with_cause(cause);
    }
    let partial = safe_output(&output, evidence).then_some(output);
    Failure::new(
        screen(error, evidence),
        partial,
        WriteDispatch::MayHaveBeenSent,
    )
}
