use super::pool_writes::option_field;
use super::*;
use serde_json::Value;

const ADDRESS: Field = flag("address", "string", true);
const PORT: Field = flag("port", "int", true);
const WEIGHT: Field = flag("weight", "int", false);
const MONITOR_PORT: Field = flag("monitor-port", "int", false);
const BACKUP: Field = flag("backup", "bool", false);
const UPDATE_NAME: Field = flag("name", "*string", false);
const UPDATE_WEIGHT: Field = flag("weight", "*int", false);
const UPDATE_MONITOR_PORT: Field = flag("monitor-port", "*int", false);
const UPDATE_BACKUP: Field = flag("backup", "*bool", false);

fn confirm_remove(_: &sdk::RemovePoolMemberInput, matches: &ArgMatches) -> Result<(), InputError> {
    if matches.get_one::<bool>("yes").copied().unwrap_or(false) {
        Ok(())
    } else {
        Err(InputError(
            "remove-pool-member stops the member taking traffic at once; pass --yes to confirm",
        ))
    }
}

pub(super) const ADD: Operation = operation!(
    @kind Kind::Write,
    AddPoolMemberInput, add_pool_member, "add-pool-member", "AddPoolMember", "Add a load balancer pool member",
    "greennode loadbalancer add-pool-member --load-balancer-id <load-balancer-id> --pool-id <pool-id> --address <address> --port <port>",
    &["Reads the pool and its member list before replacing membership. An identical member returns `Changed: false`. A zero weight uses 1, and a zero monitor port uses the traffic port.", "Use `--no-wait` to return submitted values after acceptance."], [
        LOAD_BALANCER_ID => required_id!(sdk::AddPoolMemberInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        POOL_ID => required_id!(sdk::AddPoolMemberInput, pool_id, POOL_ID, "PoolID"),
        ADDRESS => required_id!(sdk::AddPoolMemberInput, address, ADDRESS, "Address"),
        PORT => InputField { requirement: Requirement::Nonzero(|input: &sdk::AddPoolMemberInput| input.port != 0), ..int_field!(port, PORT, "Port") },
        NAME => string_field!(sdk::AddPoolMemberInput, name, NAME, "Name", Requirement::Optional),
        WEIGHT => int_field!(weight, WEIGHT, "Weight"),
        MONITOR_PORT => int_field!(monitor_port, MONITOR_PORT, "MonitorPort"),
        BACKUP => optional_bool!(backup, BACKUP, "Backup"),
        NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
    ]
);

pub(super) const UPDATE: Operation = operation!(
    @kind Kind::Write,
    UpdatePoolMemberInput, update_pool_member, "update-pool-member", "UpdatePoolMember", "Update a load balancer pool member",
    "greennode loadbalancer update-pool-member --load-balancer-id <load-balancer-id> --pool-id <pool-id> --address <address> --port <port>",
    &["Updates the selected address and port while preserving other members. Supply at least one field. A zero weight uses 1, and a zero monitor port uses the traffic port.", "Use `--no-wait` to return submitted values after acceptance."], [
        LOAD_BALANCER_ID => required_id!(sdk::UpdatePoolMemberInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        POOL_ID => required_id!(sdk::UpdatePoolMemberInput, pool_id, POOL_ID, "PoolID"),
        ADDRESS => required_id!(sdk::UpdatePoolMemberInput, address, ADDRESS, "Address"),
        PORT => InputField { requirement: Requirement::Nonzero(|input: &sdk::UpdatePoolMemberInput| input.port != 0), ..int_field!(port, PORT, "Port") },
        UPDATE_NAME => option_field!(name, UPDATE_NAME, "Name", String, |value: Value| value.as_str().map(str::to_owned)),
        UPDATE_WEIGHT => option_field!(weight, UPDATE_WEIGHT, "Weight", i64, |value: Value| value.as_i64()),
        UPDATE_MONITOR_PORT => option_field!(monitor_port, UPDATE_MONITOR_PORT, "MonitorPort", i64, |value: Value| value.as_i64()),
        UPDATE_BACKUP => option_field!(backup, UPDATE_BACKUP, "Backup", bool, |value: Value| value.as_bool()),
        NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
    ]
);

pub(super) const REMOVE: Operation = operation!(
    @kind Kind::Write,
    RemovePoolMemberInput, remove_pool_member, "remove-pool-member", "RemovePoolMember", "Remove a load balancer pool member",
    "greennode loadbalancer remove-pool-member --load-balancer-id <load-balancer-id> --pool-id <pool-id> --address <address> --port <port> --yes",
    &["Removes the member selected by address and port. Use `--yes` to confirm. The remaining members are kept.", "Use `--no-wait` to return submitted values after acceptance."], [
        LOAD_BALANCER_ID => required_id!(sdk::RemovePoolMemberInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        POOL_ID => required_id!(sdk::RemovePoolMemberInput, pool_id, POOL_ID, "PoolID"),
        ADDRESS => required_id!(sdk::RemovePoolMemberInput, address, ADDRESS, "Address"),
        PORT => InputField { requirement: Requirement::Nonzero(|input: &sdk::RemovePoolMemberInput| input.port != 0), ..int_field!(port, PORT, "Port") },
        NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
    ], guard: confirm_remove
);
