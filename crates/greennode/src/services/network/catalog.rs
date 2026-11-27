use greennode_sdk::network::{ListNatInstancesInput, ListNatPackagesInput, ListNatZonesInput};

use super::*;

const PAGE: Field = Field {
    name: "page",
    go_type: "int",
    required: false,
};
const SIZE: Field = Field {
    name: "size",
    go_type: "int",
    required: false,
};
const AZ: Field = Field {
    name: "availability-zone-id",
    go_type: "string",
    required: true,
};

const INSTANCES_INPUT: InputDescriptor<ListNatInstancesInput> = InputDescriptor {
    defaults: ListNatInstancesInput::default,
    fields: &[
        nat_field!("ZoneID", ZONE_ID, zone_id, Requirement::Optional),
        integer!("Page", PAGE, page),
        integer!("Size", SIZE, size),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};

const PACKAGES_INPUT: InputDescriptor<ListNatPackagesInput> = InputDescriptor {
    defaults: || ListNatPackagesInput::new(""),
    fields: &[
        nat_field!("ZoneID", ZONE_ID, zone_id, Requirement::Optional),
        nat_field!(
            "AvailabilityZoneID",
            AZ,
            availability_zone_id,
            Requirement::Nonzero(|input| !input.availability_zone_id.is_empty())
        ),
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};

const ZONES_INPUT: InputDescriptor<ListNatZonesInput> = InputDescriptor {
    defaults: ListNatZonesInput::default,
    fields: &[nat_field!(
        "ZoneID",
        ZONE_ID,
        zone_id,
        Requirement::Optional
    )],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};

pub(super) const INSTANCES: Operation = read!(
    "list-nat-instances",
    "network.ListNATInstances",
    "List one page of Public NAT instances in hcm-3 and han-1",
    "greennode network list-nat-instances",
    &[ZONE_ID, PAGE, SIZE],
    NAT_NOTES,
    INSTANCES_INPUT,
    list_nat_instances
);
pub(super) const PACKAGES: Operation = read!(
    "list-nat-packages",
    "network.ListNATPackages",
    "List Public NAT packages",
    "greennode network list-nat-packages --availability-zone-id <availability-zone-id>",
    &[ZONE_ID, AZ],
    NAT_NOTES,
    PACKAGES_INPUT,
    list_nat_packages
);
pub(super) const ZONES: Operation = read!(
    "list-nat-zones",
    "network.ListNATZones",
    "List Public NAT availability zones",
    "greennode network list-nat-zones",
    &[ZONE_ID],
    NAT_NOTES,
    ZONES_INPUT,
    list_nat_zones
);

run!(list_nat_instances, ListNatInstancesInput);
run!(list_nat_packages, ListNatPackagesInput);
run!(list_nat_zones, ListNatZonesInput);
