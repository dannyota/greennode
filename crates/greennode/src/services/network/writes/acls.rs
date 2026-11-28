use super::*;

fn create_network_acl<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::CreateNetworkAclInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .create_network_acl(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const CREATE_NETWORK_ACL: Operation = op!(
   "create-network-acl", "network.CreateNetworkACL", create_network_acl, sdk::CreateNetworkAclInput, Write, "Create network ACL",
   "greennode network create-network-acl --vpc-id <vpc-id> --name <name>",
   &["Confirmed live: the new ACL is already ACTIVE in the create response and starts with an inbound and outbound pass-all rule at priority 0, ordinary and removable, plus an inbound and outbound deny-all rule at priority 2000, server-protected. A rule is default, and never removed by add-network-acl-rule or remove-network-acl-rule, when its priority is 2000 or above or it decodes System true; the priority-0 pass-all rules are neither. Whether a deny rule at a caller priority takes effect while those pass-all rules are still in the list has not been shown live; do not rely on a deny rule alone to block traffic. A duplicate --name fails with the server's own message."],
   None, &[], || sdk::CreateNetworkAclInput::new("", ""), create_network_acl, [
       VPC_ID => field!("VPCID", VPC_ID, vpc_id, Requirement::Nonzero(|input| !input.vpc_id.is_empty()), string_json, string_flag),
       NAME => field!("Name", NAME, name, Requirement::Nonzero(|input| !input.name.is_empty()), string_json, string_flag),
   ]
);

fn delete_network_acl<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::DeleteNetworkAclInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .delete_network_acl(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const DELETE_NETWORK_ACL: Operation = op!(
   "delete-network-acl", "network.DeleteNetworkACL", delete_network_acl, sdk::DeleteNetworkAclInput, DestructiveWrite, "Delete network ACL",
   "greennode network delete-network-acl --network-acl-id <network-acl-id> --yes",
   &["Refuses, before any write, with error code DefaultResource for a project's default ACL, and ResourceInUse when any subnet is still associated; disassociate every subnet first. Confirmed live: a deleted ACL's own GET returns 500, not 404, so this command, and a repeat delete, confirm through the ACL list instead of trusting that status alone; a plain 404 for an id that was never valid still returns NotFound directly. A DELETE sent while the ACL is still settling an earlier write gets the server's own busy 400 back, mapped to error code ResourceBusy rather than a plain API error; nothing changed, so the command can be run again. After a subnet associate or disassociate, a DELETE inside the ACL's own busy window (about 20-30 seconds after the change) gets a 500 back instead; nothing changed, so the command can be run again. If a 500 is returned, the command then checks the ACL list for up to 60 seconds and, if the ACL is still listed, exits with that 500 as a plain API error; wait and run the command again."],
   None, &[], || sdk::DeleteNetworkAclInput::new(""), delete_network_acl, [
       NETWORK_ACL_ID => field!("NetworkACLID", NETWORK_ACL_ID, network_acl_id, Requirement::Nonzero(|input| !input.network_acl_id.is_empty()), string_json, string_flag),
   ]
);

fn add_network_acl_rule<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::AddNetworkAclRuleInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .add_network_acl_rule(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const ADD_NETWORK_ACL_RULE: Operation = op!(
   "add-network-acl-rule", "network.AddNetworkACLRule", add_network_acl_rule, sdk::AddNetworkAclRuleInput, Write, "Add network ACL rule",
   "greennode network add-network-acl-rule --network-acl-id <network-acl-id> --direction <direction> --priority <priority> --protocol tcp --cidr <cidr> --action <action> --port-range-min 22 --port-range-max 22 --yes",
   &["Needs --yes on every call: add-network-acl-rule can pass or drop traffic for every subnet this ACL covers, and the CLI cannot tell cheaply whether the ACL is in active use. Waits for the ACL to reach ACTIVE before sending; past that wait, error code ResourceBusy, nothing sent. Resends every rule read, including the server's own default rules (priority 2000 or above, or a rule marked System), which are never sent changed. For --protocol tcp or udp, needs an explicit port or range: --port-range-min and --port-range-max must not both be left at 0, refused with InvalidUsage before any request; for every port pass --port-range-min 0 --port-range-max 65535. Protocol ANY always requires that same full range; icmp accepts it or 0 and 0 together, for every ICMP type. Adding a rule already present at the same --direction and --priority with every other field equal is a no-op: Changed is false and nothing is sent. The same --direction and --priority already there with a different field is refused with InvalidUsage; remove-network-acl-rule the old one first. Reads the ACL again right before sending and refuses with ResourceBusy, nothing sent, if its rules changed since that first read; a write that lands in the moment between this re-read and the send can still be overwritten. The rules PUT itself is sent once and never retried: landing in the ACL's own busy window (confirmed live, roughly 18 seconds after an earlier write) gets the server's own busy 400 back, mapped to ResourceBusy, and changes nothing, so it can be run again; any other failure that may already have reached the server, a 5xx, a network error, or a timeout, is NotSettled instead, and is not resent automatically, so read the ACL first before trying again. Without --no-wait, a successful send waits once more and confirms that a fresh read names exactly the rules just sent; a mismatch, such as from another writer changing the ACL at the same time, is also NotSettled."],
   Some(|_, matches| confirm(matches, "network add-network-acl-rule can change which traffic reaches a subnet; pass --yes to confirm")), &[], || sdk::AddNetworkAclRuleInput::new("", "", 0, "", "", ""), add_network_acl_rule, [
       NETWORK_ACL_ID => field!("NetworkACLID", NETWORK_ACL_ID, network_acl_id, Requirement::Nonzero(|input| !input.network_acl_id.is_empty()), string_json, string_flag),
       DIRECTION => field!("Direction", DIRECTION, direction, Requirement::Nonzero(|input| !input.direction.is_empty()), string_json, string_flag),
       PRIORITY_REQUIRED => field!("Priority", PRIORITY_REQUIRED, priority, Requirement::Nonzero(|input| input.priority != 0), int_json, int_flag),
       PROTOCOL => field!("Protocol", PROTOCOL, protocol, Requirement::Nonzero(|input| !input.protocol.is_empty()), string_json, string_flag),
       CIDR => field!("CIDR", CIDR, cidr, Requirement::Nonzero(|input| !input.cidr.is_empty()), string_json, string_flag),
       ACTION => field!("Action", ACTION, action, Requirement::Nonzero(|input| !input.action.is_empty()), string_json, string_flag),
       PORT_RANGE_MIN => field!("PortRangeMin", PORT_RANGE_MIN, port_range_min, Requirement::Optional, int_json, int_flag),
       PORT_RANGE_MAX => field!("PortRangeMax", PORT_RANGE_MAX, port_range_max, Requirement::Optional, int_json, int_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);

#[derive(Clone, Default)]
struct RemoveRuleInput {
    network_acl_id: String,
    direction: String,
    priority: Option<i64>,
    no_wait: bool,
}
fn remove_network_acl_rule<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a RemoveRuleInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .remove_network_acl_rule(ctx, {
                    let mut settings = sdk::RemoveNetworkAclRuleInput::new(
                        input.network_acl_id.as_str(),
                        input.direction.as_str(),
                        input.priority.unwrap_or_default(),
                    );
                    settings.no_wait = input.no_wait;
                    settings
                })
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const REMOVE_NETWORK_ACL_RULE: Operation = op!(
   "remove-network-acl-rule", "network.RemoveNetworkACLRule", remove_network_acl_rule, RemoveRuleInput, Write, "Remove network ACL rule",
   "greennode network remove-network-acl-rule --network-acl-id <network-acl-id> --direction <direction> --priority <priority> --yes",
   &["Needs --yes on every call: remove-network-acl-rule can pass or drop traffic for every subnet this ACL covers, and the CLI cannot tell cheaply whether the ACL is in active use. Waits for the ACL to reach ACTIVE before sending; past that wait, error code ResourceBusy, nothing sent. Resends every rule read, including the server's own default rules (priority 2000 or above, or a rule marked System), which are never sent changed. Needs --priority even to name priority 0: it carries no vngcloud:\"required\" tag on the SDK's own Input, since 0 is also the priority of the ACL's own ordinary pass-all rule, which a caller may want to remove, so this command requires the flag (or a --cli-input-json Priority, inline or file://) so a caller who simply forgot it is never mistaken for one naming that rule on purpose. Removing a --direction and --priority the ACL does not have returns NotFound, nothing sent; removing a default rule (priority 2000 or above, or a rule marked System) is refused with DefaultResource, nothing sent. Reads the ACL again right before sending and refuses with ResourceBusy, nothing sent, if its rules changed since that first read; a write that lands in the moment between this re-read and the send can still be overwritten. The rules PUT itself is sent once and never retried: landing in the ACL's own busy window (confirmed live, roughly 18 seconds after an earlier write) gets the server's own busy 400 back, mapped to ResourceBusy, and changes nothing, so it can be run again; any other failure that may already have reached the server, a 5xx, a network error, or a timeout, is NotSettled instead, and is not resent automatically, so read the ACL first before trying again. Without --no-wait, a successful send waits once more and confirms that a fresh read names exactly the rules just sent; a mismatch, such as from another writer changing the ACL at the same time, is also NotSettled."],
   Some(|input: &RemoveRuleInput, matches| { if input.priority.is_none() { return Err(InputError("--priority is required")); } confirm(matches, "network remove-network-acl-rule can change which traffic reaches a subnet; pass --yes to confirm") }), &["Priority"], RemoveRuleInput::default, remove_network_acl_rule, [
       NETWORK_ACL_ID => field!("NetworkACLID", NETWORK_ACL_ID, network_acl_id, Requirement::Nonzero(|input| !input.network_acl_id.is_empty()), string_json, string_flag),
       DIRECTION => field!("Direction", DIRECTION, direction, Requirement::Nonzero(|input| !input.direction.is_empty()), string_json, string_flag),
       PRIORITY_REQUIRED => field!("Priority", PRIORITY_REQUIRED, priority, Requirement::Presence, |value| int_json(value).map(Some), |matches, name| int_flag(matches, name).map(Some)),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);

fn associate_network_acl_subnet<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::AssociateNetworkAclSubnetInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .associate_network_acl_subnet(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const ASSOCIATE_NETWORK_ACL_SUBNET: Operation = op!(
   "associate-network-acl-subnet", "network.AssociateNetworkACLSubnet", associate_network_acl_subnet, sdk::AssociateNetworkAclSubnetInput, Write, "Associate network ACL subnet",
   "greennode network associate-network-acl-subnet --network-acl-id <network-acl-id> --subnet-id <subnet-id> --yes",
   &["Needs --yes on every call: associate-network-acl-subnet can change which ACL's rules apply to a subnet's traffic at once, and the CLI cannot tell cheaply whether the ACL is in active use. Waits for the ACL to reach ACTIVE before sending; past that wait, error code ResourceBusy, nothing sent. A subnet belongs to at most one ACL, so associating one already associated with a different ACL moves it there, and this ACL's rules apply to its traffic at once. Associating a subnet already in this ACL's list is a no-op: Changed is false and nothing is sent, including no read of the subnet itself. Otherwise reads the subnet under this ACL's own VPC first, so a subnet of a different VPC is refused with NotFound before anything is sent. The output's PreviousNetworkACLID names the ACL the subnet moved from, if any, read from the subnet just before the move; it is set only when Changed is true. Reads the ACL again right before sending and refuses with ResourceBusy, nothing sent, if its subnet list changed since that first read; a write that lands in the moment between this re-read and the send can still be overwritten. The subnets PUT itself is sent once and never retried: landing in the ACL's own busy window gets the server's own busy 400 back, mapped to ResourceBusy, and changes nothing, so it can be run again; any other failure that may already have reached the server, a 5xx, a network error, or a timeout, is NotSettled instead, and is not resent automatically, so read the ACL first before trying again. Without --no-wait, a successful send waits once more and confirms that a fresh read names exactly the subnets just sent; a mismatch, such as from another writer changing the ACL at the same time, is also NotSettled. Confirmed live, a successful call still leaves the ACL busy for about 20 more seconds, and, unlike after a rules write, the ACL's own status reads ACTIVE throughout, so nothing in a read marks the window: the very next write to this ACL, of any kind, can still get ResourceBusy during that time, which is safe to wait out and retry. See [Limitations](Limitations.md#a-network-acls-busy-window)."],
   Some(|_, matches| confirm(matches, "network associate-network-acl-subnet can change which traffic reaches a subnet; pass --yes to confirm")), &[], || sdk::AssociateNetworkAclSubnetInput::new("", ""), associate_network_acl_subnet, [
       NETWORK_ACL_ID => field!("NetworkACLID", NETWORK_ACL_ID, network_acl_id, Requirement::Nonzero(|input| !input.network_acl_id.is_empty()), string_json, string_flag),
       SUBNET_ID => field!("SubnetID", SUBNET_ID, subnet_id, Requirement::Nonzero(|input| !input.subnet_id.is_empty()), string_json, string_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);

fn disassociate_network_acl_subnet<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::DisassociateNetworkAclSubnetInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .disassociate_network_acl_subnet(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const DISASSOCIATE_NETWORK_ACL_SUBNET: Operation = op!(
   "disassociate-network-acl-subnet", "network.DisassociateNetworkACLSubnet", disassociate_network_acl_subnet, sdk::DisassociateNetworkAclSubnetInput, Write, "Disassociate network ACL subnet",
   "greennode network disassociate-network-acl-subnet --network-acl-id <network-acl-id> --subnet-id <subnet-id> --yes",
   &["Needs --yes on every call: disassociate-network-acl-subnet can change which ACL's rules apply to a subnet's traffic at once, and the CLI cannot tell cheaply whether the ACL is in active use. Waits for the ACL to reach ACTIVE before sending; past that wait, error code ResourceBusy, nothing sent. Disassociating a subnet not in this ACL's list is a no-op: Changed is false and nothing is sent. What a subnet falls back to once disassociated is not yet confirmed live. Reads the ACL again right before sending and refuses with ResourceBusy, nothing sent, if its subnet list changed since that first read; a write that lands in the moment between this re-read and the send can still be overwritten. The subnets PUT itself is sent once and never retried: landing in the ACL's own busy window gets the server's own busy 400 back, mapped to ResourceBusy, and changes nothing, so it can be run again; any other failure that may already have reached the server, a 5xx, a network error, or a timeout, is NotSettled instead, and is not resent automatically, so read the ACL first before trying again. Without --no-wait, a successful send waits once more and confirms that a fresh read names exactly the subnets just sent; a mismatch, such as from another writer changing the ACL at the same time, is also NotSettled. Confirmed live, a successful call still leaves the ACL busy for about 20 more seconds, and, unlike after a rules write, the ACL's own status reads ACTIVE throughout, so nothing in a read marks the window: the very next write to this ACL, of any kind, can still get ResourceBusy during that time, which is safe to wait out and retry. See [Limitations](Limitations.md#a-network-acls-busy-window)."],
   Some(|_, matches| confirm(matches, "network disassociate-network-acl-subnet can change which traffic reaches a subnet; pass --yes to confirm")), &[], || sdk::DisassociateNetworkAclSubnetInput::new("", ""), disassociate_network_acl_subnet, [
       NETWORK_ACL_ID => field!("NetworkACLID", NETWORK_ACL_ID, network_acl_id, Requirement::Nonzero(|input| !input.network_acl_id.is_empty()), string_json, string_flag),
       SUBNET_ID => field!("SubnetID", SUBNET_ID, subnet_id, Requirement::Nonzero(|input| !input.subnet_id.is_empty()), string_json, string_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);
