use super::*;

fn create_vpc<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::CreateVpcInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .create_vpc(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const CREATE_VPC: Operation = op!(
   "create-vpc", "network.CreateVPC", create_vpc, sdk::CreateVpcInput, Write, "Create VPC",
   "greennode network create-vpc --name <name> --cidr <cidr>",
   &["Takes no zone: the server ignores a VPC's zone and always places it in the region's first zone, which can be disabled for the account. The zone that matters is a subnet's own --zone-id."],
   None, &[], || sdk::CreateVpcInput::new("", ""), create_vpc, [
       NAME => field!("Name", NAME, name, Requirement::Nonzero(|input| !input.name.is_empty()), string_json, string_flag),
       CIDR => field!("CIDR", CIDR, cidr, Requirement::Nonzero(|input| !input.cidr.is_empty()), string_json, string_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);

fn update_vpc<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::UpdateVpcInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .update_vpc(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const UPDATE_VPC: Operation = op!(
   "update-vpc", "network.UpdateVPC", update_vpc, sdk::UpdateVpcInput, Write, "Update VPC",
   "greennode network update-vpc --vpc-id <vpc-id> --name <name>",
   &["The SDK refuses write redirects, including a same-origin redirect of the rename request."],
   None, &[], || sdk::UpdateVpcInput::new("", ""), update_vpc, [
       VPC_ID => field!("VPCID", VPC_ID, vpc_id, Requirement::Nonzero(|input| !input.vpc_id.is_empty()), string_json, string_flag),
       NAME => field!("Name", NAME, name, Requirement::Nonzero(|input| !input.name.is_empty()), string_json, string_flag),
   ]
);

fn delete_vpc<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::DeleteVpcInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .delete_vpc(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const DELETE_VPC: Operation = op!(
   "delete-vpc", "network.DeleteVPC", delete_vpc, sdk::DeleteVpcInput, DestructiveWrite, "Delete VPC",
   "greennode network delete-vpc --vpc-id <vpc-id> --yes",
   &["Refuses, before any write, a VPC that still has a server, a volume, or a subnet; delete the subnets first. Even once every subnet is gone, the server keeps refusing the VPC's own delete with error code ResourceInUse for several minutes afterward; this command reads first every time, so a rerun once that window passes is safe."],
   None, &[], || sdk::DeleteVpcInput::new(""), delete_vpc, [
       VPC_ID => field!("VPCID", VPC_ID, vpc_id, Requirement::Nonzero(|input| !input.vpc_id.is_empty()), string_json, string_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);

fn enable_vpc_private_dns<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::EnableVpcPrivateDnsInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .enable_vpc_private_dns(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const ENABLE_VPC_PRIVATE_DNS: Operation = op!(
   "enable-vpc-private-dns", "network.EnableVPCPrivateDNS", enable_vpc_private_dns, sdk::EnableVpcPrivateDnsInput, DestructiveWrite, "Enable VPC private DNS",
   "greennode network enable-vpc-private-dns --vpc-id <vpc-id> --yes",
   &["One-way: the API has no call that disables Private DNS again, so this command needs --yes. Takes about 6 minutes to settle; a server already on the VPC only picks up the new resolver after its own DHCP renew."],
   None, &[], || sdk::EnableVpcPrivateDnsInput::new(""), enable_vpc_private_dns, [
       VPC_ID => field!("VPCID", VPC_ID, vpc_id, Requirement::Nonzero(|input| !input.vpc_id.is_empty()), string_json, string_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);
