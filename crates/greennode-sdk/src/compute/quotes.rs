use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::pricing::{GetQuoteInput, GetQuoteOutput, PriceProperty};
use crate::quote::{input, request::decode::invalid_quote};
use crate::wire::product::evidence::Evidence;
use crate::wire::product::options::PathIds;
use crate::{Error, ErrorClass, Failure, RequestContext, Secret};

use super::wire::{api, invalid_input};
use super::{Client, failure};

/// Server configuration for both an order and its quote.
#[derive(Clone, Default)]
pub struct CreateServerInput {
    /// Name of the server.
    pub name: String,
    /// Target availability zone.
    pub zone_id: String,
    /// Selected compute flavor.
    pub flavor_id: String,
    /// Boot image.
    pub image_id: String,
    /// Network to attach.
    pub vpc_id: String,
    /// Subnet on the network.
    pub subnet_id: String,
    /// Security groups to attach.
    pub security_group_ids: Vec<String>,
    /// Existing SSH key for guest login.
    pub ssh_key_id: String,
    /// Root disk size in GB.
    pub root_disk_size: i64,
    /// Root disk type.
    pub root_disk_type_id: String,
    /// Optional root disk encryption type.
    pub root_disk_encryption_type_id: String,
    /// Optional data disk size in GB.
    pub data_disk_size: i64,
    /// Optional data disk type.
    pub data_disk_type_id: String,
    /// Optional data disk name.
    pub data_disk_name: String,
    /// Optional data disk encryption type.
    pub data_disk_encryption_type_id: String,
    /// Optional server group.
    pub server_group_id: String,
    /// Cloud-init configuration for guest login, excluded from pricing.
    pub user_data: Option<Secret>,
    /// Enable automatic renewal.
    pub auto_renew: bool,
    /// Include a floating IP.
    pub attach_floating_ip: bool,
    /// Permit a floating IP with publicly accessible login ports.
    pub allow_open_login: bool,
    /// Maximum estimated monthly price in VND.
    pub max_price: f64,
    /// Skip confirmation after ordering.
    pub no_wait: bool,
}

crate::error::debug_record!(CreateServerInput {
    name,
    zone_id,
    flavor_id,
    image_id,
    vpc_id,
    subnet_id,
    security_group_ids,
    ssh_key_id,
    root_disk_size,
    root_disk_type_id,
    root_disk_encryption_type_id,
    data_disk_size,
    data_disk_type_id,
    data_disk_name,
    data_disk_encryption_type_id,
    server_group_id,
    user_data,
    auto_renew,
    attach_floating_ip,
    allow_open_login,
    max_price,
    no_wait
});

/// Flavor change for both a resize and its quote.
#[derive(Clone, Default)]
pub struct ResizeServerInput {
    /// Server being resized.
    pub server_id: String,
    /// New flavor.
    pub flavor_id: String,
    /// Maximum quoted charge in VND.
    pub max_price: f64,
    /// Skip confirmation after resizing.
    pub no_wait: bool,
}

crate::error::debug_record!(ResizeServerInput {
    server_id,
    flavor_id,
    max_price,
    no_wait
});

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Quotes a new server without placing an order.
    pub async fn quote_create_server(
        &self,
        ctx: &RequestContext,
        input: CreateServerInput,
    ) -> Result<GetQuoteOutput, Failure<GetQuoteOutput>> {
        crate::wire::product::exit::finish!(async {
            let info = server_info("compute.QuoteCreateServer", &input)
                .map_err(|error| failure(*error))?;
            let mut evidence = Evidence::new();
            add_user_data(&input, &mut evidence);
            self.create_quote(ctx, &input, info, &mut evidence)
                .await
                .map_err(|error| failure(*error))
        })
    }

    /// Quotes a flavor change, which may return a credit.
    pub async fn quote_resize_server(
        &self,
        ctx: &RequestContext,
        input: ResizeServerInput,
    ) -> Result<GetQuoteOutput, Failure<GetQuoteOutput>> {
        crate::wire::product::exit::finish!(async {
            let info = resize_info("compute.QuoteResizeServer", &input)
                .map_err(|error| failure(*error))?;
            self.server_quote(ctx, "server", "resize", info, false, &mut Evidence::new())
                .await
                .map_err(|error| failure(*error))
        })
    }
}

impl Client {
    pub(super) async fn create_quote(
        &self,
        ctx: &RequestContext,
        input: &CreateServerInput,
        info: BTreeMap<String, Value>,
        evidence: &mut Evidence,
    ) -> Result<GetQuoteOutput, Box<Error>> {
        let ctx = self.server_deadline(ctx);
        let mut server = self
            .server_quote(
                &ctx,
                "server",
                "create",
                info,
                input.attach_floating_ip,
                evidence,
            )
            .await?;
        if !input.attach_floating_ip {
            return Ok(server);
        }
        validate_component(&server, evidence)?;
        if matches!(self.region.as_str(), "han-1" | "hcm-3") {
            server.properties.push(PriceProperty {
                name: "FLOATING IP".into(),
                description: "Included in server order for the verified configuration".into(),
                ..PriceProperty::default()
            });
        } else {
            let ip = self
                .server_quote(
                    &ctx,
                    "elastic-ip",
                    "create",
                    BTreeMap::from([
                        ("zoneId".into(), json!(input.zone_id)),
                        ("period".into(), json!(1)),
                        ("isPoc".into(), json!(false)),
                    ]),
                    true,
                    evidence,
                )
                .await?;
            validate_component(&ip, evidence)?;
            combine_quote(&mut server, ip);
            validate_component(&server, evidence)?;
        }
        if serde_json::to_value(&server).map_or(true, |value| evidence.output_reflects(&value)) {
            return Err(component_error());
        }
        Ok(server)
    }

    pub(super) async fn server_quote(
        &self,
        ctx: &RequestContext,
        resource: &str,
        action: &str,
        info: BTreeMap<String, Value>,
        component: bool,
        evidence: &mut Evidence,
    ) -> Result<GetQuoteOutput, Box<Error>> {
        let ctx = self.server_deadline(ctx);
        let input = GetQuoteInput {
            resource_type: resource.into(),
            action: action.into(),
            resource_info: Some(info),
        };
        self.pricing
            .compute_quote_with_evidence(&ctx, input, evidence, component)
            .await
            .map_err(|error| Box::new(error.into_parts().0))
    }
}

pub(super) fn add_user_data(input: &CreateServerInput, evidence: &mut Evidence) {
    if let Some(data) = &input.user_data {
        evidence.add_redaction(data);
        evidence.add_redaction(&encoded_user_data(data));
    }
}

pub(super) fn encoded_user_data(data: &Secret) -> Secret {
    Secret::new(data_encoding::BASE64.encode(data.expose_secret().as_bytes()))
}

pub(super) fn required(op: &str, fields: &[(&str, bool)]) -> Result<(), Box<Error>> {
    let missing: Vec<_> = fields
        .iter()
        .filter_map(|(name, present)| (!present).then_some(*name))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(Box::new(invalid_input(&format!(
        "{op} requires {}",
        missing.join(", ")
    ))))
}

pub(super) fn path_id(op: &str, field: &str, id: &str) -> Result<(), Box<Error>> {
    required(op, &[(field, !id.is_empty())])?;
    if PathIds::Strict.allows(id) {
        return Ok(());
    }
    Err(Box::new(invalid_input(&format!(
        "{op} requires {field} to match ^[A-Za-z0-9-]+$"
    ))))
}

pub(super) fn server_info(
    op: &str,
    server: &CreateServerInput,
) -> Result<BTreeMap<String, Value>, Box<Error>> {
    required(
        op,
        &[
            ("ZoneID", !server.zone_id.is_empty()),
            ("FlavorID", !server.flavor_id.is_empty()),
            ("ImageID", !server.image_id.is_empty()),
            ("RootDiskSize", server.root_disk_size != 0),
            ("RootDiskTypeID", !server.root_disk_type_id.is_empty()),
        ],
    )?;
    server_shape(op, server)?;
    let mut body = input::server(input::Server {
        zone: &server.zone_id,
        flavor: &server.flavor_id,
        image: &server.image_id,
        root_size: server.root_disk_size,
        root_type: &server.root_disk_type_id,
        data_size: server.data_disk_size,
        data_type: &server.data_disk_type_id,
        encrypted: encrypted(server),
    });
    serde_json::from_value(body["resourceInfo"].take())
        .map_err(|_| Box::new(Error::new(ErrorClass::InvalidInput)))
}

pub(super) fn encrypted(server: &CreateServerInput) -> bool {
    !server.root_disk_encryption_type_id.is_empty()
        || !server.data_disk_encryption_type_id.is_empty()
}

fn server_shape(op: &str, server: &CreateServerInput) -> Result<(), Box<Error>> {
    let invalid = |detail: &str| Box::new(invalid_input(&format!("{op}: {detail}")));
    path_id(op, "ZoneID", &server.zone_id)?;
    if server.root_disk_size < 0 {
        return Err(invalid("RootDiskSize must be greater than 0"));
    }
    if server.data_disk_size < 0 {
        return Err(invalid("DataDiskSize must not be negative"));
    }
    if (server.data_disk_size > 0) == server.data_disk_type_id.is_empty() {
        return Err(Box::new(invalid_input(&format!(
            "{op} requires DataDiskSize and DataDiskTypeID together or neither"
        ))));
    }
    if !server.data_disk_encryption_type_id.is_empty() && server.data_disk_size == 0 {
        return Err(invalid(
            "DataDiskEncryptionTypeID needs DataDiskSize and DataDiskTypeID",
        ));
    }
    for (field, id) in [
        ("FlavorID", &server.flavor_id),
        ("ImageID", &server.image_id),
        ("VPCID", &server.vpc_id),
        ("SubnetID", &server.subnet_id),
        ("SSHKeyID", &server.ssh_key_id),
        ("RootDiskTypeID", &server.root_disk_type_id),
        ("ServerGroupID", &server.server_group_id),
        ("DataDiskTypeID", &server.data_disk_type_id),
    ] {
        if !id.is_empty() {
            path_id(op, field, id)?;
        }
    }
    for (field, id) in [
        (
            "RootDiskEncryptionTypeID",
            &server.root_disk_encryption_type_id,
        ),
        (
            "DataDiskEncryptionTypeID",
            &server.data_disk_encryption_type_id,
        ),
    ] {
        if !id.is_empty()
            && !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(Box::new(invalid_input(&format!(
                "{op} requires {field} to match ^[A-Za-z0-9_-]+$"
            ))));
        }
    }
    for (index, id) in server.security_group_ids.iter().enumerate() {
        path_id(op, &format!("SecurityGroupIDs[{index}]"), id)?;
    }
    Ok(())
}

pub(super) fn resize_info(
    op: &str,
    input: &ResizeServerInput,
) -> Result<BTreeMap<String, Value>, Box<Error>> {
    required(
        op,
        &[
            ("ServerID", !input.server_id.is_empty()),
            ("FlavorID", !input.flavor_id.is_empty()),
        ],
    )?;
    path_id(op, "ServerID", &input.server_id)?;
    path_id(op, "FlavorID", &input.flavor_id)?;
    Ok(BTreeMap::from([
        ("flavorId".into(), json!(input.flavor_id)),
        ("serverId".into(), json!(input.server_id)),
    ]))
}

fn component_error() -> Box<Error> {
    Box::new(invalid_quote("pricing.GetQuote", None))
}

fn validate_component(quote: &GetQuoteOutput, evidence: &Evidence) -> Result<(), Box<Error>> {
    if quote.optimum_price <= 0.0 {
        return Err(Box::new(Error::new(ErrorClass::Unpriced).with_cause(api(
            "", None, "Unpriced", "vngcloud: quote priced the order at 0; refusing to order an unpriced paid resource: conservative monthly component estimate is not positive", evidence,
        ))));
    }
    let finite = |amount: f64| amount.is_finite() && amount >= 0.0;
    let discount = quote.original_price - quote.optimum_price;
    let tolerance = 4.0 * (quote.original_price - quote.original_price.next_down());
    let percent = 100.0 * (discount / quote.original_price);
    if ![
        quote.optimum_price,
        quote.original_price,
        quote.discount_price,
        quote.discount_percent,
    ]
    .into_iter()
    .all(finite)
        || quote.discount_percent > 100.0
        || discount < 0.0
        || (discount - quote.discount_price).abs() > tolerance
        || (quote.discount_percent - percent).abs() > 1e-7
        || quote.properties.iter().any(|line| {
            ![
                line.optimum_price,
                line.monthly_price,
                line.discount_percent,
            ]
            .into_iter()
            .all(finite)
                || line.discount_percent > 100.0
                || line.current_price.is_some_and(|price| !finite(price))
        })
    {
        return Err(component_error());
    }
    Ok(())
}

fn combine_quote(server: &mut GetQuoteOutput, ip: GetQuoteOutput) {
    const DESCRIPTION: &str = "Conservative Elastic IP estimate; extra create charge unconfirmed";
    server.optimum_price += ip.optimum_price;
    server.original_price += ip.original_price;
    server.discount_price += ip.discount_price;
    server.discount_percent =
        100.0 * ((server.original_price - server.optimum_price) / server.original_price);
    if ip.properties.is_empty() {
        server.properties.push(PriceProperty {
            name: "FLOATING IP".into(),
            description: DESCRIPTION.into(),
            optimum_price: ip.optimum_price,
            monthly_price: ip.optimum_price,
            discount_percent: ip.discount_percent,
            ..PriceProperty::default()
        });
    } else {
        server
            .properties
            .extend(ip.properties.into_iter().map(|mut line| {
                line.name = format!("FLOATING IP: {}", line.name);
                line.description = DESCRIPTION.into();
                line
            }));
    }
}
