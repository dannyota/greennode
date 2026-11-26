use clap::ArgMatches;
use greennode_sdk::loadbalancer as sdk;

use crate::cli::dispatch::{self, OutputView};
use crate::cli::errors::{Rule, RuleName, RuleOperation};
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation, Service};

const fn flag(name: &'static str, go_type: &'static str, required: bool) -> Field {
    Field {
        name,
        go_type,
        required,
    }
}

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
            set_json: |input: &mut _, value| {
                input.$field = value.as_i64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input: &mut _, matches| {
                input.$field = *matches.get_one::<i64>($flag.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! json_field {
    ($name:literal, $accessor:ident, $field:ident) => {
        InputField {
            json_name: $name,
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input: &mut _, value| {
                input.$field = value.$accessor().ok_or(())?;
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! operation {
    ($input:ident, $method:ident, $name:literal, $go:literal, $help:literal, $example:literal, $notes:expr, $flags:tt $(, $json:tt)?) => {
        operation!(@kind Kind::Read, $input, $method, $name, $go, $help, $example, $notes, $flags $(, $json)?)
    };
    (@kind $kind:expr, $input:ident, $method:ident, $name:literal, $go:literal,
     $help:literal, $example:literal, $notes:expr,
     [$($flag:ident => $field:expr),* $(,)?] $(, [$($hidden:expr => $json:expr),+ $(,)?])?) => {{
        const INPUT: InputDescriptor<sdk::$input> = InputDescriptor {
            defaults: sdk::$input::default,
            fields: &[$($field,)* $($($json,)*)?],
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: Some(check_input_source),
            guard: None,
        };
        Operation {
            name: $name,
            sdk_operation: concat!("loadbalancer.", $go),
            short_help: $help,
            example: Some($example),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: $kind,
            flags: &[$($flag),*],
            json_fields: &[$($($hidden),*)?],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
                Box::pin(dispatch::management_with_view(
                    invocation, operation, matches, &INPUT,
                    |ctx, config, input| Box::pin(async move {
                        let result = sdk::Client::new(config).$method(ctx, input.clone()).await;
                        dispatch::project_result(result, std::convert::identity)
                    }),
                    OutputView::default(),
                ))
            })),
        }
    }};
}

macro_rules! optional_bool {
    ($member:ident, $flag:ident, $name:literal) => {
        InputField {
            json_name: $name,
            flag: Some($flag),
            secret: false,
            nullable: false,
            set_json: |input: &mut _, value| {
                input.$member = value.as_bool().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input: &mut _, matches| {
                input.$member = *matches.get_one::<bool>($flag.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

const NO_WAIT: Field = flag("no-wait", "bool", false);

const CERTIFICATE_ID: Field = flag("certificate-id", "string", true);
const LISTENER_ID: Field = flag("listener-id", "string", true);
const LOAD_BALANCER_ID: Field = flag("load-balancer-id", "string", true);
const NAME: Field = flag("name", "string", false);
const PACKAGE_ID: Field = flag("package-id", "string", true);
const PAGE: Field = flag("page", "int", false);
const POLICY_ID: Field = flag("policy-id", "string", true);
const POOL_ID: Field = flag("pool-id", "string", true);
const REQUIRED_ZONE_ID: Field = flag("zone-id", "string", true);
const SCHEME: Field = flag("scheme", "string", false);
const SIZE: Field = flag("size", "int", false);
const SUBNET_ID: Field = flag("subnet-id", "string", false);
const TYPE: Field = flag("type", "string", false);
const ZONE_ID: Field = flag("zone-id", "string", false);

pub(super) const SERVICE: Service = Service {
    name: "loadbalancer",
    error_rules: &[Rule {
        identity: "loadbalancer.resource-in-use",
        matches: |error| error.detail::<sdk::WriteError>() == Some(&sdk::WriteError::ResourceInUse),
        name: Some(RuleName {
            rank: 34,
            code: "ResourceInUse",
            include_api_status: false,
            operation: RuleOperation::Omit,
        }),
        exit: None,
        message: None,
    }],
    operations: &[
        operation!(
            @kind Kind::DestructiveWrite,
            DeleteCertificateInput, delete_certificate, "delete-certificate", "DeleteCertificate",
            "Delete an unused certificate",
            "greennode loadbalancer delete-certificate --certificate-id <certificate-id> --yes",
            &["Reads the certificate before deleting it and refuses if a listener uses it."], [
                CERTIFICATE_ID => required_id!(sdk::DeleteCertificateInput, certificate_id, CERTIFICATE_ID, "CertificateID"),
            ]
        ),
        operation!(
            @kind Kind::DestructiveWrite,
            DeleteLoadBalancerInput, delete_load_balancer, "delete-load-balancer", "DeleteLoadBalancer",
            "Delete a regional load balancer",
            "greennode loadbalancer delete-load-balancer --load-balancer-id <load-balancer-id> --yes",
            &["Waits up to 15 minutes for the load balancer to disappear. Use `--no-wait` to return after acceptance or when a deletion is already in progress."], [
                LOAD_BALANCER_ID => required_id!(sdk::DeleteLoadBalancerInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
            ]
        ),
        operation!(
            GetCertificateInput, get_certificate, "get-certificate", "GetCertificate",
            "Get certificate",
            "greennode loadbalancer get-certificate --certificate-id <certificate-id> --query Certificate",
            &[], [
                CERTIFICATE_ID => required_id!(sdk::GetCertificateInput, certificate_id, CERTIFICATE_ID, "CertificateID"),
            ]
        ),
        operation!(
            GetListenerInput, get_listener, "get-listener", "GetListener",
            "Get listener",
            "greennode loadbalancer get-listener --load-balancer-id <load-balancer-id> --listener-id <listener-id> --query Listener",
            &[], [
                LOAD_BALANCER_ID => required_id!(sdk::GetListenerInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                LISTENER_ID => required_id!(sdk::GetListenerInput, listener_id, LISTENER_ID, "ListenerID"),
            ]
        ),
        operation!(
            GetLoadBalancerInput, get_load_balancer, "get-load-balancer", "GetLoadBalancer",
            "Get load balancer",
            "greennode loadbalancer get-load-balancer --load-balancer-id <load-balancer-id> --query LoadBalancer",
            &[], [
                LOAD_BALANCER_ID => required_id!(sdk::GetLoadBalancerInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
            ]
        ),
        operation!(
            GetPolicyInput, get_policy, "get-policy", "GetPolicy",
            "Get policy",
            "greennode loadbalancer get-policy --load-balancer-id <load-balancer-id> --listener-id <listener-id> --policy-id <policy-id> --query Policy",
            &[], [
                LOAD_BALANCER_ID => required_id!(sdk::GetPolicyInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                LISTENER_ID => required_id!(sdk::GetPolicyInput, listener_id, LISTENER_ID, "ListenerID"),
                POLICY_ID => required_id!(sdk::GetPolicyInput, policy_id, POLICY_ID, "PolicyID"),
            ]
        ),
        operation!(
            GetPoolInput, get_pool, "get-pool", "GetPool",
            "Get pool",
            "greennode loadbalancer get-pool --load-balancer-id <load-balancer-id> --pool-id <pool-id> --query Pool",
            &[], [
                LOAD_BALANCER_ID => required_id!(sdk::GetPoolInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                POOL_ID => required_id!(sdk::GetPoolInput, pool_id, POOL_ID, "PoolID"),
            ]
        ),
        operation!(
            GetPoolHealthMonitorInput, get_pool_health_monitor, "get-pool-health-monitor", "GetPoolHealthMonitor",
            "Get pool health monitor",
            "greennode loadbalancer get-pool-health-monitor --load-balancer-id <load-balancer-id> --pool-id <pool-id> --query HealthMonitor",
            &[], [
                LOAD_BALANCER_ID => required_id!(sdk::GetPoolHealthMonitorInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                POOL_ID => required_id!(sdk::GetPoolHealthMonitorInput, pool_id, POOL_ID, "PoolID"),
            ]
        ),
        operation!(
            ListCertificatesInput, list_certificates, "list-certificates", "ListCertificates",
            "List certificates",
            "greennode loadbalancer list-certificates",
            &[], [
                NAME => string_field!(sdk::ListCertificatesInput, name, NAME, "Name", Requirement::Optional),
                PAGE => int_field!(page, PAGE, "Page"),
                SIZE => int_field!(size, SIZE, "Size"),
            ]
        ),
        operation!(
            ListListenersInput, list_listeners, "list-listeners", "ListListeners",
            "List listeners",
            "greennode loadbalancer list-listeners --load-balancer-id <load-balancer-id>",
            &[], [
                LOAD_BALANCER_ID => required_id!(sdk::ListListenersInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
            ]
        ),
        operation!(
            ListLoadBalancersInput, list_load_balancers, "list-load-balancers", "ListLoadBalancers",
            "List load balancers",
            "greennode loadbalancer list-load-balancers --query 'Items[].{ID:UUID,Name:Name,Status:DisplayStatus}'",
            &[], [
                NAME => string_field!(sdk::ListLoadBalancersInput, name, NAME, "Name", Requirement::Optional),
                PAGE => int_field!(page, PAGE, "Page"),
                SIZE => int_field!(size, SIZE, "Size"),
            ]
        ),
        operation!(
            ListPackagesInput, list_packages, "list-packages", "ListPackages",
            "List packages",
            "greennode loadbalancer list-packages",
            &[], [
                ZONE_ID => string_field!(sdk::ListPackagesInput, zone_id, ZONE_ID, "ZoneID", Requirement::Optional),
            ]
        ),
        operation!(
            ListPoliciesInput, list_policies, "list-policies", "ListPolicies",
            "List policies",
            "greennode loadbalancer list-policies --load-balancer-id <load-balancer-id> --listener-id <listener-id>",
            &[], [
                LOAD_BALANCER_ID => required_id!(sdk::ListPoliciesInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                LISTENER_ID => required_id!(sdk::ListPoliciesInput, listener_id, LISTENER_ID, "ListenerID"),
            ]
        ),
        operation!(
            ListPoolMembersInput, list_pool_members, "list-pool-members", "ListPoolMembers",
            "List pool members",
            "greennode loadbalancer list-pool-members --load-balancer-id <load-balancer-id> --pool-id <pool-id>",
            &[], [
                LOAD_BALANCER_ID => required_id!(sdk::ListPoolMembersInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                POOL_ID => required_id!(sdk::ListPoolMembersInput, pool_id, POOL_ID, "PoolID"),
            ]
        ),
        operation!(
            ListPoolsInput, list_pools, "list-pools", "ListPools",
            "List pools",
            "greennode loadbalancer list-pools --load-balancer-id <load-balancer-id>",
            &[], [
                LOAD_BALANCER_ID => required_id!(sdk::ListPoolsInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
            ]
        ),
        operation!(
            ListTagsInput, list_tags, "list-tags", "ListTags",
            "List tags",
            "greennode loadbalancer list-tags --load-balancer-id <load-balancer-id>",
            &[], [
                LOAD_BALANCER_ID => required_id!(sdk::ListTagsInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
            ]
        ),
        operation!(
            CreateLoadBalancerInput, quote_create_load_balancer, "quote-create-load-balancer", "QuoteCreateLoadBalancer",
            "Quote create load balancer",
            "greennode loadbalancer quote-create-load-balancer --package-id <package-id> --zone-id <zone-id>",
            &["Quotes the selected package and zone without ordering. The price is in VND per month and includes VAT. `--name` and `--type` do not affect the quote. If provided, `--scheme` and `--subnet-id` are validated."], [
                NAME => string_field!(sdk::CreateLoadBalancerInput, name, NAME, "Name", Requirement::Optional),
                PACKAGE_ID => required_id!(sdk::CreateLoadBalancerInput, package_id, PACKAGE_ID, "PackageID"),
                TYPE => string_field!(sdk::CreateLoadBalancerInput, r#type, TYPE, "Type", Requirement::Optional),
                SCHEME => string_field!(sdk::CreateLoadBalancerInput, scheme, SCHEME, "Scheme", Requirement::Optional),
                SUBNET_ID => string_field!(sdk::CreateLoadBalancerInput, subnet_id, SUBNET_ID, "SubnetID", Requirement::Optional),
                REQUIRED_ZONE_ID => required_id!(sdk::CreateLoadBalancerInput, zone_id, REQUIRED_ZONE_ID, "ZoneID"),
            ], [
                flag("MaxPrice", "float64", false) => json_field!("MaxPrice", as_f64, max_price),
                flag("NoWait", "bool", false) => json_field!("NoWait", as_bool, no_wait),
            ]
        ),
        operation!(
            ResizeLoadBalancerInput, quote_resize_load_balancer, "quote-resize-load-balancer", "QuoteResizeLoadBalancer",
            "Quote resize load balancer",
            "greennode loadbalancer quote-resize-load-balancer --load-balancer-id <load-balancer-id> --package-id <package-id>",
            &["Quotes the new package without resizing. Use the load balancer’s `ZoneID` when listing packages. Prices are in VND and include VAT."], [
                LOAD_BALANCER_ID => required_id!(sdk::ResizeLoadBalancerInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
                PACKAGE_ID => required_id!(sdk::ResizeLoadBalancerInput, package_id, PACKAGE_ID, "PackageID"),
            ], [
                flag("MaxPrice", "float64", false) => json_field!("MaxPrice", as_f64, max_price),
                flag("NoWait", "bool", false) => json_field!("NoWait", as_bool, no_wait),
            ]
        ),
    ],
};
