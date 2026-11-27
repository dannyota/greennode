use greennode_sdk::network::{QuoteCreateEndpointInput, QuoteCreateNatInstanceInput};

use super::*;

const NAME: Field = Field {
    name: "name",
    go_type: "string",
    required: true,
};
const ZONE: Field = Field {
    required: true,
    ..ZONE_ID
};
const AZ: Field = Field {
    name: "availability-zone-id",
    go_type: "string",
    required: true,
};
const PACKAGE: Field = Field {
    name: "package-id",
    go_type: "string",
    required: true,
};
const VPC: Field = Field {
    name: "vpc-id",
    go_type: "string",
    required: true,
};
const MAX: Field = Field {
    name: "MaxPrice",
    go_type: "float64",
    required: false,
};

macro_rules! required_string {
    ($name:literal, $flag:ident, $field:ident) => {
        nat_field!(
            $name,
            $flag,
            $field,
            Requirement::Nonzero(|input| !input.$field.is_empty())
        )
    };
}

const NAT_INPUT: InputDescriptor<QuoteCreateNatInstanceInput> = InputDescriptor {
    defaults: || QuoteCreateNatInstanceInput::new("", "", "", "", ""),
    fields: &[
        required_string!("Name", NAME, name),
        required_string!("ZoneID", ZONE, zone_id),
        required_string!("AvailabilityZoneID", AZ, availability_zone_id),
        required_string!("PackageID", PACKAGE, package_id),
        required_string!("VPCID", VPC, vpc_id),
        InputField {
            json_name: "MaxPrice",
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.max_price = value.as_f64().ok_or(())?;
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        },
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(check_file),
    guard: None,
};

pub(super) const NAT: Operation = Operation {
    json_fields: &[MAX],
    ..read!(
        "quote-create-nat-instance",
        "network.QuoteCreateNATInstance",
        "Quote a new Public NAT instance",
        "greennode network quote-create-nat-instance --name <name> --zone-id <zone-id> --availability-zone-id <availability-zone-id> --package-id <package-id> --vpc-id <vpc-id>",
        &[NAME, ZONE, AZ, PACKAGE, VPC],
        &[
            "Reads a fresh one-month prepaid VND quote. Read-only profiles can run this command.",
            "Use the regional zone from `list-v-network-regions`, the availability zone from `list-nat-zones`, and the package UUID from `list-nat-packages`.",
            "Requires NAT V3 enabledForAll mode. The JSON `MaxPrice` field does not cap the quote.",
        ],
        NAT_INPUT,
        quote_create_nat_instance
    )
};

run!(quote_create_nat_instance, QuoteCreateNatInstanceInput);

const SUBNET: Field = Field {
    name: "subnet-id",
    go_type: "string",
    required: true,
};
const EP_AZ: Field = Field {
    required: false,
    ..AZ
};
const EP_PACKAGE: Field = Field {
    required: false,
    ..PACKAGE
};
const CATEGORY: Field = Field {
    name: "category-id",
    go_type: "string",
    required: false,
};
const SERVICE_ID: Field = Field {
    name: "service-id",
    go_type: "string",
    required: false,
};

const ENDPOINT_INPUT: InputDescriptor<QuoteCreateEndpointInput> = InputDescriptor {
    defaults: || QuoteCreateEndpointInput::new("", "", ""),
    fields: &[
        required_string!("Name", NAME, name),
        required_string!("VPCID", VPC, vpc_id),
        required_string!("SubnetID", SUBNET, subnet_id),
        nat_field!(
            "AvailabilityZoneID",
            EP_AZ,
            availability_zone_id,
            Requirement::Optional
        ),
        nat_field!("CategoryID", CATEGORY, category_id, Requirement::Optional),
        nat_field!("ServiceID", SERVICE_ID, service_id, Requirement::Optional),
        nat_field!("PackageID", EP_PACKAGE, package_id, Requirement::Optional),
        InputField {
            json_name: "MaxPrice",
            flag: None,
            secret: false,
            nullable: false,
            set_json: |input, value| {
                input.max_price = value.as_f64().ok_or(())?;
                Ok(())
            },
            set_flag: None,
            requirement: Requirement::Optional,
        },
    ],
    allow_json: true,
    allow_query: true,
    project_binding: None,
    pre_guard: Some(|matches, json| {
        for (name, refusal) in [
            (EP_AZ.name, "--availability-zone-id must not be empty"),
            (CATEGORY.name, "--category-id must not be empty"),
            (SERVICE_ID.name, "--service-id must not be empty"),
            (EP_PACKAGE.name, "--package-id must not be empty"),
        ] {
            if matches
                .get_one::<String>(name)
                .is_some_and(String::is_empty)
            {
                return Err(InputError(refusal));
            }
        }
        check_file(matches, json)
    }),
    guard: None,
};

pub(super) const ENDPOINT: Operation = Operation {
    json_fields: &[MAX],
    ..read!(
        "quote-create-endpoint",
        "network.QuoteCreateEndpoint",
        "Quote a new Gateway endpoint",
        "greennode network quote-create-endpoint --region hcm-3 --name demo --vpc-id <net-id> --subnet-id <sub-id>",
        &[NAME, VPC, SUBNET, EP_AZ, CATEGORY, SERVICE_ID, EP_PACKAGE],
        &[
            "Reads a fresh monthly VND quote for one Gateway endpoint. Read-only profiles can run this command.",
            "Requires SDK IAM-user login and default endpoints in `hcm-3`. The existing VPC and subnet must offer the selected Gateway endpoint.",
            "Omitted catalog selectors use HCM03-1C, the sole default category, vstorage.hcm03, and Standard. The JSON `MaxPrice` field does not cap the quote.",
        ],
        ENDPOINT_INPUT,
        quote_create_endpoint
    )
};

run!(quote_create_endpoint, QuoteCreateEndpointInput);
