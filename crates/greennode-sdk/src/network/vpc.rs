mod models;
mod policy;
mod subnets;
mod wire;

#[cfg(test)]
mod deadlines;
#[cfg(test)]
mod tests;

pub use models::*;

use crate::{Failure, RequestContext};

use super::{Client, failure};
use wire::{Read, Reads, Style};

const GET_DHCP: Read = Read::new(
    "network.GetDHCPOptions",
    "v2/{}/dhcp_option/{}",
    Style::Plain,
);
const LIST_DHCP: Read = Read::new("network.ListDHCPOptions", "v2/{}/dhcp_option", Style::Plain);

#[expect(
    clippy::result_large_err,
    reason = "SDK failures are returned by value"
)]
impl Client {
    /// Reads a DHCP options set by ID.
    pub async fn get_dhcp_options(
        &self,
        ctx: &RequestContext,
        input: GetDhcpOptionsInput,
    ) -> Result<GetDhcpOptionsOutput, Failure<GetDhcpOptionsOutput>> {
        crate::wire::product::exit::finish!(async {
            GET_DHCP
                .path("DHCPOptionsID", &input.dhcp_options_id)
                .map_err(failure)?;
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let options = reads
                .get(GET_DHCP, &[&input.dhcp_options_id])
                .await
                .map_err(failure)?;
            let dhcp_options = reads.sanitize(GET_DHCP, options).map_err(failure)?;
            Ok(GetDhcpOptionsOutput { dhcp_options })
        })
    }

    /// Reads a page of DHCP options sets, with an optional name filter.
    pub async fn list_dhcp_options(
        &self,
        ctx: &RequestContext,
        input: ListDhcpOptionsInput,
    ) -> Result<ListDhcpOptionsOutput, Failure<ListDhcpOptionsOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut reads = Reads::new(self, ctx).map_err(failure)?;
            let page = reads
                .page(LIST_DHCP, &input.name, input.page, input.size)
                .await
                .map_err(failure)?;
            reads.sanitize_page(LIST_DHCP, page).map_err(failure)
        })
    }
}
