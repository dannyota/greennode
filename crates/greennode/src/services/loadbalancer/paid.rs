use clap::ArgMatches;

use crate::cli::dispatch;
use crate::cli::input::{InputDescriptor, InputError, InputField, Requirement};
use crate::cli::op::{Field, Handler, Kind, Operation};

use super::{
    LOAD_BALANCER_ID, NO_WAIT, PACKAGE_ID, REQUIRED_ZONE_ID, check_input_source, flag, sdk,
};

const NAME: Field = flag("name", "string", true);
const TYPE: Field = flag("type", "string", true);
const SCHEME: Field = flag("scheme", "string", true);
const SUBNET_ID: Field = flag("subnet-id", "string", true);
const MAX_PRICE: Field = flag("max-price", "float64", false);

macro_rules! max_price {
    () => {
        InputField {
            json_name: "MaxPrice",
            flag: Some(MAX_PRICE),
            secret: false,
            nullable: false,
            set_json: |input: &mut _, value| {
                input.max_price = value.as_f64().ok_or(())?;
                Ok(())
            },
            set_flag: Some(|input: &mut _, matches| {
                input.max_price = *matches.get_one::<f64>(MAX_PRICE.name).ok_or(())?;
                Ok(())
            }),
            requirement: Requirement::Optional,
        }
    };
}

macro_rules! paid_write {
    ($input:ident, $method:ident, $name:literal, $sdk:literal, $help:literal, $example:literal, $notes:expr, $guard:expr, [$($flag:ident => $field:expr),+ $(,)?]) => {{
        const INPUT: InputDescriptor<sdk::$input> = InputDescriptor {
            defaults: sdk::$input::default,
            fields: &[$($field),+],
            allow_json: true,
            allow_query: true,
            project_binding: None,
            pre_guard: Some(check_input_source),
            guard: $guard,
        };
        Operation {
            name: $name,
            sdk_operation: $sdk,
            short_help: $help,
            example: Some($example),
            presence_required_fields: &[],
            inherited_bindings: &[],
            kind: Kind::Write,
            flags: &[$($flag),+],
            json_fields: &[],
            extra_flags: &[],
            notes: $notes,
            handler: Some(Handler::new(&INPUT, |operation, matches, invocation| {
                Box::pin(dispatch::management(invocation, operation, matches, &INPUT, |ctx, config, input| {
                    Box::pin(async move {
                        let result = sdk::Client::new(config).$method(ctx, input.clone()).await;
                        dispatch::project_result(result, std::convert::identity)
                    })
                }))
            })),
        }
    }};
}

type Create = sdk::CreateLoadBalancerInput;

pub(super) const CREATE: Operation = paid_write!(
    CreateLoadBalancerInput, create_load_balancer, "create-load-balancer", "loadbalancer.CreateLoadBalancer",
    "Order a regional load balancer",
    "greennode loadbalancer create-load-balancer --name <name> --type 'Layer 4' --scheme Internal --subnet-id <subnet-id> --zone-id <zone-id> --package-id <package-id> --max-price <monthly-vnd>",
    &["Set a maximum monthly price in VND. An Internet scheme requires `--yes`. Waits up to 20 minutes for creation unless `--no-wait` is set."],
    Some(guard_create),
    [
        NAME => required_id!(Create, name, NAME, "Name"),
        PACKAGE_ID => required_id!(Create, package_id, PACKAGE_ID, "PackageID"),
        TYPE => required_id!(Create, r#type, TYPE, "Type"),
        SCHEME => required_id!(Create, scheme, SCHEME, "Scheme"),
        SUBNET_ID => required_id!(Create, subnet_id, SUBNET_ID, "SubnetID"),
        REQUIRED_ZONE_ID => required_id!(Create, zone_id, REQUIRED_ZONE_ID, "ZoneID"),
        MAX_PRICE => max_price!(),
        NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
    ]
);

type Resize = sdk::ResizeLoadBalancerInput;

pub(super) const RESIZE: Operation = paid_write!(
    ResizeLoadBalancerInput, resize_load_balancer, "resize-load-balancer", "loadbalancer.ResizeLoadBalancer",
    "Change a regional load balancer's package",
    "greennode loadbalancer resize-load-balancer --load-balancer-id <load-balancer-id> --package-id <package-id> --max-price <vnd>",
    &["The target package comes from the load balancer's zone. The current package is a no-op. Resize accepts negative quotes as credits and waits up to 45 minutes unless `--no-wait` is set."],
    None,
    [
        LOAD_BALANCER_ID => required_id!(Resize, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
        PACKAGE_ID => required_id!(Resize, package_id, PACKAGE_ID, "PackageID"),
        MAX_PRICE => max_price!(),
        NO_WAIT => optional_bool!(no_wait, NO_WAIT, "NoWait"),
    ]
);

fn guard_create(input: &Create, matches: &ArgMatches) -> Result<(), InputError> {
    if input.scheme.trim().eq_ignore_ascii_case("Internal")
        || matches.get_one::<bool>("yes") == Some(&true)
    {
        return Ok(());
    }
    let message = match input.scheme.as_str() {
        "" => {
            "create-load-balancer: Scheme \"\" may get a public address reachable from the entire internet; pass --yes to confirm"
        }
        "Internet" => {
            "create-load-balancer: Scheme \"Internet\" may get a public address reachable from the entire internet; pass --yes to confirm"
        }
        _ => {
            "create-load-balancer: Scheme may get a public address reachable from the entire internet; pass --yes to confirm"
        }
    };
    Err(InputError(message))
}
