use std::{collections::HashSet, time::Duration};

use serde::Serialize;
use serde_json::value::RawValue;

use crate::config::session::AuthKind;
use crate::endpoint::{Base, vnetwork_regional_gateway};
use crate::model::wire::{Field, go_json};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, ErrorClass, RequestContext};

use super::Client;
use super::endpoint_quote::{OP, QuoteCreateEndpointInput, fields, local_error};
use super::inventory::Selection;
use super::nat_catalog::reflects;
use super::wire::{Fields, PAGING, Read, Style};

const PROJECTS: Read = Read::new(OP, "v1/projects", Style::EndpointQuote);
const REGIONS: Read = PROJECTS.at("vnetwork/v1/regions", Style::EndpointQuote);
const ZONES: Read = PROJECTS.at("vnetwork/v1/{}/{}/endpoints/zones", Style::EndpointQuote);
const CATEGORIES: Read = PROJECTS.at(
    "vnetwork/v1/{}/{}/endpoints/categories",
    Style::EndpointQuote,
);
const SERVICES: Read = PROJECTS.at(
    "vnetwork/v1/{}/{}/endpoints/{}/services",
    Style::EndpointQuote,
);
const PACKAGES: Read = PROJECTS.at(
    "vnetwork/v1/{}/{}/endpoints/{}/packages",
    Style::EndpointQuote,
);
const VPCS: Read = PROJECTS.at("vnetwork/v1/{}/{}/endpoints/vpcs", Style::EndpointQuote);
const SUBNETS: Read = PROJECTS.at(
    "vnetwork/v1/{}/{}/endpoints/vpcs/{}/subnets",
    Style::EndpointQuote,
);
const ORIGIN: &str = "https://hcm-3-vnetwork.console.greennode.ai";
const OTHER_ORIGIN: &str = "https://han-1-vnetwork.console.greennode.ai";

pub(super) struct EndpointQuote<'a> {
    pub(super) client: &'a Client,
    pub(super) ctx: &'a RequestContext,
    pub(super) evidence: &'a mut Evidence,
    pub(super) base: Base,
    pub(super) project: String,
    pub(super) zone: String,
}

pub(super) struct Offer {
    pub(super) category: String,
    pub(super) service: String,
    pub(super) package: String,
    pub(super) package_name: String,
    pub(super) zone: String,
    pub(super) service_name: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Page {
    size: i64,
    total: i64,
    pages: i64,
}

#[derive(Serialize)]
struct Filter<'a> {
    field: &'a str,
    value: &'a str,
}

struct Scan<'a> {
    read: Read,
    suffix: &'a [&'a str],
    filters: &'a [(&'a str, &'a str)],
    query: &'a [(&'a str, &'a str)],
    required: &'a [&'a str],
    require_paging: bool,
}

impl<'a> Scan<'a> {
    fn new(read: Read, required: &'a [&'a str]) -> Self {
        Self {
            read,
            suffix: &[],
            filters: &[],
            query: &[],
            required,
            require_paging: false,
        }
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl<'a> EndpointQuote<'a> {
    pub(super) async fn new(
        client: &'a Client,
        ctx: &'a RequestContext,
        evidence: &'a mut Evidence,
        write: bool,
    ) -> Result<Self, Error> {
        let session = &client.session;
        let endpoints = session.endpoints();
        let defaults = session.default_endpoints();
        let vserver = endpoints.vserver.as_ref();
        let vnetwork = endpoints.vnetwork.as_ref();
        let is_default = |current: Option<&Base>, default: Option<&Base>| {
            current.zip(default).is_some_and(|(current, default)| {
                current.configured_url() == default.configured_url()
            })
        };
        let trusted_fixture = {
            #[cfg(any(test, feature = "test-support"))]
            {
                use crate::testing::Product;
                let fixture = |product| {
                    if write {
                        session.is_write_fixture(product)
                    } else {
                        session.has_product_fixture(product)
                    }
                };
                fixture(Product::Vserver) && fixture(Product::Vnetwork)
            }
            #[cfg(not(any(test, feature = "test-support")))]
            {
                let _ = write;
                false
            }
        };
        let refused = || {
            local_error(
                ErrorClass::InvalidConfig,
                " requires hcm-3, SDK IAM-user login, and default endpoints",
            )
        };
        if client.region != "hcm-3"
            || !(trusted_fixture
                || (session.auth_kind() == AuthKind::IamUser
                    && session.uses_default_auth_endpoints()
                    && is_default(vserver, defaults.vserver.as_ref())
                    && is_default(vnetwork, defaults.vnetwork.as_ref())
                    && vnetwork.is_some_and(|base| !base.is_explicit_override())))
        {
            return Err(refused());
        }
        let vserver = vserver.ok_or_else(refused)?;
        let base = if trusted_fixture {
            Base::parse(vnetwork.ok_or_else(refused)?.configured_url()).ok()
        } else {
            vnetwork_regional_gateway("hcm-3")
        }
        .ok_or_else(refused)?;
        let configured = session.configured_project_id();
        if configured.is_some_and(|id| evidence.id_reflects(id)) {
            return Err(Error::new(ErrorClass::InvalidInput));
        }
        let project_body = client.read(ctx, vserver, PROJECTS, &[], evidence).await?;
        let project = project(&go_json(&project_body), configured, evidence)?;
        let project = super::mutations::guard_operands(PROJECTS.id, project, evidence)?;
        let zone = if write {
            let zone_body = client.read(ctx, &base, REGIONS, &[], evidence).await?;
            let zone = region(&go_json(&zone_body), evidence)?;
            super::mutations::guard_operands(REGIONS.id, zone, evidence)?
        } else if let Some(zone) = client.endpoint_verified_zone.get() {
            zone.clone()
        } else {
            let zone_body = client.read(ctx, &base, REGIONS, &[], evidence).await?;
            let zone = region(&go_json(&zone_body), evidence)?;
            if client.endpoint_verified_zone.get_or_init(|| zone.clone()) != &zone {
                return Err(REGIONS.invalid(evidence));
            }
            let _ = client.endpoint_zone.set(zone.clone());
            zone
        };
        if evidence.id_reflects(&project) || evidence.id_reflects(&zone) {
            return Err(PROJECTS.invalid(evidence));
        }
        Ok(Self {
            client,
            ctx,
            evidence,
            base,
            project,
            zone,
        })
    }

    pub(super) async fn offer(&mut self, input: &QuoteCreateEndpointInput) -> Result<Offer, Error> {
        let zone = default(&input.availability_zone_id, "HCM03-1C");
        let zones = self
            .scan(Scan::new(ZONES, &["uuid", "zoneType", "isEnabled"]))
            .await?;
        let found = one(&zones, "uuid", zone, "zone is not eligible exactly once")?;
        if !get::<bool>(found, "isEnabled")? || get::<String>(found, "zoneType")? != "AVAILABILITY"
        {
            return Err(refusal("zone is not eligible exactly once"));
        }
        let categories = self
            .scan(Scan::new(CATEGORIES, &["uuid", "isDefault"]))
            .await?;
        let category = if input.category_id.is_empty() {
            let [row] = categories.as_slice() else {
                return Err(refusal("category is not offered exactly once"));
            };
            if !get::<bool>(row, "isDefault")? {
                return Err(refusal("category is not offered exactly once"));
            }
            get::<String>(row, "uuid")?
        } else {
            get::<String>(
                one(
                    &categories,
                    "uuid",
                    &input.category_id,
                    "category is not offered exactly once",
                )?,
                "uuid",
            )?
        };
        if !hex24(&category) {
            return Err(refusal("category is not offered exactly once"));
        }
        let services = self
            .scan(Scan {
                suffix: &[&category],
                query: &[("zoneUuid", zone)],
                ..Scan::new(SERVICES, &["uuid", "name", "endpointType"])
            })
            .await?;
        let service = one(
            &services,
            if input.service_id.is_empty() {
                "name"
            } else {
                "uuid"
            },
            default(&input.service_id, "vstorage.hcm03"),
            "service is not offered exactly once",
        )?;
        let service_name = get::<String>(service, "name")?;
        let service = if get::<String>(service, "endpointType")? == "Gateway"
            && !service_name.trim().is_empty()
        {
            get::<String>(service, "uuid")?
        } else {
            return Err(refusal("service is not offered exactly once"));
        };
        if !uuid(&service) {
            return Err(refusal("service is not offered exactly once"));
        }
        let packages = self
            .scan(Scan {
                suffix: &[&service],
                ..Scan::new(PACKAGES, &["uuid", "name", "serviceName", "currencyUnit"])
            })
            .await?;
        let selection = one(
            &packages,
            if input.package_id.is_empty() {
                "name"
            } else {
                "uuid"
            },
            default(&input.package_id, "Standard"),
            "package is not offered exactly once",
        )?;
        let package = get::<String>(selection, "uuid")?;
        let package_name = get::<String>(selection, "name")?;
        if !uuid(&package)
            || package_name.trim().is_empty()
            || get::<String>(selection, "serviceName")? != service_name
            || get::<String>(selection, "currencyUnit")? != "VND"
        {
            return Err(refusal("package is not offered exactly once"));
        }
        one(
            &packages,
            "name",
            &package_name,
            "package is not offered exactly once",
        )?;
        Ok(Offer {
            category,
            service,
            package,
            package_name,
            zone: zone.into(),
            service_name,
        })
    }

    pub(super) async fn check_placement(
        &mut self,
        input: &QuoteCreateEndpointInput,
        offer: &Offer,
    ) -> Result<(), Error> {
        let vpcs = self
            .scan(Scan {
                require_paging: true,
                filters: &[
                    ("status", "ACTIVE"),
                    ("endpointServiceId", &offer.service),
                    ("zoneType", "AVAILABILITY"),
                ],
                ..Scan::new(VPCS, &["uuid", "status", "projectUuid", "regionUuid"])
            })
            .await?;
        for row in &vpcs {
            if !get::<String>(row, "uuid")?
                .strip_prefix("net-")
                .is_some_and(uuid)
            {
                return Err(VPCS.invalid(self.evidence));
            }
        }
        let vpc = find(&vpcs, &input.vpc_id).ok_or_else(|| {
            local_error(
                ErrorClass::NotFound,
                ": VPC is not eligible for the endpoint offer",
            )
        })?;
        if get::<String>(vpc, "status")? != "ACTIVE"
            || get::<String>(vpc, "projectUuid")? != self.project
            || get::<String>(vpc, "regionUuid")? != self.zone
        {
            return Err(refusal("VPC is not an active VPC of the selected scope"));
        }
        let subnets = self
            .scan(Scan {
                suffix: &[&input.vpc_id],
                filters: &[("isEndpointUsable", "true"), ("zoneUuid", &offer.zone)],
                ..Scan::new(SUBNETS, &["uuid", "status", "vpcUuid", "zone"])
            })
            .await?;
        for row in &subnets {
            if !get::<String>(row, "uuid")?
                .strip_prefix("sub-")
                .is_some_and(uuid)
            {
                return Err(SUBNETS.invalid(self.evidence));
            }
        }
        let subnet = find(&subnets, &input.subnet_id).ok_or_else(|| {
            local_error(
                ErrorClass::NotFound,
                ": subnet is not eligible for the endpoint offer",
            )
        })?;
        let zone: Fields<'_> = get(subnet, "zone")?;
        if get::<String>(subnet, "status")? != "ACTIVE"
            || get::<String>(subnet, "vpcUuid")? != input.vpc_id
            || zone
                .required::<String>("uuid")
                .map_err(|()| SUBNETS.invalid(self.evidence))?
                != offer.zone
        {
            return Err(refusal(
                "subnet is not an active subnet of the VPC in the zone",
            ));
        }
        Ok(())
    }

    pub(super) async fn endpoint_inventory(
        &mut self,
        operation: &'static str,
    ) -> Result<Vec<Box<RawValue>>, Error> {
        self.scan(Scan {
            require_paging: true,
            ..Scan::new(
                Read::new(
                    operation,
                    "vnetwork/v1/{}/{}/endpoints",
                    Style::EndpointQuote,
                ),
                &[],
            )
        })
        .await
    }

    async fn scan(&mut self, spec: Scan<'_>) -> Result<Vec<Box<RawValue>>, Error> {
        let ctx = self.ctx.child_with_deadline(
            self.client.session.clock().monotonic_now() + Duration::from_secs(30),
        );
        let mut seen = HashSet::new();
        let mut rows = Vec::new();
        let mut previous = None;
        let mut retained = 0_usize;
        let path: Vec<_> = [self.zone.as_str(), self.project.as_str()]
            .into_iter()
            .chain(spec.suffix.iter().copied())
            .collect();
        let filters: Vec<_> = spec
            .filters
            .iter()
            .map(|(field, value)| Filter { field, value })
            .collect();
        let filters =
            serde_json::to_string(&filters).map_err(|_| Error::new(ErrorClass::InvalidInput))?;
        for page in 1..=50 {
            let params = format!(r#"{{"search":{filters},"sort":{{}},"page":{page},"size":1000}}"#);
            let query: Vec<_> = [("params", params.as_str())]
                .into_iter()
                .chain(spec.query.iter().copied())
                .collect();
            let body = self
                .client
                .read_query(
                    &ctx,
                    &self.base,
                    spec.read,
                    Selection {
                        path: &path,
                        query: &query,
                        headers: &[],
                    },
                    self.evidence,
                )
                .await?;
            let invalid = || spec.read.invalid(self.evidence);
            let text = go_json(&body);
            let envelope = endpoint_envelope(&text, self.evidence).map_err(|()| invalid())?;
            if !envelope.only(&[
                "message",
                "code",
                "success",
                "data",
                "page",
                "size",
                "total",
                "totalPage",
            ]) {
                return Err(invalid());
            }
            let info = page_info(&envelope, page, spec.require_paging).map_err(|()| invalid())?;
            if page > 1 && previous != info {
                return Err(invalid());
            }
            previous = info;
            let data: Vec<&RawValue> = match envelope.raw("data").map_err(|()| invalid())? {
                Some(raw) => serde_json::from_str(raw.get()).map_err(|_| invalid())?,
                None if info.is_some_and(|info| info.total == 0) => Vec::new(),
                None => return Err(invalid()),
            };
            let count = i64::try_from(data.len()).map_err(|_| invalid())?;
            if info.is_some_and(|info| {
                let expected = if info.total == 0 {
                    0
                } else if page == info.pages {
                    info.total - info.size * (info.pages - 1)
                } else {
                    info.size
                };
                count != expected
            }) {
                return Err(invalid());
            }
            let data = super::mutations::guard_operands(spec.read.id, data, self.evidence)?;
            let invalid = || spec.read.invalid(self.evidence);
            for raw in data {
                let row = fields(raw).map_err(|()| invalid())?;
                for key in spec.required {
                    let valid = match *key {
                        "isEnabled" | "isDefault" => row.required::<bool>(key).is_ok(),
                        "zone" => row
                            .required::<Fields<'_>>(key)
                            .and_then(|zone| zone.required::<String>("uuid"))
                            .is_ok(),
                        _ => row.required::<String>(key).is_ok(),
                    };
                    if !valid {
                        return Err(invalid());
                    }
                }
                let id: String = row.required("uuid").map_err(|()| invalid())?;
                if !PathIds::Strict.allows(&id)
                    || self.evidence.id_reflects(&id)
                    || !seen.insert(id)
                {
                    return Err(invalid());
                }
                retained = retained.saturating_add(raw.get().len().saturating_add(128));
                if retained > 4 << 20 {
                    return Err(invalid());
                }
                rows.push(raw.to_owned());
            }
            if info.is_none_or(|info| page >= info.pages) {
                return Ok(rows);
            }
        }
        Err(spec.read.invalid(self.evidence))
    }
}

fn page_info(envelope: &Fields<'_>, page: i64, require: bool) -> Result<Option<Page>, ()> {
    if !envelope.has_any(&["page", "size", "total", "totalPage"]) {
        return if require || page != 1 {
            Err(())
        } else {
            Ok(None)
        };
    }
    let value = |name| envelope.required::<Field<Option<i64>>>(name)?.0.ok_or(());
    let size = value("size")?;
    let total = value("total")?;
    let pages = value("totalPage")?;
    if value("page")? != page
        || size <= 0
        || total < 0
        || !(0..=50).contains(&pages)
        || total / size + i64::from(total % size != 0) != pages
        || (page > pages && total != 0)
    {
        return Err(());
    }
    Ok(Some(Page { size, total, pages }))
}

fn endpoint_envelope<'a>(text: &'a str, evidence: &Evidence) -> Result<Fields<'a>, ()> {
    if reflects(text, evidence) {
        return Err(());
    }
    let fields = Fields::decode(text)?;
    if !fields.required::<bool>("success")? {
        return Err(());
    }
    for field in ["code", "page", "size", "total", "totalPage"] {
        fields.raw(field)?;
    }
    Ok(fields)
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn project(text: &str, configured: Option<&str>, evidence: &Evidence) -> Result<String, Error> {
    let invalid = || PROJECTS.invalid(evidence);
    if reflects(text, evidence) {
        return Err(invalid());
    }
    let fields = Fields::decode(text).map_err(|()| invalid())?;
    if fields.has_any(PAGING) {
        return Err(invalid());
    }
    let rows: Vec<Fields<'_>> = fields.required("projects").map_err(|()| invalid())?;
    let mut selected = None;
    for row in rows {
        let id: String = row
            .optional("projectId")
            .map_err(|()| invalid())?
            .unwrap_or_default();
        let region: String = row
            .optional("region")
            .map_err(|()| invalid())?
            .unwrap_or_default();
        if (!region.is_empty() && region != "hcm-3") || configured.is_some_and(|value| value != id)
        {
            continue;
        }
        if !id.strip_prefix("pro-").is_some_and(uuid) || evidence.id_reflects(&id) {
            return Err(invalid());
        }
        if selected.replace(id).is_some() {
            return Err(Error::new(ErrorClass::ProjectAmbiguous));
        }
    }
    selected.ok_or_else(|| Error::new(ErrorClass::ProjectNotFound))
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn region(text: &str, evidence: &Evidence) -> Result<String, Error> {
    let invalid = || REGIONS.invalid(evidence);
    let fields = endpoint_envelope(text, evidence).map_err(|()| invalid())?;
    let rows: Vec<Fields<'_>> = fields.required("data").map_err(|()| invalid())?;
    let mut selected = None;
    for row in rows {
        let dashboard: String = row
            .optional::<Option<_>>("vnetworkDashboard")
            .map_err(|()| invalid())?
            .flatten()
            .unwrap_or_default();
        let gateway: String = row
            .optional::<Option<_>>("gatewayUrl")
            .map_err(|()| invalid())?
            .flatten()
            .unwrap_or_default();
        let matches = |origin| {
            dashboard.trim_end_matches('/') == origin
                || gateway
                    .trim_end_matches('/')
                    .strip_suffix("/vnetwork-gateway")
                    == Some(origin)
        };
        if !matches(ORIGIN) {
            continue;
        }
        if matches(OTHER_ORIGIN) {
            return Err(local_error(
                ErrorClass::InvalidConfig,
                ": conflicting region mapping",
            ));
        }
        let id: String = row.required("uuid").map_err(|()| invalid())?;
        if !hex24(&id) || evidence.id_reflects(&id) {
            return Err(invalid());
        }
        if selected.replace(id).is_some() {
            return Err(local_error(
                ErrorClass::InvalidConfig,
                ": no unique region mapping",
            ));
        }
    }
    selected.ok_or_else(|| local_error(ErrorClass::InvalidConfig, ": no unique region mapping"))
}

pub(super) fn hex24(value: &str) -> bool {
    value.len() == 24 && value.bytes().all(lower_hex)
}

pub(super) fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, byte)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                lower_hex(byte)
            }
        })
}

fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
}

fn default<'a>(value: &'a str, default: &'a str) -> &'a str {
    if value.is_empty() { default } else { value }
}

fn refusal(reason: &str) -> Error {
    local_error(ErrorClass::InvalidInput, &format!(": {reason}"))
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn get<'a, T: serde::Deserialize<'a>>(raw: &'a RawValue, name: &str) -> Result<T, Error> {
    fields(raw)
        .and_then(|row| row.required(name))
        .map_err(|()| PROJECTS.invalid(&Evidence::new()))
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn one<'a>(
    rows: &'a [Box<RawValue>],
    field: &str,
    value: &str,
    reason: &str,
) -> Result<&'a RawValue, Error> {
    let mut selected = None;
    for row in rows {
        if get::<String>(row, field)? == value && selected.replace(row.as_ref()).is_some() {
            return Err(refusal(reason));
        }
    }
    selected.ok_or_else(|| refusal(reason))
}

fn find<'a>(rows: &'a [Box<RawValue>], id: &str) -> Option<&'a RawValue> {
    rows.iter()
        .find(|raw| get::<String>(raw, "uuid").is_ok_and(|uuid| uuid == id))
        .map(Box::as_ref)
}
