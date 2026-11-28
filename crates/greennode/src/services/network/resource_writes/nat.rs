use super::*;

const NAT_AZ_REQUIRED: Field = flag("availability-zone-id", "string", true);
const NAT_PACKAGE_REQUIRED: Field = flag("package-id", "string", true);
const CREATE_NAT_INSTANCE_INPUT: InputDescriptor<CreateNatInstanceInput> = InputDescriptor {
    defaults: || {
        let mut input = CreateNatInstanceInput::new("", "", "", "", "");
        input.max_price = MISSING_PRICE;
        input
    },
    fields: &[
        text!("Name", NAME_REQUIRED, name, required),
        text!("ZoneID", ZONE_ID_REQUIRED, zone_id, required),
        text!(
            "AvailabilityZoneID",
            NAT_AZ_REQUIRED,
            availability_zone_id,
            required
        ),
        text!("PackageID", NAT_PACKAGE_REQUIRED, package_id, required),
        text!("VPCID", VPC_ID_REQUIRED, vpc_id, required),
        max_price!(MAX_PRICE_REQUIRED),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: Some(|input, matches| {
        purchase_consent(
            matches,
            input.max_price,
            "create-nat-instance changes VPC egress; pass --yes to confirm",
        )
    }),
};
run!(create_nat_instance, CreateNatInstanceInput);
pub(in crate::services::network) const CREATE_NAT_INSTANCE: Operation = Operation {
    handler: Some(Handler::new(
        &CREATE_NAT_INSTANCE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management_with_read_only_code(
                invocation,
                operation,
                matches,
                &CREATE_NAT_INSTANCE_INPUT,
                create_nat_instance,
                "InvalidUsage",
            ))
        },
    )),
    ..operation!(
        "create-nat-instance",
        "network.CreateNATInstance",
        Write,
        "Create nat instance",
        "greennode network create-nat-instance --region han-1 --name example --zone-id <region-uuid> --availability-zone-id HAN01-1B --package-id <package-uuid> --vpc-id <disposable-vpc-id> --max-price <VND> --yes",
        create_nat_instance,
        CREATE_NAT_INSTANCE_INPUT,
        &[
            NAME_REQUIRED,
            ZONE_ID_REQUIRED,
            NAT_AZ_REQUIRED,
            NAT_PACKAGE_REQUIRED,
            VPC_ID_REQUIRED,
            MAX_PRICE_REQUIRED
        ],
        &["MaxPrice"],
        &[
            "Creates a 0.0.0.0/0 route in the named VPC. Every VM in that VPC uses this NAT for egress. Use a disposable VPC for testing.",
            "The purchase starts with auto-renew enabled. After the NAT is ACTIVE, this command disables renewal through billing and confirms it. The command waits for both steps. If it fails after purchase, renewal may remain enabled; inspect the NAT and billing before taking another action.",
            "Use the region UUID from network list-v-network-regions, the availability-zone UUID from network list-nat-zones, and the package UUID from network list-nat-packages for that zone.",
            "Requires SDK IAM-user login in han-1 or hcm-3, NAT V3 eligibility, --yes, and a finite nonnegative --max-price (or non-null JSON MaxPrice). The cap is compared with the fresh one-month prepaid quote in VND. The command waits up to 15 minutes after the order attempt, subject to the caller's deadline.",
            "After an unconfirmed purchase, run network list-nat-instances and billing list-resources. Inspect payment history if the NAT is absent. If renewal is enabled or unknown, disable it deliberately in the billing console and confirm it with another billing read."
        ]
    )
};

const DELETE_NAT_INSTANCE_INPUT: InputDescriptor<DeleteNatInstanceInput> = InputDescriptor {
    defaults: || DeleteNatInstanceInput::new("", "", ""),
    fields: &[
        text!("ZoneID", ZONE_ID_REQUIRED, zone_id, required),
        text!("VPCID", VPC_ID_REQUIRED, vpc_id, required),
        text!("NATID", NAT_ID_REQUIRED, nat_id, required),
        boolean!("NoWait", NO_WAIT, no_wait),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};
run!(delete_nat_instance, DeleteNatInstanceInput);
pub(in crate::services::network) const DELETE_NAT_INSTANCE: Operation = Operation {
    handler: Some(Handler::new(
        &DELETE_NAT_INSTANCE_INPUT,
        |operation, matches, invocation| {
            Box::pin(dispatch::management_with_read_only_code(
                invocation,
                operation,
                matches,
                &DELETE_NAT_INSTANCE_INPUT,
                delete_nat_instance,
                "InvalidUsage",
            ))
        },
    )),
    ..operation!(
        "delete-nat-instance",
        "network.DeleteNATInstance",
        DestructiveWrite,
        "Delete nat instance",
        "greennode network delete-nat-instance --region han-1 --zone-id <region-uuid> --vpc-id <disposable-vpc-id> --nat-id <nat-id> --yes",
        delete_nat_instance,
        DELETE_NAT_INSTANCE_INPUT,
        &[ZONE_ID_REQUIRED, VPC_ID_REQUIRED, NAT_ID_REQUIRED, NO_WAIT],
        &[],
        &[
            "Requires --yes: egress can stop for every VM in the named VPC, and prior routes are not restored by the CLI.",
            "--zone-id is the required region-level vNetwork ID. greennode network list-v-network-regions lists it: use the uuid of the selected region's row.",
            "NAT writes support HCM (hcm-3) and HAN (han-1) with IAM-user login only; The Rust port uses hermetic verification. Unsupported auth or region returns InvalidConfig (exit 2); create also returns it when the account's NAT V3 mode is not enabled for all users.",
            "Requires explicit --vpc-id and --nat-id and refuses read-only profiles before any request. Confirms the NAT belongs to the named VPC, then sends one delete. Waits for absence, polling every 5 seconds for up to 10 minutes; a shorter context deadline wins. --no-wait confirms acceptance only, not absence, refund, or route restoration. NotFound exits 4. After NotSettled, inspect network list-nat-instances in the same scope before taking another action."
        ]
    )
};
