use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::pricing::{GetQuoteInput, GetQuoteOutput};
use crate::quote::input;
use crate::wire::product::evidence::Evidence;
use crate::{Failure, RequestContext};

use super::inventory::response_id;
use super::wire::{failure, invalid, screen};
use super::{Client, path_id};

/// Parameters for the price of a new volume.
#[derive(Clone, Debug, Default)]
pub struct CreateVolumeInput {
    /// Volume name, ignored by the quote.
    pub name: String,
    /// Target zone.
    pub zone_id: String,
    /// Requested size in GB.
    pub size: i64,
    /// Selected volume type.
    pub volume_type_id: String,
    /// Optional encryption key.
    pub encryption_type_id: String,
    /// Automatic renewal selection, ignored by the quote.
    pub auto_renew: bool,
    /// Maximum price, ignored by the quote.
    pub max_price: f64,
    /// Settlement selection, ignored by the quote.
    pub no_wait: bool,
}

/// Parameters for the price of growing an existing volume.
#[derive(Clone, Debug, Default)]
pub struct ResizeVolumeInput {
    /// Volume UUID.
    pub volume_id: String,
    /// New size in GB.
    pub size: i64,
    /// Maximum price, ignored by the quote.
    pub max_price: f64,
    /// Settlement selection, ignored by the quote.
    pub no_wait: bool,
}

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Quotes a new volume without placing an order.
    pub async fn quote_create_volume(
        &self,
        ctx: &RequestContext,
        volume: CreateVolumeInput,
    ) -> Result<GetQuoteOutput, Failure<GetQuoteOutput>> {
        crate::wire::product::exit::finish!(async {
            let quote = create_quote("volume.QuoteCreateVolume", &volume).map_err(failure)?;
            self.request_volume_quote(ctx, quote, &mut Evidence::new())
                .await
                .map_err(failure)
        })
    }

    /// Reads the current size and type, then quotes a grow without resizing the volume.
    pub async fn quote_resize_volume(
        &self,
        ctx: &RequestContext,
        input: ResizeVolumeInput,
    ) -> Result<GetQuoteOutput, Failure<GetQuoteOutput>> {
        crate::wire::product::exit::finish!(async {
            const OP: &str = "volume.QuoteResizeVolume";
            if input.size == 0 {
                return Err(failure(invalid(
                    "vngcloud: invalid input: volume.QuoteResizeVolume requires Size",
                )));
            }
            path_id(OP, "VolumeID", &input.volume_id).map_err(failure)?;
            let mut evidence = Evidence::new();
            self.guard_body(ctx, &mut evidence, &json!([input.volume_id, input.size]))
                .map_err(failure)?;
            let current = self
                .volume(ctx, &input.volume_id, &mut evidence)
                .await
                .map_err(failure)?;
            self.guard_body(
                ctx,
                &mut evidence,
                &json!([current.size, current.volume_type_id]),
            )
            .map_err(failure)?;
            let (_, _, quote) = resize_bodies(OP, &input, &current).map_err(failure)?;
            self.request_volume_quote(ctx, quote, &mut evidence)
                .await
                .map_err(failure)
        })
    }

    pub(super) async fn request_volume_quote(
        &self,
        ctx: &RequestContext,
        quote: GetQuoteInput,
        evidence: &mut Evidence,
    ) -> Result<GetQuoteOutput, Box<crate::Error>> {
        self.guard_body(ctx, evidence, &json!(quote.resource_info))?;
        let output = self
            .pricing
            .quote_with_evidence(ctx, quote, evidence)
            .await
            .map_err(|error| Box::new(error.into_parts().0))?;
        screen(evidence, output, "pricing.GetQuote")
    }
}

pub(super) fn create_quote(
    op: &str,
    volume: &CreateVolumeInput,
) -> Result<GetQuoteInput, Box<crate::Error>> {
    if volume.zone_id.is_empty() || volume.size == 0 || volume.volume_type_id.is_empty() {
        return Err(invalid(format!(
            "vngcloud: invalid input: {op} requires ZoneID, Size and VolumeTypeID"
        )));
    }
    if volume.size < 0 {
        return Err(invalid(format!(
            "vngcloud: invalid input: {op}: Size must be greater than 0"
        )));
    }
    path_id(op, "VolumeTypeID", &volume.volume_type_id)?;
    if !volume.encryption_type_id.is_empty() && !response_id(&volume.encryption_type_id) {
        return Err(invalid(format!(
            "vngcloud: invalid input: {op} requires EncryptionTypeID to match ^[A-Za-z0-9_-]+$"
        )));
    }
    let body = input::volume(
        &volume.zone_id,
        &volume.volume_type_id,
        volume.size,
        &volume.encryption_type_id,
    );
    let details = body
        .get("resourceInfo")
        .and_then(Value::as_object)
        .map(|info| {
            info.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        });
    Ok(GetQuoteInput {
        resource_type: "volume".into(),
        action: "create".into(),
        resource_info: details,
    })
}

pub(super) fn resize_bodies(
    op: &str,
    input: &ResizeVolumeInput,
    volume: &super::Volume,
) -> Result<(u64, Value, GetQuoteInput), Box<crate::Error>> {
    let size = u64::try_from(input.size)
        .ok()
        .filter(|size| *size > volume.size)
        .ok_or_else(|| {
            invalid(format!(
                "vngcloud: invalid input: {op}: Size must be greater than the current size"
            ))
        })?;
    if !response_id(&volume.volume_type_id) {
        return Err(super::wire::shape(
            "volume.GetVolume",
            super::wire::DETAIL_SHAPE,
        ));
    }
    let mut details = BTreeMap::from([
        ("newSize".into(), json!(input.size)),
        ("newVolumeTypeId".into(), json!(volume.volume_type_id)),
    ]);
    let body = json!(details);
    details.insert("volumeId".into(), json!(input.volume_id));
    Ok((
        size,
        body,
        GetQuoteInput {
            resource_type: "volume".into(),
            action: "resize".into(),
            resource_info: Some(details),
        },
    ))
}
