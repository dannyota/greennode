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
        const OP: &str = "volume.QuoteCreateVolume";
        if volume.zone_id.is_empty() || volume.size == 0 || volume.volume_type_id.is_empty() {
            return Err(failure(invalid(
                "vngcloud: invalid input: volume.QuoteCreateVolume requires ZoneID, Size and VolumeTypeID",
            )));
        }
        if volume.size < 0 {
            return Err(failure(invalid(format!(
                "vngcloud: invalid input: {OP}: Size must be greater than 0, got {}",
                volume.size
            ))));
        }
        if !response_id(&volume.volume_type_id)
            || (!volume.encryption_type_id.is_empty() && !response_id(&volume.encryption_type_id))
        {
            return Err(failure(invalid(
                "vngcloud: invalid input: volume.QuoteCreateVolume requires valid VolumeTypeID and EncryptionTypeID",
            )));
        }
        let body = input::volume(
            &volume.zone_id,
            &volume.volume_type_id,
            volume.size,
            &volume.encryption_type_id,
        );
        let mut evidence = Evidence::new();
        self.guard_body(ctx, &mut evidence, &body)
            .map_err(failure)?;
        let details: Option<BTreeMap<String, Value>> = body
            .get("resourceInfo")
            .and_then(Value::as_object)
            .map(|info| info.clone().into_iter().collect());
        let quote = self
            .pricing
            .get_quote(
                ctx,
                GetQuoteInput {
                    resource_type: "volume".into(),
                    action: "create".into(),
                    resource_info: details,
                },
            )
            .await?;
        screen(&evidence, quote, "pricing.GetQuote").map_err(failure)
    }

    /// Reads the current size and type, then quotes a grow without resizing the volume.
    pub async fn quote_resize_volume(
        &self,
        ctx: &RequestContext,
        input: ResizeVolumeInput,
    ) -> Result<GetQuoteOutput, Failure<GetQuoteOutput>> {
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
        if u64::try_from(input.size).map_or(true, |size| size <= current.size) {
            return Err(failure(invalid(format!(
                "vngcloud: invalid input: {OP}: Size {} must be greater than the current size {}",
                input.size, current.size
            ))));
        }
        if !response_id(&current.volume_type_id) {
            return Err(failure(super::wire::shape(
                "volume.GetVolume",
                super::wire::DETAIL_SHAPE,
            )));
        }
        let details = BTreeMap::from([
            ("newSize".into(), json!(input.size)),
            ("newVolumeTypeId".into(), json!(current.volume_type_id)),
            ("volumeId".into(), json!(input.volume_id)),
        ]);
        self.guard_body(ctx, &mut evidence, &json!(details))
            .map_err(failure)?;
        let quoted = self
            .pricing
            .get_quote(
                ctx,
                GetQuoteInput {
                    resource_type: "volume".into(),
                    action: "resize".into(),
                    resource_info: Some(details),
                },
            )
            .await?;
        screen(&evidence, quoted, "pricing.GetQuote").map_err(failure)
    }
}
