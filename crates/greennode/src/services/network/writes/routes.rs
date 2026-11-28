use super::*;

fn create_route_table<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::CreateRouteTableInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .create_route_table(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const CREATE_ROUTE_TABLE: Operation = op!(
   "create-route-table", "network.CreateRouteTable", create_route_table, sdk::CreateRouteTableInput, Write, "Create route table",
   "greennode network create-route-table --vpc-id <vpc-id> --name <name>",
   &["Confirmed live: the new table reaches ACTIVE about 5 seconds after the create response, with no routes. A duplicate --name is refused with the server's own message at status 400. If --vpc-id names a VPC with no main route table yet, the new table becomes it; see delete-route-table for what that means for a later delete."],
   None, &[], || sdk::CreateRouteTableInput::new("", ""), create_route_table, [
       VPC_ID => field!("VPCID", VPC_ID, vpc_id, Requirement::Nonzero(|input| !input.vpc_id.is_empty()), string_json, string_flag),
       NAME => field!("Name", NAME, name, Requirement::Nonzero(|input| !input.name.is_empty()), string_json, string_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);

fn delete_route_table<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::DeleteRouteTableInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .delete_route_table(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const DELETE_ROUTE_TABLE: Operation = op!(
   "delete-route-table", "network.DeleteRouteTable", delete_route_table, sdk::DeleteRouteTableInput, DestructiveWrite, "Delete route table",
   "greennode network delete-route-table --route-table-id <route-table-id> --yes",
   &["Reads the table, its VPC, and every subnet of that VPC first. Refuses, before any write, with error code ResourceInUse when a subnet names this table, and with DefaultResource when this is the VPC's main route table and some subnet names no table at all, so it relies on this one; a main table no subnet relies on, including one with no subnets at all, deletes normally. Confirmed live: the DELETE settles on a 404 read about 5 seconds later. A repeat delete of an already-deleted table also returns NotFound."],
   None, &[], || sdk::DeleteRouteTableInput::new(""), delete_route_table, [
       ROUTE_TABLE_ID => field!("RouteTableID", ROUTE_TABLE_ID, route_table_id, Requirement::Nonzero(|input| !input.route_table_id.is_empty()), string_json, string_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);

fn add_route<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::AddRouteInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config).add_route(ctx, input.clone()).await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const ADD_ROUTE: Operation = op!(
   "add-route", "network.AddRoute", add_route, sdk::AddRouteInput, Write, "Add route",
   "greennode network add-route --route-table-id <route-table-id> --destination-cidr <destination-cidr> --target <target> --yes",
   &["Needs --yes on every call: add-route changes routing for every server behind the table, and the CLI cannot tell cheaply whether that table is in use. Waits for the table to reach ACTIVE before sending; past that wait, error code ResourceBusy, nothing sent. Matches --destination-cidr as a parsed prefix, not by its exact text. Adding a route to --destination-cidr that is already there with the same --target is a no-op: Changed is false and nothing is sent. The same --destination-cidr with a different --target already there is refused with InvalidUsage, naming the current target; remove-route the old one first. Reads the table again right before sending and refuses with ResourceBusy, nothing sent, if its routes changed since that first read; a write that lands in the moment between this re-read and the send can still be overwritten. Without --no-wait, waits again after sending and confirms that a fresh read names exactly the routes just sent, which only catches a write that lands after this command's own send, not one from before it; a mismatch there is NotSettled.", "Sends one replacement PUT without retries, authentication replay, or redirects. An ambiguous send reconciles by reading and remains NotSettled; read the table before another write.", "All requests share one credential history. Responses suppress captures; outputs and errors are checked against every observed operation credential."],
   Some(|_, matches| confirm(matches, "network add-route changes routing for every server behind this table; pass --yes to confirm")), &[], || sdk::AddRouteInput::new("", "", ""), add_route, [
       ROUTE_TABLE_ID => field!("RouteTableID", ROUTE_TABLE_ID, route_table_id, Requirement::Nonzero(|input| !input.route_table_id.is_empty()), string_json, string_flag),
       DESTINATION_CIDR => field!("DestinationCIDR", DESTINATION_CIDR, destination_cidr, Requirement::Nonzero(|input| !input.destination_cidr.is_empty()), string_json, string_flag),
       TARGET => field!("Target", TARGET, target, Requirement::Nonzero(|input| !input.target.is_empty()), string_json, string_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);

fn remove_route<'a>(
    ctx: &'a RequestContext,
    config: &'a Config,
    input: &'a sdk::RemoveRouteInput,
) -> FutureResult<'a, ExecutionResult> {
    Box::pin(async move {
        dispatch::project_result(
            sdk::Client::new(config)
                .remove_route(ctx, input.clone())
                .await,
            std::convert::identity,
        )
    })
}

pub(in crate::services::network) const REMOVE_ROUTE: Operation = op!(
   "remove-route", "network.RemoveRoute", remove_route, sdk::RemoveRouteInput, Write, "Remove route",
   "greennode network remove-route --route-table-id <route-table-id> --destination-cidr <destination-cidr> --yes",
   &["Needs --yes on every call: remove-route changes routing for every server behind the table, and the CLI cannot tell cheaply whether that table is in use. Waits for the table to reach ACTIVE before sending; past that wait, error code ResourceBusy, nothing sent. Matches --destination-cidr as a parsed prefix, not by its exact text. Removing a --destination-cidr the table does not have returns NotFound, nothing sent. A --destination-cidr matching more than one route by its parsed prefix is refused, nothing sent. Reads the table again right before sending and refuses with ResourceBusy, nothing sent, if its routes changed since that first read; a write that lands in the moment between this re-read and the send can still be overwritten. Without --no-wait, waits again after sending and confirms that a fresh read names exactly the routes just sent, which only catches a write that lands after this command's own send, not one from before it; a mismatch there is NotSettled.", "Sends one replacement PUT without retries, authentication replay, or redirects. An ambiguous send reconciles by reading and remains NotSettled; read the table before another write.", "All requests share one credential history. Responses suppress captures; outputs and errors are checked against every observed operation credential."],
   Some(|_, matches| confirm(matches, "network remove-route changes routing for every server behind this table; pass --yes to confirm")), &[], || sdk::RemoveRouteInput::new("", ""), remove_route, [
       ROUTE_TABLE_ID => field!("RouteTableID", ROUTE_TABLE_ID, route_table_id, Requirement::Nonzero(|input| !input.route_table_id.is_empty()), string_json, string_flag),
       DESTINATION_CIDR => field!("DestinationCIDR", DESTINATION_CIDR, destination_cidr, Requirement::Nonzero(|input| !input.destination_cidr.is_empty()), string_json, string_flag),
       NO_WAIT => field!("NoWait", NO_WAIT, no_wait, Requirement::Optional, bool_json, bool_flag),
   ]
);
