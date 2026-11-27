use std::collections::HashSet;

use serde::Serialize;
use serde_json::value::RawValue;

use crate::endpoint::{Base, vnetwork_regional_gateway};
use crate::model::wire::{Field, WireField, go_json};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, ErrorClass, Failure, List, RequestContext};

use super::wire::{Fields, PAGING, Read, Style};
use super::{Client, failure};

const DETAIL: Read = Read::new(
    "network.GetNATInstance",
    "vnetwork/v1/{}/{}/nats/{}",
    Style::Nat,
);
const RULES: Read = Read::new(
    "network.ListNATInboundRules",
    "vnetwork/v1/{}/{}/nats/{}/sec-groups/rules",
    Style::Nat,
);
const INVALID_REGIONS: Read = Read::new("network.ListNATInstances", "", Style::Nat);
const REGIONS: Read = Read::new(
    "network.ListVNetworkRegions",
    "vnetwork/v1/regions",
    Style::Nat,
);

/// Identifies the Public NAT instance to read.
#[derive(Clone, Debug)]
pub struct GetNatInstanceInput {
    /// Path-safe NAT identifier.
    pub nat_id: String,
    /// Zone identifier. An empty value discovers the zone for the configured region.
    pub zone_id: String,
}

impl GetNatInstanceInput {
    /// Selects a NAT instance in the configured region.
    pub fn new(nat_id: impl Into<String>) -> Self {
        Self {
            nat_id: nat_id.into(),
            zone_id: String::new(),
        }
    }
}

/// Detail of the selected Public NAT.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GetNatInstanceOutput {
    /// NAT configuration and status.
    #[serde(rename = "NAT")]
    pub nat: NatDetail,
}

/// Public NAT configuration and status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NatDetail {
    /// Identifier matching the requested NAT.
    #[serde(rename = "ID")]
    pub id: String,
    /// Display name.
    #[serde(rename = "Name")]
    pub name: String,
    /// Current state.
    #[serde(rename = "Status")]
    pub status: String,
    /// Gateway address.
    #[serde(rename = "GatewayIP")]
    pub gateway_ip: String,
    /// Public address.
    #[serde(rename = "PublicIP")]
    pub public_ip: String,
    /// Creation time as supplied by the service.
    #[serde(rename = "CreatedAt")]
    pub created_at: String,
    /// NAT package identifier.
    #[serde(rename = "PackageID")]
    pub package_id: String,
    /// NAT package display name.
    #[serde(rename = "PackageName")]
    pub package_name: String,
    /// VPC identifier.
    #[serde(rename = "VPCID")]
    pub vpc_id: String,
    /// VPC display name.
    #[serde(rename = "VPCName")]
    pub vpc_name: String,
}

/// Identifies the Public NAT whose inbound rules to read.
#[derive(Clone, Debug)]
pub struct ListNatInboundRulesInput {
    /// Path-safe NAT identifier.
    pub nat_id: String,
    /// Zone identifier. An empty value discovers the zone for the configured region.
    pub zone_id: String,
}

impl ListNatInboundRulesInput {
    /// Selects a NAT instance in the configured region.
    pub fn new(nat_id: impl Into<String>) -> Self {
        Self {
            nat_id: nat_id.into(),
            zone_id: String::new(),
        }
    }
}

/// Inbound rules in provider order.
pub type ListNatInboundRulesOutput = List<NatInboundRule>;

/// Inbound rule for a Public NAT instance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NatInboundRule {
    /// Provider rule identifier.
    #[serde(rename = "ID")]
    pub id: String,
    /// IP protocol.
    #[serde(rename = "Protocol")]
    pub protocol: String,
    /// Ethernet protocol type.
    #[serde(rename = "EtherType")]
    pub ether_type: String,
    /// Traffic direction.
    #[serde(rename = "Direction")]
    pub direction: String,
    /// Lower port bound, when supplied.
    #[serde(rename = "PortRangeMin")]
    pub port_range_min: Option<i64>,
    /// Upper port bound, when supplied.
    #[serde(rename = "PortRangeMax")]
    pub port_range_max: Option<i64>,
    /// Rule description.
    #[serde(rename = "Description")]
    pub description: String,
    /// Creation time as supplied by the service.
    #[serde(rename = "CreatedAt")]
    pub created_at: String,
    /// Whether the provider manages the rule.
    #[serde(rename = "System")]
    pub system: bool,
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Reads one Public NAT instance in Hanoi or HCM.
    pub async fn get_nat_instance(
        &self,
        ctx: &RequestContext,
        input: GetNatInstanceInput,
    ) -> Result<GetNatInstanceOutput, Failure<GetNatInstanceOutput>> {
        let (raw, evidence) = self
            .nat_read(ctx, DETAIL, &input.nat_id, input.zone_id)
            .await
            .map_err(failure)?;
        let text = go_json(&raw);
        let nat = decode_detail(&text, &input.nat_id, &evidence)
            .map_err(|()| failure(DETAIL.invalid(&evidence)))?;
        Ok(GetNatInstanceOutput { nat })
    }

    /// Lists a Public NAT instance's inbound rules in Hanoi or HCM.
    pub async fn list_nat_inbound_rules(
        &self,
        ctx: &RequestContext,
        input: ListNatInboundRulesInput,
    ) -> Result<ListNatInboundRulesOutput, Failure<ListNatInboundRulesOutput>> {
        let (raw, evidence) = self
            .nat_read(ctx, RULES, &input.nat_id, input.zone_id)
            .await
            .map_err(failure)?;
        let text = go_json(&raw);
        let items =
            decode_rules(&text, &evidence).map_err(|()| failure(RULES.invalid(&evidence)))?;
        Ok(List { items })
    }

    async fn nat_read(
        &self,
        ctx: &RequestContext,
        read: Read,
        id: &str,
        mut zone: String,
    ) -> Result<(Vec<u8>, Evidence), Error> {
        read.path("NATID", id)?;
        if !zone.is_empty() {
            read.path("ZoneID", &zone)?;
        }
        let project = self
            .session
            .project_id_context(ctx)
            .await
            .map_err(|error| *error)?;
        if let Some(project) = &project {
            read.path("ProjectID", project)?;
        }
        let base = self.nat_base()?;
        let mut evidence = self.request_evidence(ctx)?;
        if [Some(id), Some(zone.as_str()), project.as_deref()]
            .into_iter()
            .flatten()
            .any(|value| evidence.text_reflects(value))
        {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        let project = match project {
            Some(project) => project,
            None => {
                let vserver = self
                    .session
                    .endpoints()
                    .vserver
                    .as_ref()
                    .ok_or_else(|| Error::new(ErrorClass::InvalidConfig))?;
                self.discover_project(
                    ctx,
                    vserver,
                    read.at("v1/projects", Style::Nat),
                    &mut evidence,
                )
                .await?
            }
        };
        if zone.is_empty() {
            zone = self.nat_zone(ctx, &base, &mut evidence).await?;
        }
        if [zone.as_str(), project.as_str()]
            .into_iter()
            .any(|value| evidence.id_reflects(value))
        {
            return Err(read.invalid(&evidence));
        }
        let raw = self
            .read(ctx, &base, read, &[&zone, &project, id], &mut evidence)
            .await?;
        Ok((raw, evidence))
    }

    pub(super) fn nat_base(&self) -> Result<Base, Error> {
        if !matches!(self.region.as_str(), "hcm-3" | "han-1") {
            return Err(Error::new(ErrorClass::InvalidConfig));
        }
        self.session
            .endpoints()
            .vnetwork
            .as_ref()
            .filter(|base| base.is_explicit_override())
            .and_then(|base| Base::parse(base.configured_url()).ok())
            .or_else(|| vnetwork_regional_gateway(&self.region))
            .ok_or_else(|| Error::new(ErrorClass::InvalidConfig))
    }

    pub(super) async fn nat_zone(
        &self,
        ctx: &RequestContext,
        base: &Base,
        evidence: &mut Evidence,
    ) -> Result<String, Error> {
        let raw = self.read(ctx, base, REGIONS, &[], evidence).await?;
        let invalid = || INVALID_REGIONS.invalid_as(evidence, "invalid regions envelope");
        let text = go_json(&raw);
        if evidence.go_peer_body_reflects(&raw) != Some(false) {
            return Err(invalid());
        }
        let data: &RawValue = nat_data(&text).map_err(|()| invalid())?;
        if data.get() == "null" {
            return Err(invalid());
        }
        let row_invalid = || INVALID_REGIONS.invalid_as(evidence, "invalid regions data");
        let regions: Vec<Fields<'_>> =
            serde_json::from_str(data.get()).map_err(|_| row_invalid())?;
        let origin = format!("https://{}-vnetwork.console.greennode.ai", self.region);
        let other = if self.region == "hcm-3" {
            "han-1"
        } else {
            "hcm-3"
        };
        let other_origin = format!("https://{other}-vnetwork.console.greennode.ai");
        let mut selected = None;
        for row in regions {
            let dashboard: String = row
                .optional::<Option<_>>("vnetworkDashboard")
                .map_err(|()| row_invalid())?
                .flatten()
                .unwrap_or_default();
            let gateway: String = row
                .optional::<Option<_>>("gatewayUrl")
                .map_err(|()| row_invalid())?
                .flatten()
                .unwrap_or_default();
            let zone: String = row
                .optional::<Option<_>>("uuid")
                .map_err(|()| row_invalid())?
                .flatten()
                .unwrap_or_default();
            let dashboard = dashboard.trim_end_matches('/');
            let gateway = gateway.trim_end_matches('/');
            let matches = |origin: &str| {
                dashboard == origin || gateway.strip_suffix("/vnetwork-gateway") == Some(origin)
            };
            if !matches(&origin) {
                continue;
            }
            if matches(&other_origin)
                || !PathIds::Strict.allows(&zone)
                || selected.replace(zone).is_some()
            {
                return Err(Error::new(ErrorClass::InvalidConfig));
            }
        }
        selected.ok_or_else(|| Error::new(ErrorClass::InvalidConfig))
    }
}

fn nat_data<'a, T: serde::Deserialize<'a>>(text: &'a str) -> Result<T, ()> {
    let fields = Fields::decode(text)?;
    if !fields.required::<bool>("success")? {
        return Err(());
    }
    fields.required("data")
}

fn decode_detail(text: &str, expected_id: &str, evidence: &Evidence) -> Result<NatDetail, ()> {
    let fields: Fields<'_> = nat_data(text)?;
    let id: String = fields.required("uuid")?;
    let status: String = fields.required("status")?;
    let package: Fields<'_> = fields.required("natPackage")?;
    let vpc: Fields<'_> = fields.required("vpc")?;
    let vpc_id: String = vpc.required("uuid")?;
    if id != expected_id
        || !PathIds::Strict.allows(&vpc_id)
        || evidence.id_reflects(&id)
        || evidence.id_reflects(&vpc_id)
    {
        return Err(());
    }
    let display = |fields: &Fields<'_>, name| -> Result<String, ()> {
        let text: Option<String> = fields.optional::<Option<_>>(name)?.flatten();
        if status == "ACTIVE" {
            text.ok_or(())
        } else {
            Ok(text.unwrap_or_default())
        }
    };
    let mut nat = NatDetail {
        id,
        name: display(&fields, "natName")?,
        gateway_ip: display(&fields, "natGatewayIp")?,
        public_ip: display(&fields, "publicIp")?,
        created_at: display(&fields, "createdAt")?,
        package_id: package.required("uuid")?,
        package_name: display(&package, "name")?,
        vpc_id,
        vpc_name: display(&vpc, "name")?,
        status,
    };
    if evidence.id_reflects(&nat.package_id) {
        return Err(());
    }
    redact_strings(
        evidence,
        [
            &mut nat.id,
            &mut nat.name,
            &mut nat.status,
            &mut nat.gateway_ip,
            &mut nat.public_ip,
            &mut nat.created_at,
            &mut nat.package_id,
            &mut nat.package_name,
            &mut nat.vpc_id,
            &mut nat.vpc_name,
        ],
    )?;
    Ok(nat)
}

fn decode_rules(text: &str, evidence: &Evidence) -> Result<Vec<NatInboundRule>, ()> {
    let fields = Fields::decode(text)?;
    if !fields.required::<bool>("success")? {
        return Err(());
    }
    let rows: Vec<Fields<'_>> = fields.required("data")?;
    for name in PAGING {
        fields.raw(name)?;
    }
    for (name, ceiling) in [
        ("page", 1),
        ("totalPage", 1),
        ("total", rows.len()),
        ("totalItem", rows.len()),
        ("totalItems", rows.len()),
    ] {
        if let Some(Field(value)) = fields.optional::<Field<Option<i64>>>(name)?
            && !value
                .and_then(|value| usize::try_from(value).ok())
                .is_some_and(|value| value <= ceiling)
        {
            return Err(());
        }
    }
    for name in ["next", "nextPage"] {
        if fields
            .optional::<String>(name)?
            .is_some_and(|value| !value.is_empty())
        {
            return Err(());
        }
    }
    let mut seen = HashSet::new();
    rows.into_iter()
        .map(|row| {
            let mut item = NatInboundRule {
                id: row.required("uuid")?,
                protocol: row.required("protocol")?,
                ether_type: row.required("etherType")?,
                direction: row.required("direction")?,
                port_range_min: row.required::<Field<Option<i64>>>("portRangeMin")?.0,
                port_range_max: row.required::<Field<Option<i64>>>("portRangeMax")?.0,
                description: row
                    .optional::<Option<_>>("description")?
                    .flatten()
                    .unwrap_or_default(),
                created_at: row
                    .optional::<Option<_>>("createdAt")?
                    .flatten()
                    .unwrap_or_default(),
                system: row.required("isSystem")?,
            };
            if !PathIds::WithUnderscore.allows(&item.id)
                || evidence.id_reflects(&item.id)
                || !seen.insert(item.id.clone())
            {
                return Err(());
            }
            redact_strings(
                evidence,
                [
                    &mut item.id,
                    &mut item.protocol,
                    &mut item.ether_type,
                    &mut item.direction,
                    &mut item.description,
                    &mut item.created_at,
                ],
            )?;
            for port in [item.port_range_min, item.port_range_max] {
                let text = port.map_or_else(|| "null".into(), |port| port.to_string());
                if evidence.text_reflects(&text)
                    || port.is_some_and(|mut port| !port.redact(evidence))
                {
                    return Err(());
                }
            }
            if evidence.text_reflects(if item.system { "true" } else { "false" }) {
                return Err(());
            }
            Ok(item)
        })
        .collect()
}

fn redact_strings<'a>(
    evidence: &Evidence,
    strings: impl IntoIterator<Item = &'a mut String>,
) -> Result<(), ()> {
    strings
        .into_iter()
        .all(|text| !evidence.credentials().reflects(text.as_bytes()) && text.redact(evidence))
        .then_some(())
        .ok_or(())
}
