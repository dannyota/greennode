use std::collections::HashSet;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

use super::wire::{Family, Read, decode, failure, input_error, path_id_failure};
use super::{Client, ResponseField};
use crate::model::wire::{Field, WireField};
use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, Failure, PagedList, RequestContext};

const LIST: Read = Read {
    id: "monitor.ListDashboards",
    route: "vmonitor-api/api/v1/dashboards",
    limit: 4 << 20,
    family: Family::Dashboard,
};
const GET: Read = Read {
    id: "monitor.GetDashboard",
    route: "vmonitor-api/api/v1/dashboards/{}",
    ..LIST
};

/// Public dashboard metadata.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Dashboard {
    /// Dashboard identifier.
    #[serde(
        rename(deserialize = "id", serialize = "ID"),
        serialize_with = "crate::wire::product::identifier::serialize"
    )]
    pub id: String,
    /// Display name.
    #[serde(
        rename(deserialize = "name", serialize = "Name"),
        default,
        deserialize_with = "nullable"
    )]
    pub name: String,
    /// Whether GreenNode generated this dashboard.
    #[serde(rename(deserialize = "system", serialize = "System"))]
    pub system: ResponseField<bool>,
    /// Whether the dashboard is marked as a favorite.
    #[serde(
        rename(deserialize = "favorite", serialize = "Favorite"),
        default,
        deserialize_with = "nullable"
    )]
    pub favorite: ResponseField<bool>,
    /// Provider creation timestamp text.
    #[serde(
        rename(deserialize = "createdAt", serialize = "CreatedAt"),
        default,
        deserialize_with = "nullable"
    )]
    pub created_at: String,
    /// Provider modification timestamp text.
    #[serde(
        rename(deserialize = "updatedAt", serialize = "UpdatedAt"),
        default,
        deserialize_with = "nullable"
    )]
    pub updated_at: String,
}

crate::wire::product::output::public_fields!(Dashboard {
    id,
    name,
    system,
    favorite,
    created_at,
    updated_at
});

/// Filter and pagination for dashboard listing.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListDashboardsInput {
    /// One-based page number. Zero selects the first page.
    pub page: i64,
    /// Page size up to 100. Zero selects 10.
    pub size: i64,
    /// Returns system dashboards when true and custom dashboards otherwise.
    pub system: bool,
}

/// A page of matching dashboards.
pub type ListDashboardsOutput = PagedList<Dashboard>;

/// Dashboard selection for a detail read.
#[derive(Clone, Debug)]
pub struct GetDashboardInput {
    /// Path-safe dashboard identifier.
    pub dashboard_id: String,
}

impl GetDashboardInput {
    /// Selects a dashboard by identifier.
    pub fn new(dashboard_id: impl Into<String>) -> Self {
        Self {
            dashboard_id: dashboard_id.into(),
        }
    }
}

/// The selected dashboard's metadata.
#[derive(Clone, Serialize)]
pub struct GetDashboardOutput {
    /// Dashboard without widgets.
    #[serde(rename = "Dashboard")]
    pub dashboard: Dashboard,
}

crate::wire::product::output::public_fields!(GetDashboardOutput { dashboard });

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page {
    #[serde(rename = "lstData")]
    items: Vec<Dashboard>,
    page: Field<Option<i64>>,
    page_size: Field<Option<i64>>,
    total_page: Field<Option<i64>>,
    total_item: Field<Option<i64>>,
}

const DASHBOARD: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "id",
        shape: Shape::Any,
    },
    ShapeField {
        name: "name",
        shape: Shape::Any,
    },
    ShapeField {
        name: "system",
        shape: Shape::Any,
    },
    ShapeField {
        name: "favorite",
        shape: Shape::Any,
    },
    ShapeField {
        name: "createdAt",
        shape: Shape::Any,
    },
    ShapeField {
        name: "updatedAt",
        shape: Shape::Any,
    },
]);
const PAGE: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "lstData",
        shape: Shape::Array(&DASHBOARD),
    },
    ShapeField {
        name: "page",
        shape: Shape::Any,
    },
    ShapeField {
        name: "pageSize",
        shape: Shape::Any,
    },
    ShapeField {
        name: "totalPage",
        shape: Shape::Any,
    },
    ShapeField {
        name: "totalItem",
        shape: Shape::Any,
    },
]);

impl Client {
    /// Reads one filtered page of account dashboards.
    pub async fn list_dashboards(
        &self,
        ctx: &RequestContext,
        input: ListDashboardsInput,
    ) -> Result<ListDashboardsOutput, Failure<ListDashboardsOutput>> {
        crate::wire::product::exit::finish!(async {
            if input.page < 0 || !(0..=100).contains(&input.size) {
                return Err(failure(input_error(format!(
                    "vngcloud: invalid input: {}: invalid page or size",
                    LIST.id
                ))));
            }
            let page = input.page.max(1);
            let size = if input.size == 0 { 10 } else { input.size };
            let filter = if input.system {
                "systemType:productVng"
            } else {
                "system:false"
            };
            let query = [
                ("filter", filter),
                ("page", &page.to_string()),
                ("searching-field", ""),
                ("searching-text", ""),
                ("size", &size.to_string()),
            ];
            let mut evidence = Evidence::new();
            let raw = self
                .read(ctx, &LIST, &[], &query, &mut evidence)
                .await
                .map_err(failure)?;
            page_output(&raw, input.system, page, size, &evidence).map_err(failure)
        })
    }

    /// Reads a dashboard by identifier.
    pub async fn get_dashboard(
        &self,
        ctx: &RequestContext,
        input: GetDashboardInput,
    ) -> Result<GetDashboardOutput, Failure<GetDashboardOutput>> {
        crate::wire::product::exit::finish!(async {
            if !PathIds::Strict.allows(&input.dashboard_id) {
                return Err(path_id_failure(GET.id, "DashboardID"));
            }
            let mut evidence = Evidence::new();
            let raw = self
                .read(ctx, &GET, &[&input.dashboard_id], &[], &mut evidence)
                .await
                .map_err(failure)?;
            let mut dashboard: Dashboard =
                decode(&raw, &DASHBOARD, &GET, &evidence).map_err(failure)?;
            if dashboard.id != input.dashboard_id || !dashboard.redact(&evidence) {
                return Err(failure(GET.invalid(&evidence)));
            }
            Ok(GetDashboardOutput { dashboard })
        })
    }
}

fn page_output(
    raw: &RawValue,
    system: bool,
    page: i64,
    size: i64,
    evidence: &Evidence,
) -> Result<ListDashboardsOutput, Error> {
    let invalid = || LIST.invalid(evidence);
    let mut wire: Page = decode(raw, &PAGE, &LIST, evidence)?;
    let (Some(total_item), Some(total_page)) = (wire.total_item.0, wire.total_page.0) else {
        return Err(invalid());
    };
    let remainder = total_item % size;
    let pages = total_item / size + i64::from(remainder > 0);
    let expected = match page.cmp(&pages) {
        std::cmp::Ordering::Greater => 0,
        std::cmp::Ordering::Equal if remainder > 0 => remainder,
        _ => size,
    };
    if wire.page.0 != Some(page)
        || wire.page_size.0 != Some(size)
        || total_item < 0
        || total_page != pages
        || usize::try_from(expected).ok() != Some(wire.items.len())
    {
        return Err(invalid());
    }
    let mut ids = HashSet::new();
    if !wire.items.iter().all(|dashboard| {
        dashboard.system == ResponseField::Value(system)
            && PathIds::Strict.allows(&dashboard.id)
            && ids.insert(&dashboard.id)
    }) {
        return Err(invalid());
    }
    if !wire
        .items
        .iter_mut()
        .all(|dashboard| dashboard.redact(evidence))
        || [page, size, total_page, total_item]
            .iter_mut()
            .any(|value| !value.redact(evidence))
    {
        return Err(invalid());
    }
    Ok(PagedList {
        items: wire.items,
        page,
        page_size: size,
        total_page,
        total_item,
    })
}

impl Dashboard {
    fn redact(&mut self, evidence: &Evidence) -> bool {
        !evidence.id_reflects(&self.id)
            && !evidence.text_reflects(&self.name)
            && self.system.redact(evidence)
            && self.favorite.redact(evidence)
            && self.created_at.redact(evidence)
            && self.updated_at.redact(evidence)
    }
}

fn nullable<'de, D: Deserializer<'de>, T: WireField>(input: D) -> Result<T, D::Error> {
    Field::<T>::deserialize(input).map(|field| field.0)
}
