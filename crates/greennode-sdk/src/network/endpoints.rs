use std::time::Duration;

use serde::Serialize;

use crate::endpoint::{Base, vnetwork_regional_gateway};
use crate::model::wire::{Field, go_json};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, ErrorClass, Failure, List, PagedList, RequestContext};

use super::inventory::{InventoryRead, InventoryStyle, Selection, screen};
use super::wire::Fields;
use super::{Client, Endpoint, EndpointTag, VNetworkRegion, failure};

const REGIONS: InventoryRead = InventoryRead {
    id: "network.ListVNetworkRegions",
    template: "vnetwork/v1/regions",
    style: InventoryStyle::Region,
};
const DETAIL: InventoryRead = InventoryRead {
    id: "network.GetEndpoint",
    template: "vnetwork/v1/{}/{}/endpoints/{}",
    style: InventoryStyle::Plain,
};
const LIST: InventoryRead = InventoryRead {
    id: "network.ListEndpoints",
    template: "vnetwork/v1/{}/{}/endpoints",
    style: InventoryStyle::Plain,
};
const TAGS: InventoryRead = InventoryRead {
    id: "network.ListEndpointTags",
    template: "vnetwork/v1/{}/tags",
    style: InventoryStyle::Plain,
};

/// Selects the configured region's vNetwork inventory.
#[derive(Clone, Debug, Default)]
pub struct ListVNetworkRegionsInput {}

/// Regions returned by the vNetwork gateway.
pub type ListVNetworkRegionsOutput = List<VNetworkRegion>;

/// Selects a regional endpoint.
#[derive(Clone, Debug)]
pub struct GetEndpointInput {
    /// Endpoint identifier.
    pub endpoint_id: String,
}

impl GetEndpointInput {
    /// Selects an endpoint by identifier.
    pub fn new(endpoint_id: impl Into<String>) -> Self {
        Self {
            endpoint_id: endpoint_id.into(),
        }
    }
}

/// Endpoint detail with the same fields as an inventory row.
pub type EndpointDetail = Endpoint;

/// Detail of the selected endpoint.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GetEndpointOutput {
    /// Endpoint details as supplied by the service.
    #[serde(rename = "Endpoint")]
    pub endpoint: EndpointDetail,
}

/// Selects one page of regional endpoints.
#[derive(Clone, Debug, Default)]
pub struct ListEndpointsInput {
    /// Optional vNetwork zone identifier.
    pub zone_id: String,
    /// Optional VPC selector.
    pub vpc_id: String,
    /// Optional endpoint selector.
    pub uuid: String,
    /// One-based page number. Nonpositive values select page one.
    pub page: i64,
    /// Page size. Nonpositive values select the provider default.
    pub size: i64,
}

/// A page of endpoint inventory.
pub type ListEndpointsOutput = PagedList<Endpoint>;

/// Selects the endpoint whose tags to list.
#[derive(Clone, Debug)]
pub struct ListEndpointTagsInput {
    /// Endpoint identifier.
    pub endpoint_id: String,
}

impl ListEndpointTagsInput {
    /// Selects an endpoint by identifier.
    pub fn new(endpoint_id: impl Into<String>) -> Self {
        Self {
            endpoint_id: endpoint_id.into(),
        }
    }
}

/// Tags attached to the selected endpoint.
pub type ListEndpointTagsOutput = List<EndpointTag>;

#[derive(Serialize)]
struct Filter<'a> {
    field: &'static str,
    value: &'a str,
}

#[derive(Serialize)]
struct Params<'a> {
    page: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    search: Vec<Filter<'a>>,
    size: i64,
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Lists the regions advertised by the configured vNetwork gateway.
    pub async fn list_v_network_regions(
        &self,
        ctx: &RequestContext,
        _input: ListVNetworkRegionsInput,
    ) -> Result<ListVNetworkRegionsOutput, Failure<ListVNetworkRegionsOutput>> {
        crate::wire::product::exit::finish!(async {
            let base = self.vnetwork_base().map_err(failure)?;
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            let items = self
                .regions(ctx, &base, &mut evidence)
                .await
                .map_err(failure)?;
            Ok(List { items })
        })
    }

    /// Reads an endpoint in the configured project and region.
    pub async fn get_endpoint(
        &self,
        ctx: &RequestContext,
        input: GetEndpointInput,
    ) -> Result<GetEndpointOutput, Failure<GetEndpointOutput>> {
        crate::wire::product::exit::finish!(async {
            DETAIL
                .check_path("EndpointID", &input.endpoint_id)
                .map_err(failure)?;
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            validate_selection(&evidence, &[&input.endpoint_id]).map_err(failure)?;
            let base = self.vnetwork_base().map_err(failure)?;
            let project = self
                .endpoint_project(ctx, &mut evidence)
                .await
                .map_err(failure)?;
            let (zone, discovered) = self
                .resolve_zone(ctx, &base, DETAIL, &mut evidence)
                .await
                .map_err(failure)?;
            let body = self
                .inventory(
                    ctx,
                    &base,
                    DETAIL,
                    Selection::path(&[&zone, &project, &input.endpoint_id]),
                    &mut evidence,
                )
                .await;
            let body = body
                .map_err(|error| failure(recheck(DETAIL, error, &zone, discovered, &evidence)))?;
            let text = go_json(&body);
            let fields = envelope(DETAIL, &text, &evidence).map_err(failure)?;
            let endpoint: Endpoint = member(&fields, "data", DETAIL, &evidence).map_err(failure)?;
            validate_endpoint_ids(&evidence, &endpoint)
                .map_err(|()| failure(DETAIL.refused(&evidence)))?;
            let endpoint = DETAIL.retain(endpoint, &evidence).map_err(failure)?;
            Ok(GetEndpointOutput { endpoint })
        })
    }

    /// Lists one page of matching endpoints.
    pub async fn list_endpoints(
        &self,
        ctx: &RequestContext,
        input: ListEndpointsInput,
    ) -> Result<ListEndpointsOutput, Failure<ListEndpointsOutput>> {
        crate::wire::product::exit::finish!(async {
            if !input.zone_id.is_empty() {
                LIST.check_path("ZoneID", &input.zone_id).map_err(failure)?;
            }
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            validate_selection(&evidence, &[&input.zone_id, &input.vpc_id, &input.uuid])
                .map_err(failure)?;
            let base = self.vnetwork_base().map_err(failure)?;
            let project = self
                .endpoint_project(ctx, &mut evidence)
                .await
                .map_err(failure)?;
            let (zone, discovered) = if input.zone_id.is_empty() {
                self.resolve_zone(ctx, &base, LIST, &mut evidence)
                    .await
                    .map_err(failure)?
            } else {
                (input.zone_id, false)
            };
            let params = Params {
                page: input.page.max(1),
                search: [
                    ("vpcId", input.vpc_id.as_str()),
                    ("uuid", input.uuid.as_str()),
                ]
                .into_iter()
                .filter(|(_, value)| !value.is_empty())
                .map(|(field, value)| Filter { field, value })
                .collect(),
                size: if input.size > 0 { input.size } else { 10_000 },
            };
            let params = serde_json::to_string(&params)
                .map_err(|_| failure(Error::new(ErrorClass::InvalidInput)))?;
            let body = self
                .inventory(
                    ctx,
                    &base,
                    LIST,
                    Selection {
                        path: &[&zone, &project],
                        query: &[("params", &params)],
                        headers: &[],
                    },
                    &mut evidence,
                )
                .await;
            let body =
                body.map_err(|error| failure(recheck(LIST, error, &zone, discovered, &evidence)))?;
            let text = go_json(&body);
            let fields = envelope(LIST, &text, &evidence).map_err(failure)?;
            let items: Vec<Endpoint> = member(&fields, "data", LIST, &evidence).map_err(failure)?;
            for item in &items {
                validate_endpoint_ids(&evidence, item)
                    .map_err(|()| failure(LIST.refused(&evidence)))?;
            }
            Ok(PagedList {
                items: LIST.retain(items, &evidence).map_err(failure)?,
                page: scalar(&fields, "page", LIST, &evidence).map_err(failure)?,
                page_size: scalar(&fields, "size", LIST, &evidence).map_err(failure)?,
                total_page: scalar(&fields, "totalPage", LIST, &evidence).map_err(failure)?,
                total_item: scalar(&fields, "total", LIST, &evidence).map_err(failure)?,
            })
        })
    }

    /// Lists tags using project metadata for the portal user header.
    pub async fn list_endpoint_tags(
        &self,
        ctx: &RequestContext,
        input: ListEndpointTagsInput,
    ) -> Result<ListEndpointTagsOutput, Failure<ListEndpointTagsOutput>> {
        crate::wire::product::exit::finish!(async {
            if input.endpoint_id.is_empty() {
                TAGS.check_path("EndpointID", "").map_err(failure)?;
            }
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            validate_selection(&evidence, &[&input.endpoint_id]).map_err(failure)?;
            let base = self.vnetwork_base().map_err(failure)?;
            let project = self
                .session
                .require_project_with_evidence(ctx, &mut evidence)
                .await
                .map_err(|error| failure(screen(*error, &evidence)))?;
            validate_selection(&evidence, &[&project.id, &input.endpoint_id]).map_err(failure)?;
            let user_id = project.user_id.to_string();
            if project.user_id > 0 && evidence.id_reflects(&user_id) {
                return Err(failure(Error::new(ErrorClass::InvalidInput)));
            }
            let portal = [("portal-user-id", user_id.as_str())];
            let headers = if project.user_id > 0 {
                portal.as_slice()
            } else {
                &[]
            };
            let body = self
                .inventory(
                    ctx,
                    &base,
                    TAGS,
                    Selection {
                        path: &[&project.id],
                        query: &[("resourceUuid", &input.endpoint_id)],
                        headers,
                    },
                    &mut evidence,
                )
                .await
                .map_err(failure)?;
            let text = go_json(&body);
            let fields = envelope(TAGS, &text, &evidence).map_err(failure)?;
            let tags: Vec<EndpointTag> =
                member(&fields, "data", TAGS, &evidence).map_err(failure)?;
            for tag in &tags {
                validate_returned_ids(&evidence, [&tag.uuid, &tag.resource_uuid])
                    .map_err(|()| failure(TAGS.refused(&evidence)))?;
            }
            let items = TAGS.retain(tags, &evidence).map_err(failure)?;
            Ok(List { items })
        })
    }

    fn vnetwork_base(&self) -> Result<Base, Error> {
        let endpoint = self
            .session
            .endpoints()
            .vnetwork
            .as_ref()
            .filter(|base| base.is_explicit_override());
        let base = match endpoint {
            Some(base) => Base::parse(base.configured_url()).ok(),
            None => vnetwork_regional_gateway(&self.region),
        };
        base.ok_or_else(|| Error::new(ErrorClass::InvalidConfig))
    }

    async fn endpoint_project(
        &self,
        ctx: &RequestContext,
        evidence: &mut Evidence,
    ) -> Result<String, Error> {
        let project = self
            .session
            .require_project_id_with_evidence(ctx, evidence)
            .await
            .map_err(|error| screen(*error, evidence))?;
        validate_selection(evidence, &[&project])?;
        Ok(project)
    }

    async fn regions(
        &self,
        ctx: &RequestContext,
        base: &Base,
        evidence: &mut Evidence,
    ) -> Result<Vec<VNetworkRegion>, Error> {
        let ctx =
            ctx.child_with_deadline(self.session.clock().monotonic_now() + Duration::from_secs(30));
        let body = self
            .inventory(&ctx, base, REGIONS, Selection::path(&[]), evidence)
            .await?;
        let text = go_json(&body);
        let fields: Fields<'_> = REGIONS.decode(&text, evidence)?;
        let regions: Vec<VNetworkRegion> = member(&fields, "data", REGIONS, evidence)?;
        for region in &regions {
            validate_returned_ids(evidence, [&region.uuid])
                .map_err(|()| REGIONS.shape(evidence))?;
        }
        Ok(regions)
    }

    async fn resolve_zone(
        &self,
        ctx: &RequestContext,
        base: &Base,
        read: InventoryRead,
        evidence: &mut Evidence,
    ) -> Result<(String, bool), Error> {
        if let Some(zone) = self.endpoint_zone.get() {
            if evidence.id_reflects(zone) {
                return Err(read.refused(evidence));
            }
            return Ok((zone.clone(), true));
        }
        let origin = format!("https://{}-vnetwork.console.greennode.ai", self.region);
        if let Ok(regions) = self.regions(ctx, base, evidence).await {
            for region in regions {
                if (region.name.eq_ignore_ascii_case(&self.region)
                    || region.code.eq_ignore_ascii_case(&self.region)
                    || region.dashboard_url.trim_end_matches('/') == origin
                    || region
                        .gateway_url
                        .trim_end_matches('/')
                        .strip_suffix("/vnetwork-gateway")
                        == Some(origin.as_str()))
                    && PathIds::Strict.allows(&region.uuid)
                    && !evidence.id_reflects(&region.uuid)
                {
                    let _ = self.endpoint_zone.set(region.uuid.clone());
                    return Ok((region.uuid, true));
                }
            }
        }
        Ok((self.region.clone(), false))
    }
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn validate_selection(evidence: &Evidence, values: &[&str]) -> Result<(), Error> {
    if values
        .iter()
        .any(|value| !value.is_empty() && evidence.id_reflects(value))
    {
        return Err(Error::new(ErrorClass::InvalidInput));
    }
    Ok(())
}

fn recheck(
    read: InventoryRead,
    error: Error,
    zone: &str,
    discovered: bool,
    evidence: &Evidence,
) -> Error {
    if discovered && evidence.id_reflects(zone) {
        read.refused(evidence)
    } else {
        error
    }
}

fn validate_endpoint_ids(evidence: &Evidence, endpoint: &Endpoint) -> Result<(), ()> {
    if evidence.identifier_output_reflects(&endpoint.package_id)
        || evidence.identifier_output_reflects(&endpoint.portal_user_id)
        || [
            &endpoint.endpoint_domains,
            &endpoint.project,
            &endpoint.region,
            &endpoint.endpoint_resource,
            &endpoint.endpoint_detail_information,
            &endpoint.category,
            &endpoint.package,
            &endpoint.packages,
            &endpoint.price,
            &endpoint.monthly_price,
            &endpoint.elastic_ips,
            &endpoint.sec_groups,
            &endpoint.subnets,
            &endpoint.service,
            &endpoint.subnet,
            &endpoint.vpc,
        ]
        .into_iter()
        .any(|value| evidence.output_fields_reflect(value))
    {
        return Err(());
    }
    validate_returned_ids(
        evidence,
        [
            &endpoint.uuid,
            &endpoint.id,
            &endpoint.endpoint_service_id,
            &endpoint.vpc_id,
            &endpoint.subnet_id,
            &endpoint.project_id,
            &endpoint.project_uuid,
            &endpoint.region_id,
            &endpoint.region_uuid,
            &endpoint.resource_service_id,
            &endpoint.backend_project_id,
            &endpoint.route_table_uuid,
            &endpoint.interface_acl_policy_uuid,
            &endpoint.zone_uuid,
        ],
    )
}

fn validate_returned_ids<'a>(
    evidence: &Evidence,
    values: impl IntoIterator<Item = &'a String>,
) -> Result<(), ()> {
    if values
        .into_iter()
        .any(|value| !value.is_empty() && evidence.id_reflects(value))
    {
        return Err(());
    }
    Ok(())
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn envelope<'a>(
    read: InventoryRead,
    text: &'a str,
    evidence: &Evidence,
) -> Result<Fields<'a>, Error> {
    read.decode(
        if text.is_empty() || text.trim() == "null" {
            "{}"
        } else {
            text
        },
        evidence,
    )
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn member<'a, T: crate::model::wire::WireField>(
    fields: &Fields<'a>,
    key: &str,
    read: InventoryRead,
    evidence: &Evidence,
) -> Result<T, Error> {
    fields
        .output_raw(key)
        .and_then(|raw| {
            raw.map(|raw| serde_json::from_str::<Field<T>>(raw.get()).map_err(|_| ()))
                .transpose()
        })
        .map_err(|()| read.shape(evidence))
        .map(|field| field.map_or_else(T::default, |field| field.0))
}

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
fn scalar(
    fields: &Fields<'_>,
    key: &str,
    read: InventoryRead,
    evidence: &Evidence,
) -> Result<i64, Error> {
    let value: i64 = member(fields, key, read, evidence)?;
    if evidence.text_reflects(&value.to_string()) {
        return Err(read.shape(evidence));
    }
    read.retain(value, evidence)
}
