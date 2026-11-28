use super::*;

fn create_subnet<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::CreateSubnetInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .create_subnet(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const CREATE_SUBNET: Operation = op!(
   "create-subnet", "network.CreateSubnet", create_subnet, sdk::CreateSubnetInput, Write, "Create subnet",
   "greennode network create-subnet --vpc-id <vpc-id> --zone-id <zone-id> --name <name> --cidr <cidr>",
   &["--zone-id must name a zone enabled for the account; the SDK picks no default, since a guess would place the subnet, and any server later created in it, in a zone the caller did not choose. Run portal list-zones to find an enabled zone."],
   None, &[], || sdk::CreateSubnetInput::new("", "", "", ""), create_subnet, [
       VPC_ID => field!("VPCID", VPC_ID, vpc_id, Requirement::Nonzero(|input| !input.vpc_id.is_empty()), string_json, string_flag),
       ZONE_ID => field!("ZoneID", ZONE_ID, zone_id, Requirement::Nonzero(|input| !input.zone_id.is_empty()), string_json, string_flag),
       NAME => field!("Name", NAME, name, Requirement::Nonzero(|input| !input.name.is_empty()), string_json, string_flag),
       CIDR => field!("CIDR", CIDR, cidr, Requirement::Nonzero(|input| !input.cidr.is_empty()), string_json, string_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);

fn update_subnet<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::UpdateSubnetInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .update_subnet(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const UPDATE_SUBNET: Operation = op!(
   "update-subnet", "network.UpdateSubnet", update_subnet, sdk::UpdateSubnetInput, Write, "Update subnet",
   "greennode network update-subnet --vpc-id <vpc-id> --subnet-id <subnet-id> --name <name>",
   &[],
   None, &[], || sdk::UpdateSubnetInput::new("", "", ""), update_subnet, [
       VPC_ID => field!("VPCID", VPC_ID, vpc_id, Requirement::Nonzero(|input| !input.vpc_id.is_empty()), string_json, string_flag),
       SUBNET_ID => field!("SubnetID", SUBNET_ID, subnet_id, Requirement::Nonzero(|input| !input.subnet_id.is_empty()), string_json, string_flag),
       NAME => field!("Name", NAME, name, Requirement::Nonzero(|input| !input.name.is_empty()), string_json, string_flag),
   ]
);

fn delete_subnet<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::DeleteSubnetInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .delete_subnet(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const DELETE_SUBNET: Operation = op!(
   "delete-subnet", "network.DeleteSubnet", delete_subnet, sdk::DeleteSubnetInput, DestructiveWrite, "Delete subnet",
   "greennode network delete-subnet --vpc-id <vpc-id> --subnet-id <subnet-id> --yes",
   &["Refuses, before any write, with error code ResourceInUse when a network ACL in the subnet's own VPC still lists it; disassociate the subnet from that ACL first. Also refuses with error code InvalidInput if the subnet read names a VPC other than --vpc-id. After disassociating the subnet from a network ACL, wait about 30 seconds before deleting it, since this command cannot see the ACL's own busy window.", "ACL associations require explicit, nonempty, valid, unique subnet IDs. Null or invalid association elements cannot authorize subnet deletion."],
   None, &[], || sdk::DeleteSubnetInput::new("", ""), delete_subnet, [
       VPC_ID => field!("VPCID", VPC_ID, vpc_id, Requirement::Nonzero(|input| !input.vpc_id.is_empty()), string_json, string_flag),
       SUBNET_ID => field!("SubnetID", SUBNET_ID, subnet_id, Requirement::Nonzero(|input| !input.subnet_id.is_empty()), string_json, string_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);
