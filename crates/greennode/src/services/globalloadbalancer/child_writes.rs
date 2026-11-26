use super::*;
use crate::cli::op::InheritedBinding;

const WRITE_NAME: Field = flag("name", "string", true);
const MEMBER_NAME: Field = flag("member-name", "string", true);
const SERVER_ID: Field = flag("backend-server-id", "string", true);
const VPC_ID: Field = flag("backend-vpc-id", "string", true);
const SUBNET_ID: Field = flag("backend-subnet-id", "string", true);
const BACKEND_PORT: Field = flag("backend-port", "int", true);
const LISTENER_PORT: Field = flag("port", "int", true);
const CIDRS: Field = flag("allowed-cidrs", "[]string", true);
const PROJECT_INPUT: &str = "BackendProjectID";
const PROJECT: InheritedBinding = InheritedBinding {
    flag: flag("project-id", "string", true),
    input_field: PROJECT_INPUT,
};

const CATALOG: &str = "Each write checks the configured region and the load balancer's package against the account catalogs. SDK IAM-user login and default GLB, vServer, and auth endpoints are required.";
const BILLING: &str = "No quote is available and the request carries no price or payment field. GreenNode bills child usage under the account's terms. The SDK neither selects nor verifies the payment source. Deleting does not promise a refund.";
const SETTLEMENT: &str = "The SDK sends one create and confirms by read-back. On `NotSettled` or `WriteFailed`, partial output holds only known IDs. Inspect inventory and billing before another mutation.";

fn confirm(matches: &ArgMatches, message: &'static str) -> Result<(), InputError> {
    if matches.get_one::<bool>("yes") == Some(&true) {
        Ok(())
    } else {
        Err(InputError(message))
    }
}

macro_rules! port {
    ($ty:ty, $name:literal, $flag:ident, $field:ident) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut $ty, value| {
                input.$field = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input: &mut $ty, matches| {
                input.$field = *matches.get_one::<i64>($flag.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Nonzero(|input| input.$field != 0),
        }
    };
}

macro_rules! project {
    ($ty:ty) => {
        InputField {
            json_name: PROJECT_INPUT,
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input: &mut $ty, value| {
                input.backend_project_id = value.as_str().ok_or(())?.into();
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Nonzero(|input| !input.backend_project_id.is_empty()),
        }
    };
}

const ALLOWED_CIDRS: InputField<sdk::CreateListenerInput> = InputField {
    json_name: "AllowedCIDRs",
    flag: Some(CIDRS),
    secret: false,
    nullable: true,
    set_json: |input, value| {
        input.allowed_cidrs = if value.is_null() {
            Vec::new()
        } else {
            serde_json::from_value(value).map_err(|_| ())?
        };
        Ok(())
    },
    set_flag: Some(|input, matches| {
        input.allowed_cidrs = matches
            .get_many::<String>(CIDRS.name)
            .ok_or(())?
            .cloned()
            .collect();
        Ok(())
    }),
    requirement: Requirement::Nonzero(|input| !input.allowed_cidrs.is_empty()),
};

pub(super) const CREATE_POOL: Operation = operation!(
    CreatePoolInput, || sdk::CreatePoolInput::new("", "", "", "", "", "", "", 0), create_pool,
    "create-pool", "CreatePool", "Create a TCP pool with one private member group and one vServer backend.",
    &[CATALOG, "Requires `--yes`. Create the pool before `create-listener`. The load balancer must have no pool or listener. The backend is the server's private fixed IP, verified against the VPC and subnet under `--project-id`.", BILLING, "The pool and monitor must become `ACTIVE`. Member health is reported. A new backend can be `OFFLINE` or `UNHEALTHY`.", SETTLEMENT], [
        LOAD_BALANCER_ID => required_id!(sdk::CreatePoolInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        WRITE_NAME => required_id!(sdk::CreatePoolInput, name, WRITE_NAME, "Name"),
        MEMBER_NAME => required_id!(sdk::CreatePoolInput, member_name, MEMBER_NAME, "MemberName"),
        SERVER_ID => required_id!(sdk::CreatePoolInput, backend_server_id, SERVER_ID, "BackendServerID"),
        VPC_ID => required_id!(sdk::CreatePoolInput, backend_vpc_id, VPC_ID, "BackendVPCID"),
        SUBNET_ID => required_id!(sdk::CreatePoolInput, backend_subnet_id, SUBNET_ID, "BackendSubnetID"),
        BACKEND_PORT => port!(sdk::CreatePoolInput, "BackendPort", BACKEND_PORT, backend_port),
    ], Kind::Write, Some(|_, matches| confirm(matches, "create-pool requires --yes to confirm")),
    Some("greennode globalloadbalancer create-pool --load-balancer-id <glb-id> --name <name> --member-name <member-name> --backend-server-id <server-id> --backend-vpc-id <vpc-id> --backend-subnet-id <subnet-id> --backend-port <port> --project-id <project-id> --yes"),
    Some((PROJECT_INPUT, |input, value| input.backend_project_id = value.into())), &[PROJECT], [project!(sdk::CreatePoolInput)]
);

pub(super) const CREATE_LISTENER: Operation = operation!(
    CreateListenerInput, || sdk::CreateListenerInput::new("", "", "", 0, Vec::new(), "", ""), create_listener,
    "create-listener", "CreateListener", "Create a TCP listener bound to the pool, limited to the allowed source CIDRs.",
    &[CATALOG, "Requires `--yes`. Run after `create-pool`. The only pool and its private backend must be settled, and the load balancer must have no listener.", "Repeat `--allowed-cidrs` for canonical IPv4 networks from `/8` through `/32`. Host bits, duplicates, and collectively unrestricted sources are refused. Use `/32` for one known source. Read-back verifies the CIDR configuration. Provider enforcement remains separate.", BILLING, SETTLEMENT], [
        LOAD_BALANCER_ID => required_id!(sdk::CreateListenerInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        WRITE_NAME => required_id!(sdk::CreateListenerInput, name, WRITE_NAME, "Name"),
        POOL_ID => required_id!(sdk::CreateListenerInput, pool_id, POOL_ID, "PoolID"),
        LISTENER_PORT => port!(sdk::CreateListenerInput, "Port", LISTENER_PORT, port),
        CIDRS => ALLOWED_CIDRS,
        SERVER_ID => required_id!(sdk::CreateListenerInput, backend_server_id, SERVER_ID, "BackendServerID"),
    ], Kind::Write, Some(|_, matches| confirm(matches, "create-listener requires --yes to confirm")),
    Some("greennode globalloadbalancer create-listener --load-balancer-id <glb-id> --name <name> --pool-id <pool-id> --port <port> --allowed-cidrs <cidr> --backend-server-id <server-id> --project-id <project-id> --yes"),
    Some((PROJECT_INPUT, |input, value| input.backend_project_id = value.into())), &[PROJECT], [project!(sdk::CreateListenerInput)]
);

pub(super) const DELETE_LISTENER: Operation = operation!(
    DeleteListenerInput, || sdk::DeleteListenerInput::new("", ""), delete_listener,
    "delete-listener", "DeleteListener", "Delete the sole TCP listener. The pool and member group stay.",
    &[CATALOG, "Requires `--yes`. Run before `delete-pool`. The SDK confirms absence from the complete listener list and checks that the load balancer, pool, and member group remain. Deleting does not promise a refund."], [
        LOAD_BALANCER_ID => required_id!(sdk::DeleteListenerInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        LISTENER_ID => required_id!(sdk::DeleteListenerInput, listener_id, LISTENER_ID, "ListenerID"),
    ], Kind::DestructiveWrite, None,
    Some("greennode globalloadbalancer delete-listener --load-balancer-id <glb-id> --listener-id <listener-id> --yes")
);

pub(super) const DELETE_POOL: Operation = operation!(
    DeletePoolInput, || sdk::DeletePoolInput::new("", ""), delete_pool,
    "delete-pool", "DeletePool", "Delete the listener-free pool with its member group and backend row. The vServer stays.",
    &[CATALOG, "Requires `--yes`. Run after `delete-listener` and before deleting the load balancer or backend server. The only pool and health monitor must be `ACTIVE`, and its private backend must report a health status.", "The SDK verifies the load balancer remains and the pool and its member group are absent. Deleting does not promise a refund."], [
        LOAD_BALANCER_ID => required_id!(sdk::DeletePoolInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        POOL_ID => required_id!(sdk::DeletePoolInput, pool_id, POOL_ID, "PoolID"),
    ], Kind::DestructiveWrite, None,
    Some("greennode globalloadbalancer delete-pool --load-balancer-id <glb-id> --pool-id <pool-id> --yes")
);
