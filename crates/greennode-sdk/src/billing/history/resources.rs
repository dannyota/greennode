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
