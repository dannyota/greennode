use std::collections::HashSet;

use http::Method;
use serde::Serialize;
use serde_json::{Value, json};

use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, ErrorClass, Failure, RequestContext, WriteDispatch};

use super::quotes::{create_quote, resize_bodies};
use super::wire::{Object, Redact, api, decode, invalid};
use super::write::{
    CREATE_UNKNOWN, Mutation, Settlement, available, client_error, finish, in_use, named, not_sent,
    safe, screen_state, settlement_error,
};
use super::{
    Client, CreateVolumeInput, ListVolumesInput, ResizeVolumeInput, UnexpectedStatus, Volume,
    path_id,
};

const CREATE: Mutation = Mutation {
    id: "volume.CreateVolume",
    route: "v2/{}/volumes",
    method: Method::POST,
    once: true,
};
const RESIZE: Mutation = Mutation {
    id: "volume.ResizeVolume",
    route: "v2/{}/volumes/{}/resize",
    method: Method::PUT,
    once: true,
};

/// The created volume and price used for the cap.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateVolumeOutput {
    /// Last confirmed volume, or just its UUID and requested name.
    pub volume: Volume,
    /// Quoted monthly price in VND.
    pub monthly_price: f64,
}

impl Redact for CreateVolumeOutput {
    fn redact(&mut self, evidence: &Evidence) {
        self.volume.redact(evidence);
    }
}

/// The resized volume and adjustment used for the price cap.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResizeVolumeOutput {
    /// Last observed or pre-resize volume.
    pub volume: Volume,
    /// Quoted price adjustment in VND.
    pub quoted_adjustment: f64,
}

impl Redact for ResizeVolumeOutput {
    fn redact(&mut self, evidence: &Evidence) {
        self.volume.redact(evidence);
    }
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Creates a volume after checking its exact name and price cap.
    pub async fn create_volume(
        &self,
        ctx: &RequestContext,
        input: CreateVolumeInput,
    ) -> Result<CreateVolumeOutput, Failure<CreateVolumeOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = Evidence::new();
            if input.name.is_empty() {
                return Err(not_sent(invalid(
                    "vngcloud: invalid input: volume.CreateVolume requires Name",
                )));
            }
            max_price(&CREATE, input.max_price).map_err(not_sent)?;
            let quote = create_quote(CREATE.id, &input).map_err(not_sent)?;
            let mut body = json!({
                "name": input.name, "size": input.size, "volumeTypeId": input.volume_type_id,
                "zoneId": input.zone_id, "isEnableAutoRenew": input.auto_renew,
            });
            if !input.encryption_type_id.is_empty() {
                body["encryptionType"] = json!(input.encryption_type_id);
            }
            self.guard_body(ctx, &mut evidence, &body)
                .map_err(not_sent)?;
            self.refuse_duplicate(ctx, &input.name, &mut evidence)
                .await
                .map_err(not_sent)?;
            let quote = self
                .request_volume_quote(ctx, quote, &mut evidence)
                .await
                .map_err(not_sent)?;
            let monthly = quote.optimum_price;
            check_price(&CREATE, monthly, input.max_price, &evidence).map_err(not_sent)?;
            let project = self.project(ctx, &mut evidence).await.map_err(not_sent)?;
            let ack = self.send_write::<CreateVolumeOutput>(ctx, &CREATE, &[&project], Some(&body), &mut evidence).await.map_err(|failure| {
                let (error, _, dispatch) = failure.into_parts();
                if dispatch != WriteDispatch::MayHaveBeenSent || client_error(&error) || error.api_error().is_some_and(|api| api.message() == CREATE_UNKNOWN) {
                    return Failure::new(error, None, dispatch);
                }
                let advice = "create may have already reached the server; list volumes and match the name exactly before creating it again";
                let reason = error.api_error().map(crate::ApiError::message).filter(|message| !message.is_empty());
                let advice = reason.map_or_else(|| advice.to_owned(), |reason| format!("{advice}: {reason}"));
                Failure::new(error.with_outer_detail(crate::RecoveryAdvice::new(&advice)), None, dispatch)
            })?;
            let ids = if input.no_wait {
                PathIds::WithUnderscore
            } else {
                PathIds::Strict
            };
            let id: String = decode(&super::wire::Document(ack.0), |raw| {
                let envelope = Object::parse(raw)?;
                let data = envelope.get("data")?.ok_or(())?;
                Object::parse(data)?.require("uuid")
            })
            .ok()
            .filter(|id: &String| ids.allows(id) && !evidence.text_reflects(id))
            .ok_or_else(|| {
                Failure::new(
                    api(CREATE.id, Some(202), "", CREATE_UNKNOWN),
                    None,
                    WriteDispatch::MayHaveBeenSent,
                )
            })?;
            let mut volume = Volume {
                uuid: id.clone(),
                name: input.name,
                ..Volume::default()
            };
            let result = if input.no_wait {
                Ok(())
            } else {
                self.settle(ctx, &id, Settlement::Created, &mut volume, &mut evidence)
                    .await
            };
            finish(
                CreateVolumeOutput {
                    volume,
                    monthly_price: monthly,
                },
                result,
                &evidence,
                &CREATE,
                "the volume may exist; list volumes before ordering again",
                WriteDispatch::MayHaveBeenSent,
            )
        })
    }

    /// Grows a volume after checking its state, current size, and price cap.
    pub async fn resize_volume(
        &self,
        ctx: &RequestContext,
        input: ResizeVolumeInput,
    ) -> Result<ResizeVolumeOutput, Failure<ResizeVolumeOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = Evidence::new();
            path_id(RESIZE.id, "VolumeID", &input.volume_id).map_err(not_sent)?;
            if input.size == 0 {
                return Err(not_sent(invalid(
                    "vngcloud: invalid input: volume.ResizeVolume requires Size",
                )));
            }
            max_price(&RESIZE, input.max_price).map_err(not_sent)?;
            self.guard_inputs(
                ctx,
                &mut evidence,
                &[&input.volume_id],
                Some(&json!(input.size)),
            )
            .map_err(not_sent)?;
            let mut volume = self
                .volume(ctx, &input.volume_id, &mut evidence)
                .await
                .map_err(not_sent)?;
            screen_state(&evidence, &volume).map_err(not_sent)?;
            if !available(&volume) && !in_use(&volume) {
                return Err(not_sent(Box::new(
                    Error::new(ErrorClass::Api)
                        .with_detail(UnexpectedStatus)
                        .with_cause(api(
                            "",
                            None,
                            "",
                            &format!(
                                "volume: unexpected status: volume.ResizeVolume: volume {} is {:?}",
                                input.volume_id, volume.status
                            ),
                        )),
                )));
            }
            let (size, body, quote) =
                resize_bodies(RESIZE.id, &input, &volume).map_err(not_sent)?;
            let quote = self
                .request_volume_quote(ctx, quote, &mut evidence)
                .await
                .map_err(not_sent)?;
            let adjustment = quote.optimum_price;
            check_price(&RESIZE, adjustment, input.max_price, &evidence).map_err(not_sent)?;
            let project = self.project(ctx, &mut evidence).await.map_err(not_sent)?;
            const CHECK: &str = "call GetVolume to check its status";
            if let Err(failure) = self
                .send_write::<ResizeVolumeOutput>(
                    ctx,
                    &RESIZE,
                    &[&project, &input.volume_id],
                    Some(&body),
                    &mut evidence,
                )
                .await
            {
                let (error, _, dispatch) = failure.into_parts();
                if dispatch != WriteDispatch::MayHaveBeenSent || client_error(&error) {
                    return Err(Failure::new(error, None, dispatch));
                }
                return Err(Failure::new(
                    settlement_error(error, RESIZE.id, CHECK),
                    safe(
                        ResizeVolumeOutput {
                            volume,
                            quoted_adjustment: adjustment,
                        },
                        &evidence,
                    ),
                    dispatch,
                ));
            }
            let result = if input.no_wait {
                Ok(())
            } else {
                self.settle(
                    ctx,
                    &input.volume_id,
                    Settlement::Resized(size),
                    &mut volume,
                    &mut evidence,
                )
                .await
            };
            finish(
                ResizeVolumeOutput {
                    volume,
                    quoted_adjustment: adjustment,
                },
                result,
                &evidence,
                &RESIZE,
                CHECK,
                WriteDispatch::MayHaveBeenSent,
            )
        })
    }

    async fn refuse_duplicate(
        &self,
        ctx: &RequestContext,
        name: &str,
        evidence: &mut Evidence,
    ) -> Result<(), Box<Error>> {
        const MAX_PAGES: i64 = 50;
        let incomplete = || {
            invalid(
                "vngcloud: invalid input: volume.CreateVolume: could not confirm no volume has the requested name: incomplete inventory",
            )
        };
        let mut ids = HashSet::new();
        let mut expected = None;
        for page in 1..=MAX_PAGES {
            let input = ListVolumesInput {
                name: name.into(),
                page,
                size: 0,
            };
            let result = self.volumes(ctx, &input, evidence).await.map_err(|error| {
                if error
                    .api_error()
                    .is_some_and(|api| api.message() == super::wire::PAGE_SHAPE)
                {
                    incomplete()
                } else {
                    error
                }
            })?;
            let count = i64::try_from(result.items.len()).map_err(|_| incomplete())?;
            let totals = (result.total_page, result.total_item, result.page_size);
            if result.page != page
                || result.page_size <= 0
                || count > result.page_size
                || !(0..=MAX_PAGES).contains(&result.total_page)
                || result.total_item < 0
                || totals != *expected.get_or_insert(totals)
                || (count == 0 && (result.total_item != 0 || page != 1 || result.total_page > 1))
            {
                return Err(incomplete());
            }
            for row in result.items {
                if !super::inventory::response_id(&row.uuid)
                    || row.name.is_empty()
                    || !ids.insert(row.uuid)
                {
                    return Err(incomplete());
                }
                if row.name == name {
                    return Err(invalid(
                        "vngcloud: invalid input: volume.CreateVolume: a volume with the requested name already exists",
                    ));
                }
            }
            let seen = i64::try_from(ids.len()).map_err(|_| incomplete())?;
            if seen > result.total_item || (result.total_page == 0 && seen != 0) {
                return Err(incomplete());
            }
            if page >= result.total_page {
                return if seen == result.total_item {
                    Ok(())
                } else {
                    Err(incomplete())
                };
            }
        }
        Err(incomplete())
    }
}

fn max_price(operation: &Mutation, cap: f64) -> Result<(), Box<Error>> {
    if !cap.is_finite() || cap < 0.0 {
        return Err(invalid(format!(
            "vngcloud: invalid input: {}: MaxPrice must be finite and nonnegative",
            operation.id
        )));
    }
    Ok(())
}

fn check_price(
    operation: &Mutation,
    quoted: f64,
    cap: f64,
    evidence: &Evidence,
) -> Result<(), Box<Error>> {
    if evidence.output_reflects(&Value::from(quoted)) {
        return Err(withheld_price());
    }
    let (class, message) = if quoted <= 0.0 {
        (
            ErrorClass::Unpriced,
            format!(
                "vngcloud: quote priced the order at 0; refusing to order an unpriced paid resource: {}: quote {quoted} VND",
                operation.id
            ),
        )
    } else if quoted > cap {
        (
            ErrorClass::PriceAboveMax,
            format!(
                "vngcloud: quoted price is above MaxPrice: {}: quote {quoted:.0} VND exceeds MaxPrice {cap:.0} VND",
                operation.id
            ),
        )
    } else {
        return Ok(());
    };
    if evidence.text_reflects(&message) {
        return Err(withheld_price());
    }
    Err(Box::new(named(class, &message)))
}

fn withheld_price() -> Box<Error> {
    Box::new(api(
        "pricing.GetQuote",
        None,
        "InvalidResponse",
        "invalid quote response; body withheld",
    ))
}
