use clap::ArgMatches;
use greennode_sdk::globalloadbalancer as sdk;

use crate::cli::dispatch;
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

const fn flag(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}

const NAME: Field = flag("name", "string", false);
const OFFSET: Field = flag("offset", "int", false);
const LIMIT: Field = flag("limit", "int", false);
const LOAD_BALANCER_ID: Field = flag("load-balancer-id", "string", true);
const POOL_ID: Field = flag("pool-id", "string", true);
const POOL_MEMBER_ID: Field = flag("pool-member-id", "string", true);
const LISTENER_ID: Field = flag("listener-id", "string", true);
const FROM: Field = flag("from", "string", false);
const TO: Field = flag("to", "string", false);
const TYPE: Field = flag("type", "string", false);

fn check_input_source(_: &ArgMatches, json: Option<&str>) -> Result<(), InputError> {
    if json == Some("file://") {
        return Err(InputError("--cli-input-json: cannot open file"));
    }
    Ok(())
}

macro_rules! string_field {
    ($input:ty, $field:ident, $flag:ident, $name:literal, $requirement:expr) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut $input, value| {
                let serde_json::Value::String(value) = value else {
                    return Err(());
                };
                input.$field = value;
                Ok(())
            },
            set_flag: Some(|input: &mut $input, matches| {
                input.$field = matches.get_one::<String>($flag.name).ok_or(())?.clone();
                Ok(())
            }),
            requirement: $requirement,
        }
    };
}

macro_rules! required_id {
    ($input:ty, $field:ident, $flag:ident, $name:literal) => {
        string_field!(
            $input,
            $field,
            $flag,
            $name,
            Requirement::Nonzero(|input| !input.$field.is_empty())
        )
    };
}

macro_rules! int_field {
    ($field:ident, $flag:ident, $name:literal) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut sdk::ListLoadBalancersInput, value| {
                input.$field = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input: &mut sdk::ListLoadBalancersInput, matches| {
                input.$field = *matches.get_one::<i64>($flag.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! operation {
    ($input:ident, $defaults:expr, $method:ident, $name:literal, $go:literal,
     $help:literal, $notes:expr, [$($flag:ident => $field:expr),* $(,)?]) => {
        operation!($input, $defaults, $method, $name, $go, $help, $notes,
                   [$($flag => $field),*], Kind::Read, None, None)
    };
    ($input:ident, $defaults:expr, $method:ident, $name:literal, $go:literal,
     $help:literal, $notes:expr, [$($flag:ident => $field:expr),* $(,)?],
     $kind:expr, $guard:expr, $example:expr) => {{
        const INPUT: InputDescriptor<sdk::$input> = InputDescriptor {
            defaults: $defaults,
            fields: &[$($field),*],
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: Some(check_input_source),
            guard: $guard,
        };
        Operation {
            name: $name,
            sdk_operation: concat!("globalloadbalancer.", $go),
            short_help: $help,
            example: $example,
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: $kind,
            flags: &[$($flag),*],
            json_fields: &[],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
                Box::pin(dispatch::management(
                    invocation, operation, matches, &INPUT,
                    |ctx, config, input| Box::pin(async move {
                        let result = sdk::Client::new(config).$method(ctx, input.clone()).await;
                        dispatch::project_result(result, std::convert::identity)
                    }),
                ))
            })),
        }
    }};
}

mod writes;

const SHAPE_NOTE: &str =
    "The response shape comes from the Go SDK and has no populated live read fixture.";

pub(super) const SERVICE: Service = Service {
    name: "globalloadbalancer",
    error_rules: &[],
    operations: &[
        writes::CREATE,
        writes::DELETE,
        operation!(
            ListPackagesInput,
            sdk::ListPackagesInput::default,
            list_packages,
            "list-packages",
            "ListPackages",
            "Read global load balancer packages",
            &[],
            []
        ),
        operation!(
            ListRegionsInput,
            sdk::ListRegionsInput::default,
            list_regions,
            "list-regions",
            "ListRegions",
            "Read global load balancer regions",
            &[],
            []
        ),
        operation!(
            ListLoadBalancersInput, sdk::ListLoadBalancersInput::default, list_load_balancers,
            "list-load-balancers", "ListLoadBalancers", "Read global load balancers", &[], [
                NAME => string_field!(sdk::ListLoadBalancersInput, name, NAME, "Name", Requirement::Optional),
                OFFSET => int_field!(offset, OFFSET, "Offset"),
                LIMIT => int_field!(limit, LIMIT, "Limit"),
            ]
        ),
        operation!(
            GetLoadBalancerInput, || sdk::GetLoadBalancerInput::new(""), get_load_balancer,
            "get-load-balancer", "GetLoadBalancer", "Read a global load balancer", &[SHAPE_NOTE], [
                LOAD_BALANCER_ID => required_id!(sdk::GetLoadBalancerInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
            ]
        ),
        operation!(
            ListPoolsInput, || sdk::ListPoolsInput::new(""), list_pools,
            "list-pools", "ListPools", "Read global load balancer pools", &[SHAPE_NOTE], [
                LOAD_BALANCER_ID => required_id!(sdk::ListPoolsInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
            ]
        ),
        operation!(
            ListListenersInput, || sdk::ListListenersInput::new(""), list_listeners,
            "list-listeners", "ListListeners", "Read global load balancer listeners", &[SHAPE_NOTE], [
                LOAD_BALANCER_ID => required_id!(sdk::ListListenersInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
            ]
        ),
        operation!(
            GetListenerInput, || sdk::GetListenerInput::new("", ""), get_listener,
            "get-listener", "GetListener", "Read a global load balancer listener", &[SHAPE_NOTE], [
                LOAD_BALANCER_ID => required_id!(sdk::GetListenerInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                LISTENER_ID => required_id!(sdk::GetListenerInput, listener_id, LISTENER_ID, "ListenerID"),
            ]
        ),
        operation!(
            ListPoolMembersInput, || sdk::ListPoolMembersInput::new("", ""), list_pool_members,
            "list-pool-members", "ListPoolMembers", "Read global load balancer pool member groups", &[SHAPE_NOTE], [
                LOAD_BALANCER_ID => required_id!(sdk::ListPoolMembersInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                POOL_ID => required_id!(sdk::ListPoolMembersInput, pool_id, POOL_ID, "PoolID"),
            ]
        ),
        operation!(
            GetPoolMemberInput, || sdk::GetPoolMemberInput::new("", "", ""), get_pool_member,
            "get-pool-member", "GetPoolMember", "Read a global load balancer pool member group", &[SHAPE_NOTE], [
                LOAD_BALANCER_ID => required_id!(sdk::GetPoolMemberInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                POOL_ID => required_id!(sdk::GetPoolMemberInput, pool_id, POOL_ID, "PoolID"),
                POOL_MEMBER_ID => required_id!(sdk::GetPoolMemberInput, pool_member_id, POOL_MEMBER_ID, "PoolMemberID"),
            ]
        ),
        operation!(
            ListUsageHistoriesInput, || sdk::ListUsageHistoriesInput::new(""), list_usage_histories,
            "list-usage-histories", "ListUsageHistories", "Read global load balancer usage history",
            &[SHAPE_NOTE, "The CLI passes `--from`, `--to`, and `--type` through unchecked because their formats and allowed values are unverified."], [
                LOAD_BALANCER_ID => required_id!(sdk::ListUsageHistoriesInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                FROM => string_field!(sdk::ListUsageHistoriesInput, from, FROM, "From", Requirement::Optional),
                TO => string_field!(sdk::ListUsageHistoriesInput, to, TO, "To", Requirement::Optional),
                TYPE => string_field!(sdk::ListUsageHistoriesInput, r#type, TYPE, "Type", Requirement::Optional),
            ]
        ),
    ],
};
