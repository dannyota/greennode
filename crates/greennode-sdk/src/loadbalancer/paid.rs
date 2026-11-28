mod settlement;

use std::time::Duration;

use http::{Method, StatusCode};
use serde::Serialize;
use serde_json::json;

use crate::model::wire::{WireField, go_json};
use crate::quote::request::{self, Product, Request};
use crate::wire::product::guard::{Guarded, guarded_poll};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, ErrorClass, Failure, RecoveryAdvice, RequestContext, WriteDispatch};

use super::wire::{DecodeError, Envelope, Read, invalid_input, screen_input, validate_ids};
use super::write::{Write, advise, api, not_sent, reject};
use super::{
    Client, CreateLoadBalancerInput, LoadBalancer, ResizeLoadBalancerInput, ZonedLoadBalancer,
};

const CREATE: &str = "loadbalancer.CreateLoadBalancer";
const RESIZE: &str = "loadbalancer.ResizeLoadBalancer";
const GET: Read = Read {
    id: "loadbalancer.GetLoadBalancer",
    route: "v2/{}/loadBalancers/{}",
};
const CREATE_WRITE: Write = Write {
    id: CREATE,
    route: "v2/{}/loadBalancers",
    method: Method::POST,
    success: &[StatusCode::OK, StatusCode::ACCEPTED],
    sensitive: false,
    report_redirect: true,
};
const RESIZE_WRITE: Write = Write {
    id: RESIZE,
    route: "v2/{}/loadBalancers/{}/resize",
    method: Method::PUT,
    success: &[StatusCode::OK, StatusCode::ACCEPTED],
    sensitive: false,
    report_redirect: true,
};
const CREATE_ADVICE: &str = "loadbalancer.CreateLoadBalancer: create may have already reached the server; check with list-load-balancers --name and match the name exactly";
const POLL: Duration = Duration::from_secs(10);
/// Result of ordering a regional load balancer.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateLoadBalancerOutput {
    /// The confirmed resource, or known submitted fields when confirmation is skipped.
    pub load_balancer: LoadBalancer,
    /// Accepted quote in VND per month.
    pub quoted_price: f64,
}

crate::wire::product::output::public_fields!(CreateLoadBalancerOutput {
    load_balancer,
    quoted_price
});

/// Result of resizing a regional load balancer.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResizeLoadBalancerOutput {
    /// Resource metadata at the last observed state.
    pub load_balancer: LoadBalancer,
    /// Accepted quote in VND, or zero when the package already matched.
    pub quoted_price: f64,
    /// Whether a resize was submitted.
    pub changed: bool,
}

crate::wire::product::output::public_fields!(ResizeLoadBalancerOutput {
    load_balancer,
    quoted_price,
    changed
});

/// A regional write could not proceed while the resource was busy.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ResourceBusy;

crate::error::debug_record!(ResourceBusy unit);

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Quotes and orders a new regional load balancer, then optionally confirms creation.
    pub async fn create_load_balancer(
        &self,
        ctx: &RequestContext,
        input: CreateLoadBalancerInput,
    ) -> Result<CreateLoadBalancerOutput, Failure<CreateLoadBalancerOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = self.order_evidence(ctx).map_err(|error| not_sent(*error))?;
            let fields = [
                ("Name", input.name.as_str()),
                ("PackageID", &input.package_id),
                ("Type", &input.r#type),
                ("Scheme", &input.scheme),
                ("SubnetID", &input.subnet_id),
                ("ZoneID", &input.zone_id),
            ];
            screen_input(&evidence, fields.iter().map(|(_, value)| *value))
                .map_err(|error| not_sent(*error))?;
            for (field, value) in fields {
                if value.is_empty() {
                    return Err(not_sent(*invalid_input(&format!(
                        "{CREATE} requires {field}"
                    ))));
                }
            }
            if !matches!(input.scheme.as_str(), "Internet" | "Internal") {
                return Err(not_sent(*invalid_input(&format!(
                    "{CREATE}: Scheme must be \"Internet\" or \"Internal\", got {:?}",
                    input.scheme
                ))));
            }
            validate_ids(
                CREATE,
                &[
                    ("PackageID", &input.package_id),
                    ("SubnetID", &input.subnet_id),
                ],
                PathIds::Strict,
            )
            .map_err(|error| not_sent(*error))?;
            check_max_price(CREATE, input.max_price, &evidence)
                .map_err(|error| not_sent(*error))?;
            let price = self
                .guard_price(
                    ctx,
                    &mut evidence,
                    CREATE,
                    input.max_price,
                    PriceAction::Create(&input),
                )
                .await
                .map_err(not_sent)?;
            screen_input(&evidence, fields.iter().map(|(_, value)| *value))
                .map_err(|error| not_sent(*error))?;
            let body = json!({
                "name": input.name, "packageId": input.package_id, "scheme": input.scheme,
                "subnetId": input.subnet_id, "type": input.r#type, "zoneId": input.zone_id,
                "autoScalable": false, "isPoc": false,
            });
            let result = self
                .send_once(ctx, &CREATE_WRITE, &[], body, &mut evidence)
                .await;
            let advice = |failure| advise(failure, CREATE_ADVICE, &evidence);
            let reply = result.map_err(advice)?;
            let status = reply.status;
            reject(&CREATE_WRITE, &reply, &evidence, None).map_err(|failure| {
                if status.is_client_error() {
                    failure
                } else {
                    advice(failure)
                }
            })?;
            let read = Read {
                id: CREATE,
                route: CREATE_WRITE.route,
            };
            let decode_failed = |error: Box<Error>| {
                advice(Failure::new(*error, None, WriteDispatch::MayHaveBeenSent))
            };
            let receipt = reply.clean_receipt().ok_or_else(|| {
                if reply.malformed_json() {
                    return decode_failed(read.decode_error(&evidence, DecodeError::MalformedJson));
                }
                decode_failed(Box::new(
                    crate::wire::product::operation::unsafe_guard_response(CREATE),
                ))
            })?;
            let text = go_json(receipt.body());
            serde_json::from_str::<&serde_json::value::RawValue>(&text).map_err(|_| {
                decode_failed(read.decode_error(&evidence, DecodeError::MalformedJson))
            })?;
            crate::wire::strict_json::validate(text.as_bytes(), &[], text.len()).map_err(|_| {
                decode_failed(read.decode_error(&evidence, DecodeError::InvalidShape))
            })?;
            let envelope = Envelope::parse(&text, &read, &evidence).map_err(decode_failed)?;
            let id: String = read
                .parse(
                    envelope
                        .raw(&read, &evidence, "uuid")
                        .map_err(decode_failed)?,
                    &evidence,
                )
                .map_err(decode_failed)?;
            let policy = if input.no_wait {
                PathIds::WithUnderscore
            } else {
                PathIds::Strict
            };
            if !policy.allows(&id) || evidence.id_reflects(&id) {
                let error = api(
                    CREATE,
                    Some(status),
                    "",
                    "create response had no id; the resource may exist, check with list-load-balancers --name and match the name exactly",
                    &evidence,
                );
                return Err(Failure::new(error, None, WriteDispatch::MayHaveBeenSent));
            }
            let fallback = LoadBalancer {
                uuid: id.clone(),
                name: input.name,
                package_id: input.package_id,
                r#type: input.r#type,
                load_balancer_schema: input.scheme,
                private_subnet_id: input.subnet_id,
                zone_id: input.zone_id,
                ..Default::default()
            };
            let target = Target::Create;
            let acknowledgement = receipt.bind_id(id.clone());
            evidence
                .keep_acknowledgement(&acknowledgement)
                .map_err(|error| Failure::new(*error, None, WriteDispatch::MayHaveBeenSent))?;
            let (load_balancer, error) = if input.no_wait {
                (fallback, None)
            } else {
                self.settle(
                    ctx,
                    &mut evidence,
                    target,
                    &id,
                    fallback,
                    Some(&acknowledgement),
                )
                .await
            };
            if let Err(refusal) = acknowledgement.recheck(&evidence) {
                let error = error.unwrap_or_else(|| {
                    outcome(ErrorClass::NotSettled, CREATE_ADVICE, &evidence).with_cause(*refusal)
                });
                return Err(Failure::new(error, None, WriteDispatch::MayHaveBeenSent));
            }
            complete(
                load_balancer,
                price,
                error,
                &evidence,
                target,
                |load_balancer| CreateLoadBalancerOutput {
                    load_balancer,
                    quoted_price: price,
                },
            )
        })
    }

    /// Quotes and submits a package change once, then optionally confirms the new package.
    pub async fn resize_load_balancer(
        &self,
        ctx: &RequestContext,
        input: ResizeLoadBalancerInput,
    ) -> Result<ResizeLoadBalancerOutput, Failure<ResizeLoadBalancerOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = self.order_evidence(ctx).map_err(|error| not_sent(*error))?;
            let id = &input.load_balancer_id;
            let package = &input.package_id;
            screen_input(&evidence, [id.as_str(), package]).map_err(|error| not_sent(*error))?;
            validate_ids(RESIZE, &[("LoadBalancerID", id)], PathIds::Strict)
                .map_err(|error| not_sent(*error))?;
            validate_ids(RESIZE, &[("PackageID", package)], PathIds::Strict)
                .map_err(|error| not_sent(*error))?;
            check_max_price(RESIZE, input.max_price, &evidence)
                .map_err(|error| not_sent(*error))?;
            let _lock = self.lock_load_balancer(ctx, id).await.map_err(not_sent)?;
            let mut current = self
                .read_state(ctx, id, &mut evidence, PackageRequirement::Required)
                .await
                .and_then(|row| row.into_checked(&evidence))
                .map_err(|error| not_sent(*error))?;
            if current.package_id == *package {
                let load_balancer = GET
                    .retain(current, &evidence)
                    .map_err(|error| not_sent(*error))?;
                if price_reflects(0.0, &evidence) || evidence.text_reflects("false") {
                    return Err(not_sent(
                        *GET.decode_error(&evidence, DecodeError::ReflectedCredential),
                    ));
                }
                return Ok(ResizeLoadBalancerOutput {
                    load_balancer,
                    quoted_price: 0.0,
                    changed: false,
                });
            }
            self.wait_ready(ctx, id, &mut evidence)
                .await
                .map_err(not_sent)?;
            let price = self
                .guard_price(
                    ctx,
                    &mut evidence,
                    RESIZE,
                    input.max_price,
                    PriceAction::Resize(&input),
                )
                .await
                .map_err(not_sent)?;
            screen_input(&evidence, [id.as_str(), package]).map_err(|error| not_sent(*error))?;
            let result = self
                .send_once(
                    ctx,
                    &RESIZE_WRITE,
                    &[id],
                    json!({ "packageId": package }),
                    &mut evidence,
                )
                .await;
            let advice = format!(
                "{RESIZE}: load balancer {id}: resize may have already reached the server; read the load balancer with GetLoadBalancer and compare its package to the one requested before any rerun"
            );
            let reply = result.map_err(|failure| advise(failure, &advice, &evidence))?;
            let status = reply.status;
            let busy = status.is_client_error()
                && reply.clean_receipt().is_some_and(|receipt| {
                    crate::wire::product::response::error_message_matches(
                        receipt.body(),
                        evidence.credentials(),
                        |message| {
                            let message = message.to_ascii_lowercase();
                            ["is not ready", "is updating", "is creating", "is deleting"]
                                .iter()
                                .any(|part| message.contains(part))
                        },
                    )
                });
            match reject(&RESIZE_WRITE, &reply, &evidence, None) {
                Ok(()) => (),
                Err(failure) => {
                    if busy && let Some(api_error) = failure.error().api_error() {
                        let message = format!(
                            "loadbalancer: resource busy: {RESIZE}: load balancer {id}: {RESIZE}: {} (status {status_code})",
                            api_error.message(),
                            status_code = status.as_u16()
                        );
                        let (error, _, dispatch) = failure.into_parts();
                        let error = outcome(ErrorClass::Api, &message, &evidence)
                            .with_outer_detail(ResourceBusy)
                            .with_cause(error);
                        return Err(Failure::new(error, None, dispatch));
                    }
                    return Err(if status.is_client_error() {
                        failure
                    } else {
                        advise(failure, &advice, &evidence)
                    });
                }
            };
            let target = Target::Resize(package);
            let (load_balancer, error) = if input.no_wait {
                let receipt = reply.clean_receipt().ok_or_else(|| {
                    advise(
                        Failure::new(
                            crate::wire::product::operation::unsafe_guard_response(RESIZE),
                            None,
                            WriteDispatch::MayHaveBeenSent,
                        ),
                        &advice,
                        &evidence,
                    )
                })?;
                check_resize_acknowledgement(receipt.body(), id, &evidence).map_err(|error| {
                    let failure = Failure::new(*error, None, WriteDispatch::MayHaveBeenSent);
                    advise(failure, &advice, &evidence)
                })?;
                current.package_id.clone_from(package);
                (current, None)
            } else {
                reply
                    .check_discarded_acknowledgement(RESIZE)
                    .map_err(|error| {
                        advise(
                            Failure::new(*error, None, WriteDispatch::MayHaveBeenSent),
                            &advice,
                            &evidence,
                        )
                    })?;
                self.settle(ctx, &mut evidence, target, id, current, None)
                    .await
            };
            complete(
                load_balancer,
                price,
                error,
                &evidence,
                target,
                |load_balancer| ResizeLoadBalancerOutput {
                    load_balancer,
                    quoted_price: price,
                    changed: true,
                },
            )
        })
    }

    async fn read_state(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
        package_requirement: PackageRequirement,
    ) -> Result<Guarded<LoadBalancer>, Box<Error>> {
        let text = self
            .read_guard_text(
                ctx,
                super::wire::GuardRead(&GET),
                &[("LoadBalancerID", id)],
                &[],
                evidence,
            )
            .await?;
        let envelope = Envelope::parse(&text, &GET, evidence)?;
        let body = envelope.canonical_raw(&GET, evidence, "data")?;
        let ZonedLoadBalancer(row) =
            super::guard::parse(&GET, body, &super::guard::LOAD_BALANCER, evidence)?;
        let require_package = match package_requirement {
            PackageRequirement::Required => true,
            PackageRequirement::UnlessFailed => row.progress_status != "ERROR",
            PackageRequirement::Optional => false,
        };
        if row.uuid != id
            || row.progress_status.is_empty()
            || (require_package && row.package_id.is_empty())
        {
            return Err(GET.decode_error(evidence, DecodeError::InvalidShape));
        }
        if evidence.text_reflects(&row.uuid)
            || evidence.text_reflects(&row.progress_status)
            || (require_package && evidence.text_reflects(&row.package_id))
        {
            return Err(GET.decode_error(evidence, DecodeError::ReflectedCredential));
        }
        Guarded::latest(row, evidence)
    }

    async fn wait_ready(
        &self,
        ctx: &RequestContext,
        id: &str,
        evidence: &mut Evidence,
    ) -> Result<(), Error> {
        let clock = self.session.clock();
        let bound = clock.monotonic_now() + Duration::from_secs(600);
        let wait = ctx.child_with_deadline(bound);
        guarded_poll!(evidence, {
            if let Some(end) = ctx.check(clock) {
                return Err(Error::from(end));
            }
            if clock.monotonic_now() >= bound {
                return Err(outcome(ErrorClass::Api, &format!("loadbalancer: resource busy: {RESIZE}: load balancer {id} is not ready within 10m0s; nothing sent"), evidence).with_outer_detail(ResourceBusy));
            }
            match self
                .read_state(&wait, id, evidence, PackageRequirement::Required)
                .await
                .and_then(|row| row.into_checked(evidence))
            {
                Ok(row)
                    if !matches!(
                        row.progress_status.as_str(),
                        "CREATING" | "CREATING-BILLING" | "UPDATING" | "DELETING"
                    ) =>
                {
                    return Ok(());
                }
                Ok(_) => (),
                Err(error) if clock.monotonic_now() < bound => return Err(*error),
                Err(_) => continue,
            }
            wait.child_with_deadline(clock.monotonic_now() + Duration::from_secs(5))
                .wait(clock)
                .await;
        });
    }

    async fn guard_price(
        &self,
        ctx: &RequestContext,
        evidence: &mut Evidence,
        op: &'static str,
        limit: f64,
        action: PriceAction<'_>,
    ) -> Result<f64, Error> {
        let (kind, details) = match action {
            PriceAction::Create(input) => ("create", super::quotes::create_quote_info(input)),
            PriceAction::Resize(input) => ("resize", super::quotes::resize_quote_info(input)),
        };
        let body = crate::quote::input::regional("load-balancer", kind, Some(&details));
        let raw = request::exchange(
            &self.session,
            ctx,
            evidence,
            Request {
                operation: op,
                template: "v1/price",
                product: Product::Portal,
                path: &[],
                query: &[],
                body: Some(&body),
            },
            None,
        )
        .await
        .map_err(|error| evidence.screen_error(*error))?;
        let read = Read {
            id: op,
            route: "v1/price",
        };
        let result = Envelope::parse(raw.get(), &read, evidence).map_err(|error| *error)?;
        let raw = result
            .canonical_raw(&read, evidence, "optimumPrice")
            .map_err(|error| *error)?;
        if raw == "null" {
            return Err(api(op, None, "", "quote response had no price", evidence));
        }
        let price = raw
            .parse::<f64>()
            .ok()
            .filter(|price| price.is_finite() && !price_reflects(*price, evidence))
            .ok_or_else(|| *read.decode_error(evidence, DecodeError::InvalidShape))?;
        let (price, limit) = Guarded::price(evidence, price, limit)
            .and_then(|price| price.into_checked(evidence))
            .map_err(|error| *error)?;
        if price == 0.0 || (kind == "create" && price < 0.0) {
            return Err(outcome(
                ErrorClass::Unpriced,
                &format!(
                    "vngcloud: quote priced the order at 0; refusing to order an unpriced paid resource: {op}: quote {price:.0} VND"
                ),
                evidence,
            ));
        }
        if price > limit {
            return Err(outcome(
                ErrorClass::PriceAboveMax,
                &format!(
                    "vngcloud: quoted price is above MaxPrice: {op}: quote {price:.0} VND exceeds MaxPrice {limit:.0} VND"
                ),
                evidence,
            ));
        }
        Ok(price)
    }
}

#[derive(Clone, Copy)]
enum PackageRequirement {
    Required,
    UnlessFailed,
    Optional,
}

fn check_resize_acknowledgement(
    body: &[u8],
    id: &str,
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    let text = go_json(body);
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }
    let read = Read {
        id: RESIZE,
        route: RESIZE_WRITE.route,
    };
    serde_json::from_str::<&serde_json::value::RawValue>(text)
        .map_err(|_| read.decode_error(evidence, DecodeError::MalformedJson))?;
    crate::wire::strict_json::validate(text.as_bytes(), &[], text.len())
        .map_err(|_| read.decode_error(evidence, DecodeError::InvalidShape))?;
    let envelope = Envelope::parse(text, &read, evidence)?;
    let acknowledged: Option<String> =
        read.parse(envelope.raw(&read, evidence, "uuid")?, evidence)?;
    if acknowledged
        .as_deref()
        .is_some_and(|acknowledged| acknowledged != id)
    {
        return Err(read.decode_error(evidence, DecodeError::InvalidShape));
    }
    Ok(())
}

fn check_max_price(op: &str, price: f64, evidence: &Evidence) -> Result<(), Box<Error>> {
    if price.is_finite() && price >= 0.0 {
        return Ok(());
    }
    let value = format!("{price}");
    let value = if evidence.text_reflects(&value) {
        "<redacted>"
    } else {
        &value
    };
    Err(invalid_input(&format!(
        "{op}: MaxPrice must be a non-negative, finite number, got {value}"
    )))
}

fn price_reflects(price: f64, evidence: &Evidence) -> bool {
    evidence.output_number_reflects(price, true) || evidence.text_reflects(&format!("{price:.0}"))
}

fn outcome(class: ErrorClass, message: &str, evidence: &Evidence) -> Error {
    let message = if evidence.text_reflects(message) {
        ""
    } else {
        message
    };
    Error::new(class).with_detail(RecoveryAdvice::new(message))
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
fn complete<O>(
    mut row: LoadBalancer,
    price: f64,
    error: Option<Error>,
    evidence: &Evidence,
    target: Target<'_>,
    output: impl FnOnce(LoadBalancer) -> O,
) -> Result<O, Failure<O>> {
    let safe = evidence.can_retain_partial()
        && row.redact(evidence)
        && !price_reflects(price, evidence)
        && (matches!(target, Target::Create) || !evidence.text_reflects("true"));
    match error {
        None if safe => Ok(output(row)),
        error => Err(Failure::new(
            error.unwrap_or_else(|| {
                outcome(
                    ErrorClass::NotSettled,
                    "write accepted; output withheld",
                    evidence,
                )
            }),
            safe.then(|| output(row)),
            WriteDispatch::MayHaveBeenSent,
        )),
    }
}

enum PriceAction<'a> {
    Create(&'a CreateLoadBalancerInput),
    Resize(&'a ResizeLoadBalancerInput),
}

#[derive(Clone, Copy)]
enum Target<'a> {
    Create,
    Resize(&'a str),
}

impl Target<'_> {
    fn operation(self) -> &'static str {
        match self {
            Self::Create => CREATE,
            Self::Resize(_) => RESIZE,
        }
    }

    fn matches(self, row: &LoadBalancer) -> bool {
        match self {
            Self::Create => true,
            Self::Resize(package) => row.package_id == package,
        }
    }

    fn settlement(self) -> (u64, String, &'static str) {
        match self {
            Self::Create => (
                20,
                "CREATED".into(),
                "the load balancer was ordered and must not be ordered again",
            ),
            Self::Resize(package) => (
                45,
                format!("CREATED with package {package}"),
                "the resize was accepted and must not be repeated",
            ),
        }
    }
}
