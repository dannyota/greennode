use std::collections::HashSet;

use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Failure, List, PagedList, RequestContext};

use super::inventory::LIST_GROUPS;
use super::wire::{Read, ReadStyle};
use super::*;

const SECURITY: Read = Read::new("compute.ListServerSecurityGroups", "", ReadStyle::Server);
const MEMBERS: Read = Read::new("compute.ListServerGroupMembers", "", ReadStyle::Server);

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Lists the security groups attached to servers throughout the project.
    pub async fn list_server_security_groups(
        &self,
        ctx: &RequestContext,
        _: ListServerSecurityGroupsInput,
    ) -> Result<ListServerSecurityGroupsOutput, Failure<ListServerSecurityGroupsOutput>> {
        let mut evidence = Evidence::new();
        let mut walk = Inventory::default();
        let mut items = Vec::new();
        for page in 1..=50 {
            let rows = self.server_page(ctx, page, 0, &mut evidence).await?;
            let complete = walk
                .account(page, &rows, |row| &row.uuid)
                .ok_or_else(|| failure(SECURITY.invalid(&evidence)))?;
            for row in rows.items {
                let groups = row
                    .security_groups
                    .filter(|groups| distinct(groups, |group| &group.uuid))
                    .ok_or_else(|| failure(SECURITY.invalid(&evidence)))?;
                items.extend(groups.into_iter().map(|group| ServerSecurityGroup {
                    server_id: row.uuid.clone(),
                    name: group.name,
                    uuid: group.uuid,
                }));
            }
            if complete {
                return Ok(List {
                    items: SECURITY.retain(items, &evidence)?,
                });
            }
        }
        Err(failure(SECURITY.invalid(&evidence)))
    }

    /// Lists group memberships throughout the selected project.
    pub async fn list_server_group_members(
        &self,
        ctx: &RequestContext,
        _: ListServerGroupMembersInput,
    ) -> Result<ListServerGroupMembersOutput, Failure<ListServerGroupMembersOutput>> {
        let mut evidence = Evidence::new();
        let mut walk = Inventory::default();
        let mut items = Vec::new();
        for page in 1..=50 {
            let rows = self
                .group_page(
                    ctx,
                    &ListServerGroupsInput {
                        offset: walk.count,
                        ..ListServerGroupsInput::default()
                    },
                    &mut evidence,
                )
                .await?;
            let complete = walk
                .account(page, &rows, |row| &row.uuid)
                .ok_or_else(|| failure(LIST_GROUPS.invalid(&evidence)))?;
            for row in rows.items {
                let servers = row
                    .servers
                    .filter(|servers| distinct(servers, |server| &server.uuid))
                    .ok_or_else(|| failure(MEMBERS.invalid(&evidence)))?;
                items.extend(servers.into_iter().map(|server| ServerGroupMembership {
                    server_group_id: row.uuid.clone(),
                    name: server.name,
                    uuid: server.uuid,
                }));
            }
            if complete {
                return Ok(List {
                    items: MEMBERS.retain(items, &evidence)?,
                });
            }
        }
        Err(failure(LIST_GROUPS.invalid(&evidence)))
    }
}

#[derive(Default)]
struct Inventory {
    meta: Option<(i64, i64, i64)>,
    count: i64,
    ids: HashSet<String>,
}

impl Inventory {
    fn account<T>(
        &mut self,
        page: i64,
        result: &PagedList<T>,
        id: impl Fn(&T) -> &str,
    ) -> Option<bool> {
        let PagedList {
            page: actual,
            total_page,
            total_item,
            page_size,
            items,
        } = result;
        let meta = (*total_page, *total_item, *page_size);
        let len = i64::try_from(items.len()).ok()?;
        if *actual != page
            || *page_size <= 0
            || !(0..=50).contains(total_page)
            || !(0..=500_000).contains(total_item)
            || len > *page_size
            || (*total_item > 0 && (*total_page == 0 || len == 0))
            || *self.meta.get_or_insert(meta) != meta
        {
            return None;
        }
        if !items.iter().all(|row| {
            let id = id(row);
            PathIds::WithUnderscore.allows(id) && self.ids.insert(id.to_owned())
        }) {
            return None;
        }
        self.count += len;
        let complete = page >= *total_page;
        (self.count <= *total_item && (!complete || self.count == *total_item)).then_some(complete)
    }
}

fn distinct<T>(items: &[T], id: impl Fn(&T) -> &str) -> bool {
    let mut seen = HashSet::new();
    items.iter().all(|item| {
        let id = id(item);
        PathIds::WithUnderscore.allows(id) && seen.insert(id)
    })
}
