use super::*;

fn create_dhcp_options<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::CreateDhcpOptionsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .create_dhcp_options(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const CREATE_DHCP_OPTIONS: Operation = op!(
   "create-dhcp-options", "network.CreateDHCPOptions", create_dhcp_options, sdk::CreateDhcpOptionsInput, Write, "Create DHCP options",
   "greennode network create-dhcp-options --name <name> --dns-servers <dns-servers>",
   &["--dns-servers must be given at least once, each an IPv4 address; the four-address limit stays on the server. Name must not start with dhcp-option-dns-, reserved for the set VPC Private DNS creates; either problem is refused with InvalidUsage before any request. MTU is sent only when set; the server's own default is 1450. Never retried after a failure that may have already reached the server; list-dhcp-options --name and match the name exactly before creating it again rather than retrying blind."],
   None, &[], || sdk::CreateDhcpOptionsInput::new("", Vec::new()), create_dhcp_options, [
       NAME => field!("Name", NAME, name, Requirement::Nonzero(|input| !input.name.is_empty()), string_json, string_flag),
       DNS_SERVERS => field!("DNSServers", DNS_SERVERS, dns_servers, Requirement::Presence, |value| serde_json::from_value(value).map_err(|_| ()), list_flag),
       MTU => field!("MTU", MTU, mtu, Requirement::Optional, |value| int_json(value).map(Some), |matches, name| int_flag(matches, name).map(Some)),
   ]
);

fn delete_dhcp_options<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::DeleteDhcpOptionsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .delete_dhcp_options(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const DELETE_DHCP_OPTIONS: Operation = op!(
   "delete-dhcp-options", "network.DeleteDHCPOptions", delete_dhcp_options, sdk::DeleteDhcpOptionsInput, DestructiveWrite, "Delete DHCP options",
   "greennode network delete-dhcp-options --dhcp-options-id <dhcp-options-id> --yes",
   &["Refuses, before any write, with error code ResourceInUse, a set still attached to any VPC, naming them; detach it from every VPC first. A set left behind unattached by a deleted Private DNS VPC deletes like any other set. A repeat delete of an already-deleted set returns NotFound."],
   None, &[], || sdk::DeleteDhcpOptionsInput::new(""), delete_dhcp_options, [
       DHCP_OPTIONS_ID => field!("DHCPOptionsID", DHCP_OPTIONS_ID, dhcp_options_id, Requirement::Nonzero(|input| !input.dhcp_options_id.is_empty()), string_json, string_flag),
   ]
);

fn set_vpc_dhcp_options<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::SetVpcDhcpOptionsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .set_vpc_dhcp_options(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const SET_VPC_DHCP_OPTIONS: Operation = op!(
   "set-vpc-dhcp-options", "network.SetVPCDHCPOptions", set_vpc_dhcp_options, sdk::SetVpcDhcpOptionsInput, Write, "Set VPC DHCP options",
   "greennode network set-vpc-dhcp-options --vpc-id <vpc-id> --dhcp-options-id <dhcp-options-id> --yes",
   &["Needs --yes on every call: moving a VPC's set changes DNS for every server already in it, which only picks up the new resolvers after its own next DHCP renew or reboot. Running the command again with the VPC's previous set id restores it while that set still exists; clear-vpc-dhcp-options moves the VPC to no set instead. Refuses, before any write, with error code DefaultResource, a VPC whose Private DNS is enabled or whose current set already is one Private DNS created, or a target set that is itself one Private DNS created: replacing it would cut every server in the VPC off from its private zone lookups, and there is no call to put it back. Refuses, before any write, with error code ResourceBusy, a target set that is not yet ACTIVE. Setting the VPC's current set again is a no-op: Changed is false and nothing is sent, even though --yes is still required. Once the PATCH is sent, waits up to 60 seconds for a read to show the new set: a VPC that reaches ERROR is WriteFailed, and the wait running out is NotSettled, but either way the PATCH already landed and is safe to send again with the same --dhcp-options-id."],
   Some(|_, matches| confirm(matches, "network set-vpc-dhcp-options moves the VPC to a DHCP options set and changes DNS for every server in it; pass --yes to confirm")), &[], || sdk::SetVpcDhcpOptionsInput::new("", ""), set_vpc_dhcp_options, [
       VPC_ID => field!("VPCID", VPC_ID, vpc_id, Requirement::Nonzero(|input| !input.vpc_id.is_empty()), string_json, string_flag),
       DHCP_OPTIONS_ID => field!("DHCPOptionsID", DHCP_OPTIONS_ID, dhcp_options_id, Requirement::Nonzero(|input| !input.dhcp_options_id.is_empty()), string_json, string_flag),
   ]
);

fn clear_vpc_dhcp_options<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::ClearVpcDhcpOptionsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .clear_vpc_dhcp_options(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const CLEAR_VPC_DHCP_OPTIONS: Operation = op!(
   "clear-vpc-dhcp-options", "network.ClearVPCDHCPOptions", clear_vpc_dhcp_options, sdk::ClearVpcDhcpOptionsInput, Write, "Clear VPC DHCP options",
   "greennode network clear-vpc-dhcp-options --vpc-id <vpc-id> --yes",
   &["Needs --yes on every call: clearing a VPC's set changes DNS for every server already in it, which only picks up the change after its own next DHCP renew or reboot. Running set-vpc-dhcp-options with the VPC's previous set id restores it while that set still exists. Refuses, before any write, with error code DefaultResource, a VPC whose Private DNS is enabled or whose current set already is one Private DNS created: clearing it would cut every server in the VPC off from its private zone lookups, and there is no call to put it back. Clearing a VPC that already has no set is a no-op: Changed is false and nothing is sent, even though --yes is still required. Once the PATCH is sent, waits up to 60 seconds for a read to show an empty set: a VPC that reaches ERROR is WriteFailed, and the wait running out is NotSettled, but either way the PATCH already landed and is safe to send again. What DNS a VPC with no set uses is not confirmed."],
   Some(|_, matches| confirm(matches, "network clear-vpc-dhcp-options removes the VPC's DHCP options set and changes DNS for every server in it; pass --yes to confirm")), &[], || sdk::ClearVpcDhcpOptionsInput::new(""), clear_vpc_dhcp_options, [
       VPC_ID => field!("VPCID", VPC_ID, vpc_id, Requirement::Nonzero(|input| !input.vpc_id.is_empty()), string_json, string_flag),
   ]
);
