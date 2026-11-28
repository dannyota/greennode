use super::failure;
use crate::billing::Client;
use crate::billing_resources;
use crate::wire::product::evidence::Evidence;
use crate::{Failure, RequestContext};

pub use billing_resources::{
    ListResourcesOutput, Resource, ResourceBillingElement, ResourceThresholds,
};

/// Billing resource cannot be renewed.
pub const RENEW_TYPE_NON_RENEWABLE: &str = "NON-RENEWABLE";
/// Billing resource renews only by request.
pub const RENEW_TYPE_MANUAL: &str = "MANUAL";
/// Billing resource is set to renew automatically.
pub const RENEW_TYPE_AUTO_RENEW: &str = "AUTO-RENEW";

/// Reads prepaid resources across products without filters.
#[derive(Clone, Copy, Default)]
pub struct ListResourcesInput {}

crate::error::debug_record!(ListResourcesInput {});

crate::wire::product::output::public_fields!(ListResourcesOutput {
    items,
    total_auto_renews,
    total_good_resources,
    total_warning_resources,
    total_alerted_resources,
    total_expired_resources,
    extra
});

crate::wire::product::output::public_fields!(Resource {
    artifact_id,
    artifact_name,
    artifact_type,
    product,
    renew_type,
    billing_type,
    start_billing_time,
    end_billing_time,
    date_left,
    renew_period,
    channel,
    is_renewing,
    cost,
    status,
    status_ui,
    billing_elements
});

crate::wire::product::output::public_fields!(ResourceBillingElement {
    sku,
    quantity,
    meta_key
});

crate::wire::product::output::public_fields!(ResourceThresholds {
    warning_thresholds,
    alarm_thresholds
});

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Reads account-wide prepaid renewal state.
    pub async fn list_resources(
        &self,
        ctx: &RequestContext,
        _input: ListResourcesInput,
    ) -> Result<ListResourcesOutput, Failure<ListResourcesOutput>> {
        crate::wire::product::exit::finish!(async {
            billing_resources::list(&self.session, ctx, &mut Evidence::new())
                .await
                .map(billing_resources::ResourceList::into_projection)
                .map_err(|error| failure(*error))
        })
    }
}
