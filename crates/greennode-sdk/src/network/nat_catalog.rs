use serde::Serialize;
use serde_json::value::RawValue;

use crate::endpoint::Base;
use crate::model::wire::{Field, WireField, go_json};
use crate::wire::product::{evidence::Evidence, options::PathIds, response};
use crate::{ApiError, Error, ErrorClass, Failure, List, PagedList, RequestContext};

use super::inventory::{Selection, screen};
use super::wire::{Fields, Read, Style};
use super::{Client, NatAvailabilityZone, NatInstance, NatPackageOffer, failure};

const INSTANCES: Read = Read::new(
    "network.ListNATInstances",
    "vnetwork/v1/{}/{}/nats",
    Style::NatList,
);
const ZONES: Read = Read::new(
    "network.ListNATZones",
    "vnetwork/v1/{}/{}/nats/zones",
    Style::NatCatalog,
);
const PACKAGES: Read = Read::new(
    "network.ListNATPackages",
    "vnetwork/v1/{}/{}/nats/nat-package",
    Style::NatCatalog,
);

/// Selects one page of Public NAT inventory.
#[derive(Clone, Default)]
pub struct ListNatInstancesInput {
    /// Regional zone identifier. An empty value discovers the zone.
    pub zone_id: String,
    /// One-based page. Zero selects page one.
    pub page: i64,
    /// Page size. Zero selects ten.
    pub size: i64,
}

crate::error::debug_record!(ListNatInstancesInput {
    zone_id,
    page,
    size
});

/// Inventory rows and pagination for one Public NAT page.
pub type ListNatInstancesOutput = PagedList<NatInstance>;

/// Selects the regional Public NAT availability-zone catalog.
#[derive(Clone, Default)]
pub struct ListNatZonesInput {
    /// Regional zone identifier. An empty value discovers the zone.
    pub zone_id: String,
}

crate::error::debug_record!(ListNatZonesInput { zone_id });

/// Public NAT availability-zone catalog rows.
pub type ListNatZonesOutput = List<NatAvailabilityZone>;

/// Selects the Public NAT package catalog for one availability zone.
#[derive(Clone)]
pub struct ListNatPackagesInput {
    /// Regional zone identifier. An empty value discovers the zone.
    pub zone_id: String,
    /// Availability-zone identifier.
    pub availability_zone_id: String,
}

crate::error::debug_record!(ListNatPackagesInput {
    zone_id,
    availability_zone_id
});

impl ListNatPackagesInput {
    /// Selects a package catalog by availability-zone identifier.
    pub fn new(availability_zone_id: impl Into<String>) -> Self {
        Self {
            zone_id: String::new(),
            availability_zone_id: availability_zone_id.into(),
        }
    }
}

/// Public NAT package catalog rows.
pub type ListNatPackagesOutput = List<NatPackageOffer>;

pub(super) struct NatScope {
    pub(super) base: Base,
    pub(super) project: String,
    pub(super) zone: String,
}

pub(super) fn params(page: i64, size: i64) -> String {
    format!(r#"{{"search":[],"sort":{{}},"page":{page},"size":{size}}}"#)
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Reads one page of Public NAT instances.
    pub async fn list_nat_instances(
        &self,
        ctx: &RequestContext,
        input: ListNatInstancesInput,
    ) -> Result<ListNatInstancesOutput, Failure<ListNatInstancesOutput>> {
        crate::wire::product::exit::finish!(async {
            if input.page < 0 || input.size < 0 {
                return Err(failure(Error::new(ErrorClass::InvalidInput).with_cause(crate::quote::request::api("", None, "InvalidUsage", "vngcloud: invalid input: network.ListNATInstances requires nonnegative Page and Size", false))));
            }
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            let scope = self
                .nat_scope(ctx, INSTANCES, input.zone_id, &mut evidence, true)
                .await
                .map_err(failure)?;
            let params = params(
                input.page.max(1),
                if input.size == 0 { 10 } else { input.size },
            );
            let body = self
                .nat_catalog_request(
                    ctx,
                    INSTANCES,
                    &scope,
                    &[("params", &params)],
                    &mut evidence,
                )
                .await
                .map_err(failure)?;
            let text = go_json(&body);
            decode_instances(&text, &evidence).map_err(failure)
        })
    }

    /// Lists the Public NAT availability-zone catalog.
    pub async fn list_nat_zones(
        &self,
        ctx: &RequestContext,
        input: ListNatZonesInput,
    ) -> Result<ListNatZonesOutput, Failure<ListNatZonesOutput>> {
        crate::wire::product::exit::finish!(async {
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            let scope = self
                .nat_scope(ctx, ZONES, input.zone_id, &mut evidence, false)
                .await
                .map_err(failure)?;
            let items = self
                .nat_zones(ctx, ZONES.id, &scope, &mut evidence)
                .await
                .map_err(failure)?;
            Ok(List { items })
        })
    }

    /// Lists Public NAT packages for the selected availability zone.
    pub async fn list_nat_packages(
        &self,
        ctx: &RequestContext,
        input: ListNatPackagesInput,
    ) -> Result<ListNatPackagesOutput, Failure<ListNatPackagesOutput>> {
        crate::wire::product::exit::finish!(async {
            PACKAGES
                .path("AvailabilityZoneID", &input.availability_zone_id)
                .map_err(failure)?;
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            if evidence.id_reflects(&input.availability_zone_id) {
                return Err(failure(Error::new(ErrorClass::InvalidInput)));
            }
            let scope = self
                .nat_scope(ctx, PACKAGES, input.zone_id, &mut evidence, false)
                .await
                .map_err(failure)?;
            let items = self
                .nat_packages(
                    ctx,
                    PACKAGES.id,
                    &scope,
                    &input.availability_zone_id,
                    &mut evidence,
                )
                .await
                .map_err(failure)?;
            Ok(List { items })
        })
    }

    pub(super) async fn nat_scope(
        &self,
        ctx: &RequestContext,
        read: Read,
        mut zone: String,
        evidence: &mut Evidence,
        use_shared_project: bool,
    ) -> Result<NatScope, Error> {
        if !zone.is_empty() {
            read.path("ZoneID", &zone)?;
            if evidence.id_reflects(&zone) {
                return Err(Error::new(ErrorClass::InvalidInput));
            }
        }
        let configured = self
            .session
            .project_id_context(ctx)
            .await
            .map_err(|error| *error)?;
        if let Some(project) = &configured {
            read.path("ProjectID", project)?;
            if evidence.id_reflects(project) {
                return Err(Error::new(ErrorClass::InvalidInput));
            }
        }
        let base = self.nat_base()?;
        let project = match configured {
            Some(project) => project,
            None if use_shared_project => self
                .session
                .require_project_id_with_evidence(ctx, evidence)
                .await
                .map_err(|error| screen(*error, evidence))?,
            None => {
                let vserver = self
                    .session
                    .endpoints()
                    .vserver
                    .as_ref()
                    .ok_or_else(|| Error::new(ErrorClass::InvalidConfig))?;
                self.discover_project(ctx, vserver, read.at("v1/projects", Style::Nat), evidence)
                    .await?
            }
        };
        read.path("ProjectID", &project)?;
        if zone.is_empty() {
            zone = self.nat_zone(ctx, &base, evidence).await?;
        }
        if evidence.id_reflects(&project) || evidence.id_reflects(&zone) {
            return Err(catalog_invalid(read, evidence));
        }
        Ok(NatScope {
            base,
            project,
            zone,
        })
    }

    pub(super) async fn nat_catalog_request(
        &self,
        ctx: &RequestContext,
        read: Read,
        scope: &NatScope,
        query: &[(&str, &str)],
        evidence: &mut Evidence,
    ) -> Result<Vec<u8>, Error> {
        self.read_query(
            ctx,
            &scope.base,
            read,
            Selection {
                path: &[&scope.zone, &scope.project],
                query,
                headers: &[],
            },
            evidence,
        )
        .await
    }

    pub(super) async fn nat_zones(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        scope: &NatScope,
        evidence: &mut Evidence,
    ) -> Result<Vec<NatAvailabilityZone>, Error> {
        let read = Read { id: op, ..ZONES };
        let params = params(1, 1000);
        let body = self
            .nat_catalog_request(ctx, read, scope, &[("params", &params)], evidence)
            .await?;
        let text = go_json(&body);
        let invalid = || catalog_invalid(read, evidence);
        let raw = catalog_data(&text, 200, evidence).map_err(|()| invalid())?;
        decode_rows(
            raw,
            &["uuid", "zoneType", "isEnabled", "isDefault"],
            |row, item: &NatAvailabilityZone| {
                check_nulls(row, &["name", "description"])?;
                if PathIds::WithUnderscore.allows(&item.uuid)
                    && !evidence.id_reflects(&item.uuid)
                    && !item.zone_type.is_empty()
                {
                    Ok(())
                } else {
                    Err(())
                }
            },
            evidence,
        )
        .map_err(|()| invalid())
    }

    pub(super) async fn nat_packages(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        scope: &NatScope,
        az: &str,
        evidence: &mut Evidence,
    ) -> Result<Vec<NatPackageOffer>, Error> {
        let read = Read { id: op, ..PACKAGES };
        let params = params(1, 1000);
        let body = self
            .nat_catalog_request(
                ctx,
                read,
                scope,
                &[("params", &params), ("zoneUuid", az)],
                evidence,
            )
            .await?;
        let text = go_json(&body);
        let invalid = || catalog_invalid(read, evidence);
        let raw = catalog_data(&text, 200, evidence).map_err(|()| invalid())?;
        decode_rows(
            raw,
            &[
                "uuid",
                "billingSku",
                "currencyUnit",
                "isDefault",
                "monthlyPrice",
                "price",
            ],
            |row, item: &NatPackageOffer| {
                check_nulls(
                    row,
                    &[
                        "name",
                        "packageId",
                        "resourceServiceId",
                        "serviceName",
                        "createdAt",
                    ],
                )?;
                require_nonnull(
                    &row.required("price")?,
                    &[
                        "optimumPrice",
                        "originalPrice",
                        "discountPrice",
                        "discountPercent",
                    ],
                )?;
                if PathIds::WithUnderscore.allows(&item.uuid)
                    && !evidence.id_reflects(&item.uuid)
                    && !item.billing_sku.is_empty()
                {
                    Ok(())
                } else {
                    Err(())
                }
            },
            evidence,
        )
        .map_err(|()| invalid())
    }
}

pub(super) fn catalog_invalid(read: Read, evidence: &Evidence) -> Error {
    read.invalid_as(evidence, "NAT response had invalid structure")
}

pub(super) fn catalog_data<'a>(
    text: &'a str,
    expected_code: i64,
    evidence: &Evidence,
) -> Result<&'a RawValue, ()> {
    if reflects(text, evidence) {
        return Err(());
    }
    let fields = Fields::decode(text)?;
    let code: &RawValue = fields.required("code")?;
    if !fields.required::<bool>("success")? || !numeric_code(code.get(), expected_code) {
        return Err(());
    }
    fields
        .raw("data")?
        .filter(|raw| raw.get() != "null")
        .ok_or(())
}

fn numeric_code(raw: &str, expected: i64) -> bool {
    if raw.len() > 128 || !matches!(raw.as_bytes().first(), Some(b'-' | b'0'..=b'9')) {
        return false;
    }
    let (mantissa, exponent) = raw.split_once(['e', 'E']).unwrap_or((raw, "0"));
    let fraction = mantissa
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    let mut digits = mantissa.replace('.', "");
    if expected == 0 {
        return digits
            .trim_start_matches('-')
            .bytes()
            .all(|byte| byte == b'0');
    }
    if digits.starts_with('-') {
        return false;
    }
    let significant = digits.trim_end_matches('0').len();
    let zeros = digits.len() - significant;
    digits.truncate(significant);
    digits.trim_start_matches('0') == "2"
        && exponent.parse::<i64>().is_ok_and(|exp| {
            exp == 2 + i64::try_from(fraction).unwrap_or_default()
                - i64::try_from(zeros).unwrap_or_default()
        })
}

pub(super) fn decode_rows<T: WireField + Serialize>(
    raw: &RawValue,
    required: &[&str],
    validate: impl Fn(&Fields<'_>, &T) -> Result<(), ()>,
    evidence: &Evidence,
) -> Result<Vec<T>, ()> {
    let raw_rows: Vec<&RawValue> = serde_json::from_str(raw.get()).map_err(|_| ())?;
    raw_rows
        .into_iter()
        .map(|raw| {
            let fields: Fields<'_> = serde_json::from_str(raw.get()).map_err(|_| ())?;
            require_nonnull(&fields, required)?;
            let mut row: T = serde_json::from_str(raw.get()).map_err(|_| ())?;
            validate(&fields, &row)?;
            if reflects(&serde_json::to_string(&row).map_err(|_| ())?, evidence)
                || !row.redact(evidence)
            {
                return Err(());
            }
            Ok(row)
        })
        .collect()
}

fn require_nonnull(fields: &Fields<'_>, names: &[&str]) -> Result<(), ()> {
    for name in names {
        if fields.raw(name)?.is_none_or(|raw| raw.get() == "null") {
            return Err(());
        }
    }
    Ok(())
}

pub(super) fn check_nulls(fields: &Fields<'_>, names: &[&str]) -> Result<(), ()> {
    for name in names {
        if fields.raw(name)?.is_some_and(|raw| raw.get() == "null") {
            return Err(());
        }
    }
    Ok(())
}

fn check_null_elements(fields: &Fields<'_>, name: &str) -> Result<(), ()> {
    let Some(raw) = fields.raw(name)? else {
        return Ok(());
    };
    match serde_json::from_str::<Vec<Option<serde::de::IgnoredAny>>>(raw.get()) {
        Ok(items) if items.iter().any(Option::is_none) => Err(()),
        _ => Ok(()),
    }
}

pub(super) fn reflects(text: &str, evidence: &Evidence) -> bool {
    evidence.peer_body_reflects(text.as_bytes()) != Some(false)
        || response::reflects_credential_forms(text, evidence.credentials())
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn decode_instances(text: &str, evidence: &Evidence) -> Result<ListNatInstancesOutput, Error> {
    let invalid = |message| INSTANCES.invalid_as(evidence, message);
    if reflects(text, evidence) {
        return Err(invalid("invalid list envelope"));
    }
    let fields = Fields::decode(text).map_err(|()| invalid("invalid list envelope"))?;
    let success = fields
        .optional::<Option<bool>>("success")
        .map_err(|()| invalid("invalid list envelope"))?
        .flatten();
    let code = fields
        .optional::<Field<String>>("code")
        .map_err(|()| invalid("invalid list envelope"))?
        .map_or_else(String::new, |field| field.0);
    let message = fields
        .optional::<Field<String>>("message")
        .map_err(|()| invalid("invalid list envelope"))?
        .map_or_else(String::new, |field| field.0);
    match success {
        None => return Err(invalid("missing success")),
        Some(false) => {
            return Err(screen(
                Error::from_api(ApiError::without_fallback(
                    INSTANCES.id,
                    Some(200),
                    response::sanitize_error_text(&code, evidence.credentials()),
                    response::sanitize_error_text(&message, evidence.credentials()),
                    false,
                )),
                evidence,
            ));
        }
        Some(true) => (),
    }
    let count = |name, minimum| -> Result<i64, Error> {
        fields
            .optional::<Field<Option<i64>>>(name)
            .map_err(|()| invalid("invalid list envelope"))?
            .and_then(|field| field.0)
            .filter(|&value| value >= minimum && !evidence.text_reflects(&value.to_string()))
            .ok_or_else(|| invalid("invalid page metadata"))
    };
    let page = count("page", 1)?;
    let page_size = count("size", 1)?;
    let total_page = count("totalPage", 0)?;
    let total_item = count("total", 0)?;
    let items = match fields
        .raw("data")
        .map_err(|()| invalid("invalid list envelope"))?
    {
        None if total_page == 0 && total_item == 0 => Vec::new(),
        None => return Err(invalid("missing data for nonempty list")),
        Some(data) if data.get() == "null" => return Err(invalid("null data")),
        Some(data) => decode_rows(
            data,
            &[],
            |fields, _: &NatInstance| {
                if let Some(package) = fields.optional::<Fields<'_>>("natPackage")? {
                    check_nulls(
                        &package,
                        &[
                            "id",
                            "uuid",
                            "name",
                            "createdAt",
                            "packageId",
                            "default",
                            "image",
                        ],
                    )?;
                    if let Some(image) = package.optional::<Fields<'_>>("image")? {
                        check_nulls(
                            &image,
                            &[
                                "id",
                                "uuid",
                                "imageType",
                                "imageVersion",
                                "licence",
                                "flavorZoneIds",
                                "packageLimit",
                            ],
                        )?;
                        check_null_elements(&image, "flavorZoneIds")?;
                        if let Some(limit) = image.optional::<Fields<'_>>("packageLimit")? {
                            check_nulls(&limit, &["cpu", "memory", "diskSize"])?;
                        }
                    }
                }
                if let Some(vpc) = fields.optional::<Fields<'_>>("vpc")? {
                    check_nulls(
                        &vpc,
                        &[
                            "uuid",
                            "name",
                            "cidr",
                            "status",
                            "regionId",
                            "projectId",
                            "lastSyncTime",
                            "dnsStatus",
                        ],
                    )?;
                }
                check_nulls(
                    fields,
                    &[
                        "uuid",
                        "natName",
                        "status",
                        "createdAt",
                        "updatedAt",
                        "projectUuid",
                        "zoneUuid",
                        "natPackage",
                        "vpc",
                    ],
                )
            },
            evidence,
        )
        .map_err(|()| invalid("invalid NAT rows"))?,
    };
    for item in &items {
        if !PathIds::WithUnderscore.allows(&item.uuid) || evidence.id_reflects(&item.uuid) {
            return Err(invalid("missing NAT uuid"));
        }
        let package = &item.nat_package;
        let image = &package.image;
        let ids = [
            &item.project_uuid,
            &item.zone_uuid,
            &package.id,
            &package.uuid,
            &package.package_id,
            &image.id,
            &image.uuid,
            &item.vpc.uuid,
            &item.vpc.region_id,
            &item.vpc.project_id,
        ]
        .into_iter()
        .chain(&image.flavor_zone_ids);
        if ids.into_iter().any(|id| evidence.id_reflects(id)) {
            return Err(invalid("invalid NAT identifier"));
        }
    }
    Ok(PagedList {
        items,
        page,
        page_size,
        total_page,
        total_item,
    })
}
