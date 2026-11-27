use std::collections::HashSet;

use serde::Serialize;

use crate::endpoint::Base;
use crate::model::wire::{Field, WireField, go_json};
use crate::wire::product::evidence::Evidence;
use crate::{Error, ErrorClass, Failure, List, RequestContext};

use super::wire::{Fields, Read, Style};
use super::{Client, failure};

const PACKAGES: Read = Read::new(
    "network.ListBandwidthPackages",
    "vserver/bandwidth-api/v1/projects/{}/bandwidth/package-config",
    Style::Bandwidth,
);
const PROJECTS: Read = PACKAGES.at(
    "vserver/iam-vserver-gateway/v1/projects",
    Style::BandwidthProject,
);

/// Selects the configured region and project for the bandwidth catalog.
#[derive(Clone, Debug, Default)]
pub struct ListBandwidthPackagesInput {}

/// Bandwidth packages in provider order.
pub type ListBandwidthPackagesOutput = List<BandwidthPackage>;

/// An offer from the regional bandwidth catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BandwidthPackage {
    /// Package identifier.
    #[serde(rename = "ID")]
    pub id: String,
    /// Display name.
    #[serde(rename = "Name")]
    pub name: String,
    /// Provider package type, including unfamiliar values.
    #[serde(rename = "Type")]
    pub kind: String,
    /// Package description.
    #[serde(rename = "Description")]
    pub description: String,
    /// Bandwidth as supplied by the catalog.
    #[serde(rename = "Bandwidth")]
    pub bandwidth: i64,
}

crate::wire::product::output::public_fields!(BandwidthPackage {
    id,
    name,
    kind,
    description,
    bandwidth
});

#[expect(clippy::result_large_err, reason = "SDK errors are returned by value")]
impl Client {
    /// Lists the selected project's bandwidth offers in Hanoi or HCM.
    pub async fn list_bandwidth_packages(
        &self,
        ctx: &RequestContext,
        _input: ListBandwidthPackagesInput,
    ) -> Result<ListBandwidthPackagesOutput, Failure<ListBandwidthPackagesOutput>> {
        crate::wire::product::exit::finish!(async {
            let base = self.bandwidth_base().map_err(failure)?;
            let mut evidence = self.request_evidence(ctx).map_err(failure)?;
            let project = if let Some(project) = self.session.configured_project_id() {
                PACKAGES.path("ProjectID", project).map_err(failure)?;
                if evidence.text_reflects(project) {
                    return Err(failure(Error::new(ErrorClass::InvalidInput)));
                }
                project.to_owned()
            } else {
                self.discover_project(ctx, &base, PROJECTS, &mut evidence)
                    .await
                    .map_err(failure)?
            };
            let raw = self
                .read(ctx, &base, PACKAGES, &[&project], &mut evidence)
                .await
                .map_err(failure)?;
            let text = go_json(&raw);
            let items = decode_packages(&text, &evidence)
                .map_err(|()| failure(PACKAGES.invalid(&evidence)))?;
            Ok(List { items })
        })
    }

    pub(super) fn bandwidth_base(&self) -> Result<Base, Error> {
        let invalid = || Error::new(ErrorClass::InvalidConfig);
        let origin = match self.region.as_str() {
            "hcm-3" => "https://hcm-3.console.greennode.ai/",
            "han-1" => "https://han-1.console.greennode.ai/",
            _ => return Err(invalid()),
        };
        #[cfg(any(test, feature = "test-support"))]
        if self
            .session
            .has_product_fixture(crate::testing::Product::Vserver)
            && let Some(base) = self.session.endpoints().vserver.as_ref()
        {
            return Base::parse(base.configured_url()).map_err(|_| invalid());
        }
        Base::parse(origin).map_err(|_| invalid())
    }
}

fn decode_packages(text: &str, evidence: &Evidence) -> Result<Vec<BandwidthPackage>, ()> {
    let fields = Fields::decode(text)?;
    for (name, expected) in [("success", "true"), ("code", "200")] {
        if fields.raw(name)?.is_some_and(|raw| raw.get() != expected) {
            return Err(());
        }
    }
    for name in ["error", "errors", "errorCode", "errorMsg", "errorMessage"] {
        if fields.raw(name)?.is_some_and(|raw| raw.get() != "null") {
            return Err(());
        }
    }
    let rows: Vec<Fields<'_>> = fields.required("data")?;
    if fields.bandwidth_incomplete(rows.len()) {
        return Err(());
    }
    let mut seen = HashSet::new();
    rows.into_iter()
        .map(|row| {
            let mut item = BandwidthPackage {
                id: row.required("uuid")?,
                name: row.required("name")?,
                kind: row.required("type")?,
                description: row.required("description")?,
                bandwidth: row
                    .required::<Field<Option<i64>>>("bandwidth")?
                    .0
                    .ok_or(())?,
            };
            if item.id.is_empty()
                || evidence.id_reflects(&item.id)
                || !seen.insert(item.id.clone())
                || [&item.id, &item.name, &item.kind, &item.description]
                    .into_iter()
                    .any(|value| evidence.text_reflects(value))
                || evidence.text_reflects(&item.bandwidth.to_string())
                || !item.bandwidth.redact(evidence)
            {
                return Err(());
            }
            Ok(item)
        })
        .collect()
}
