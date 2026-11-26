use std::{collections::HashMap, net::Ipv4Addr};

use super::alias::{fold, same_alias};
use super::model::{Package, Region, Server, Subnet, Vpc};
use super::{Child, DecodeError, Error, PACKAGES, RequestContext, State, ids, refuse, state};
use crate::wire::product::options::PathIds;

impl Child<'_> {
    pub(super) async fn project(
        &mut self,
        ctx: &RequestContext,
        requested: &str,
    ) -> Result<String, Box<Error>> {
        let session = &self.0.client.session;
        let project = session
            .require_project_id_with_evidence(ctx, &mut self.0.evidence)
            .await?;
        if project != requested {
            return Err(refuse("BackendProjectID must equal the client's project"));
        }
        if self.0.evidence.text_reflects(&project) {
            return Err(refuse("a request value contains a credential"));
        }
        Ok(project)
    }

    pub(super) async fn region(
        &mut self,
        ctx: &RequestContext,
        package: &str,
    ) -> Result<String, Box<Error>> {
        let regions: Vec<Region> = self.get(ctx, "v1/regions", &[]).await?;
        let packages: Vec<Package> = self.get(ctx, PACKAGES, &[]).await?;
        let configured = &self.0.client.region;
        if regions.iter().any(|row| row.status.is_none()) {
            return Err(self.invalid(DecodeError::InvalidShape));
        }
        let mut matches = regions
            .iter()
            .filter(|row| same_alias(configured, &row.name));
        let Some(region) = matches.next() else {
            return Err(ineligible_region());
        };
        if matches.next().is_some() || !PathIds::WithUnderscore.allows(&region.id) {
            return Err(ineligible_region());
        }
        let endpoint = format!("https://{configured}.api.vngcloud.vn/vserver/vserver-gateway");
        if region
            .status
            .as_ref()
            .is_none_or(|status| !status.is_empty() && state(status) != State::Active)
            || region.endpoint != endpoint
        {
            return Err(refuse("the configured region is not eligible"));
        }
        if regions
            .iter()
            .filter(|row| same_alias(&row.id, &region.id))
            .count()
            != 1
        {
            return Err(refuse("the GLB region ID is ambiguous"));
        }
        let mut packages = packages.iter().filter(|row| row.id == package);
        if packages
            .next()
            .is_none_or(|package| !supports(package, configured, &region.id, &regions))
            || packages.next().is_some()
        {
            return Err(refuse(
                "the load balancer package is not enabled for the configured region",
            ));
        }
        Ok(region.id.clone())
    }

    pub(super) async fn placement(
        &mut self,
        ctx: &RequestContext,
        project: &str,
        server: &str,
        vpc: &str,
        subnet: &str,
    ) -> Result<String, Box<Error>> {
        ids(
            self.0.id,
            &[
                ("BackendServerID", server),
                ("BackendVPCID", vpc),
                ("BackendSubnetID", subnet),
            ],
        )?;
        let server_row: Server = self
            .get_from(ctx, "v2/{}/servers/{}", &[project, server], true)
            .await?;
        let vpc_row: Vpc = self
            .get_from(ctx, "v2/{}/networks/{}", &[project, vpc], true)
            .await?;
        let subnet_row: Subnet = self
            .get_from(
                ctx,
                "v2/{}/networks/{}/subnets/{}",
                &[project, vpc, subnet],
                true,
            )
            .await?;
        if server_row.data.id != server
            || state(&server_row.data.status) != State::Active
            || vpc_row.id != vpc
            || state(&vpc_row.status) != State::Active
            || subnet_row.id != subnet
            || subnet_row.vpc != vpc
            || state(&subnet_row.status) != State::Active
        {
            return Err(refuse(
                "the backend server, VPC and subnet are not an exact eligible match",
            ));
        }
        let mut nics = server_row
            .data
            .interfaces
            .iter()
            .filter(|row| row.server == server && row.vpc == vpc && row.subnet == subnet);
        let invalid_nic = || {
            refuse(
                "the backend server needs exactly one internal interface in the requested VPC and subnet",
            )
        };
        let nic = nics.next().ok_or_else(invalid_nic)?;
        if nics.next().is_some() {
            return Err(invalid_nic());
        }
        let address = nic.address.parse::<Ipv4Addr>().ok();
        let prefix = super::cidr::prefix(&subnet_row.cidr);
        if !address.zip(prefix).is_some_and(|(ip, (network, len))| {
            ip.is_private()
                && ip.to_string() == nic.address
                && super::cidr::masked(u32::from(ip), len) == super::cidr::masked(network, len)
        }) {
            return Err(refuse(
                "the backend address is not a private IPv4 address inside the subnet",
            ));
        }
        Ok(nic.address.clone())
    }
}

fn ineligible_region() -> Box<Error> {
    refuse("the configured region does not match exactly one enabled GLB region")
}

fn supports(package: &Package, configured: &str, region: &str, regions: &[Region]) -> bool {
    if package.enabled != Some(true) {
        return false;
    }
    let mut aliases = HashMap::new();
    for (index, row) in regions.iter().enumerate() {
        for alias in [&row.id, &row.name]
            .into_iter()
            .filter(|alias| !alias.is_empty())
        {
            let slot = aliases
                .entry(fold(alias).collect::<String>())
                .or_insert(Some(index));
            if *slot != Some(index) {
                *slot = None;
            }
        }
    }
    let mut found = false;
    let mut packages = HashMap::new();
    for link in &package.links {
        if !link.glb_package.matches(&package.id) {
            return false;
        }
        let index = aliases
            .get(&fold(&link.region).collect::<String>())
            .and_then(|index| *index);
        if !link.regional_package.is_empty() {
            let Some(index) = index else {
                return false;
            };
            let prior = packages.entry(index).or_insert(&link.regional_package);
            if *prior != &link.regional_package {
                return false;
            }
        }
        if same_alias(&link.region, configured) || same_alias(&link.region, region) {
            if index.is_none() {
                return false;
            }
            found = true;
        }
    }
    found
}
