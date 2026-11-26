use super::*;

const REQUIRED_NAME: Field = flag("name", "string", true);
const PACKAGE_ID: Field = flag("package-id", "string", true);
const DESCRIPTION: Field = flag("description", "string", false);

fn confirm_create(
    _: &sdk::CreateLoadBalancerInput,
    matches: &ArgMatches,
) -> Result<(), InputError> {
    if matches.get_one::<bool>("yes") == Some(&true) {
        Ok(())
    } else {
        Err(InputError(
            "create-load-balancer requires --yes to confirm charges",
        ))
    }
}

pub(super) const CREATE: Operation = operation!(
    CreateLoadBalancerInput, || sdk::CreateLoadBalancerInput::new("", ""), create_load_balancer,
    "create-load-balancer", "CreateLoadBalancer",
    "Create using credits-only automated payment. A prepaid three-day hold and later connection and traffic charges apply.",
    &[
        "Requires `--yes`, SDK IAM-user login, and default GLB and auth endpoints.",
        "Uses credits-only automated payment. A prepaid three-day hold and later connection and traffic charges apply.",
        "The SDK submits one create and waits up to 15 minutes for `ACTIVE`. If the write fails or remains unconfirmed, inspect inventory and billing before another mutation.",
    ], [
        REQUIRED_NAME => required_id!(sdk::CreateLoadBalancerInput, name, REQUIRED_NAME, "Name"),
        PACKAGE_ID => required_id!(sdk::CreateLoadBalancerInput, package_id, PACKAGE_ID, "PackageID"),
        DESCRIPTION => string_field!(sdk::CreateLoadBalancerInput, description, DESCRIPTION, "Description", Requirement::Optional),
    ], Kind::Write, Some(confirm_create), Some("greennode globalloadbalancer create-load-balancer --name <name> --package-id <package-id> --yes")
);

pub(super) const DELETE: Operation = operation!(
    DeleteLoadBalancerInput, || sdk::DeleteLoadBalancerInput::new(""), delete_load_balancer,
    "delete-load-balancer", "DeleteLoadBalancer", "Delete a Layer 4 global load balancer",
    &[
        "Requires `--yes`, SDK IAM-user login, and default GLB and auth endpoints. Deletion does not promise a refund.",
        "The SDK submits one delete for a Layer 4 load balancer in `ACTIVE` or `ERROR` and waits up to 10 minutes for a detail read to confirm absence.",
    ], [
        LOAD_BALANCER_ID => required_id!(sdk::DeleteLoadBalancerInput, load_balancer_id, LOAD_BALANCER_ID, "LoadBalancerID"),
    ], Kind::DestructiveWrite, None, Some("greennode globalloadbalancer delete-load-balancer --load-balancer-id <load-balancer-id> --yes")
);
