use std::collections::HashSet;

use crate::wire::product::{evidence::Evidence, options::PathIds};
use crate::{Error, ErrorClass, Failure, List, PagedList, RequestContext};

use super::inventory::{paged, response_id};
use super::wire::{Object, PAGE_SHAPE, invalid, screen, shape};
use super::{
    Client, ListVolumesInput, Read, Snapshot, SnapshotBackend, SnapshotPolicy, output, page_query,
    path_id,
};

const SNAPSHOTS: Read = Read::vserver(
    "volume.ListSnapshots",
    "v2/{}/volumes/{}/snapshots",
    PAGE_SHAPE,
);
const BACKENDS: Read = Read::backup("volume.ListSnapshotBackends", "v1/backends");
const POLICIES: Read = Read::backup("volume.ListSnapshotPolicies", "v1/snapshot-policies");
const ALL: &str = "volume.ListAllSnapshots";
const MAX_PAGES: i64 = 50;

/// Volume and page selection for its snapshots.
#[derive(Clone, Default)]
pub struct ListSnapshotsInput {
    /// Volume UUID.
    pub volume_id: String,
    /// One-based page number. A nonpositive value selects the default.
    pub page: i64,
    /// Requested page size. A nonpositive value selects the default.
    pub size: i64,
}

crate::error::debug_record!(ListSnapshotsInput {
    volume_id,
    page,
    size
});

/// One page of snapshots.
pub type ListSnapshotsOutput = PagedList<Snapshot>;

/// Input for collecting all snapshots in the project.
#[derive(Clone, Copy, Default)]
pub struct ListAllSnapshotsInput;

crate::error::debug_record!(ListAllSnapshotsInput unit);

/// Complete set of snapshots reached through the project's volumes.
pub type ListAllSnapshotsOutput = List<Snapshot>;

/// Selects Backup Center snapshot backends by name.
#[derive(Clone, Default)]
pub struct ListSnapshotBackendsInput {
    /// Backend name, such as `HAN-01`.
    pub name: String,
}

crate::error::debug_record!(ListSnapshotBackendsInput { name });

/// Backends matching the requested name.
pub type ListSnapshotBackendsOutput = List<SnapshotBackend>;

/// Selects a snapshot backend and a page of its policies.
#[derive(Clone, Default)]
pub struct ListSnapshotPoliciesInput {
    /// Snapshot backend identifier.
    pub backend_id: String,
    /// One-based page number. Zero selects page 1.
    pub page: i64,
    /// Requested page size. Zero selects 10 items.
    pub size: i64,
}

crate::error::debug_record!(ListSnapshotPoliciesInput {
    backend_id,
    page,
    size
});

/// One page of policies at the selected snapshot backend.
pub type ListSnapshotPoliciesOutput = PagedList<SnapshotPolicy>;

#[expect(
    clippy::result_large_err,
    reason = "The public API returns Failure by value"
)]
impl Client {
    /// Reads a page of snapshots for the selected volume.
    pub async fn list_snapshots(
        &self,
        ctx: &RequestContext,
        input: ListSnapshotsInput,
    ) -> Result<ListSnapshotsOutput, Failure<ListSnapshotsOutput>> {
        crate::wire::product::exit::finish!(async {
            output(
                self.snapshots(
                    ctx,
                    &input.volume_id,
                    input.page,
                    input.size,
                    &mut Evidence::new(),
                )
                .await,
            )
        })
    }

    async fn snapshots(
        &self,
        ctx: &RequestContext,
        id: &str,
        page: i64,
        size: i64,
        evidence: &mut Evidence,
    ) -> Result<ListSnapshotsOutput, Box<Error>> {
        path_id(SNAPSHOTS.id, "VolumeID", id)?;
        let project = self.project(ctx, evidence).await?;
        let (page, size) = page_query(page, size);
        self.read(
            ctx,
            SNAPSHOTS,
            &[&project, id],
            &[("page", &page), ("size", &size)],
            evidence,
            |raw, names| paged(raw, names, "items", "totalPages", "totalItems"),
        )
        .await
    }

    /// Collects all snapshot pages and returns them only after checking completeness.
    pub async fn list_all_snapshots(
        &self,
        ctx: &RequestContext,
        _input: ListAllSnapshotsInput,
    ) -> Result<ListAllSnapshotsOutput, Failure<ListAllSnapshotsOutput>> {
        crate::wire::product::exit::finish!(async { output(self.all_snapshots(ctx).await) })
    }

    async fn all_snapshots(
        &self,
        ctx: &RequestContext,
    ) -> Result<ListAllSnapshotsOutput, Box<Error>> {
        let mut evidence = Evidence::new();
        let volumes = pages(
            ctx,
            self,
            |volume: &super::Volume| Some(volume.uuid.as_str()),
            PathIds::Strict,
            async |page| {
                self.volumes(
                    ctx,
                    &ListVolumesInput {
                        page,
                        ..ListVolumesInput::default()
                    },
                    &mut evidence,
                )
                .await
            },
        )
        .await?;
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        for volume in volumes {
            let id = volume.uuid.as_str();
            let snapshots = pages(
                ctx,
                self,
                |snapshot: &Snapshot| (snapshot.volume_id == id).then(|| snapshot_id(snapshot)),
                PathIds::WithUnderscore,
                async |page| self.snapshots(ctx, id, page, 0, &mut evidence).await,
            )
            .await?;
            for snapshot in snapshots {
                if !seen.insert(snapshot_id(&snapshot).to_owned()) {
                    return Err(incomplete());
                }
                items.push(snapshot);
            }
        }
        screen(&evidence, List { items }, ALL)
    }

    /// Reads snapshot backends in `hcm-3` or `han-1`.
    pub async fn list_snapshot_backends(
        &self,
        ctx: &RequestContext,
        input: ListSnapshotBackendsInput,
    ) -> Result<ListSnapshotBackendsOutput, Failure<ListSnapshotBackendsOutput>> {
        crate::wire::product::exit::finish!(async {
            output(
                async {
                    if input.name.trim().is_empty() {
                        return Err(invalid(
                            "vngcloud: invalid input: volume.ListSnapshotBackends requires Name",
                        ));
                    }
                    self.snapshot_region(BACKENDS.id)?;
                    self.read(
                        ctx,
                        BACKENDS,
                        &[],
                        &[("backend", &input.name)],
                        &mut Evidence::new(),
                        |raw, names| {
                            Ok(List {
                                items: Object::parse(raw, names)?.require("items")?,
                            })
                        },
                    )
                    .await
                }
                .await,
            )
        })
    }

    /// Reads a page of project-scoped snapshot policies at one backend.
    pub async fn list_snapshot_policies(
        &self,
        ctx: &RequestContext,
        input: ListSnapshotPoliciesInput,
    ) -> Result<ListSnapshotPoliciesOutput, Failure<ListSnapshotPoliciesOutput>> {
        crate::wire::product::exit::finish!(async {
            output(async {
                if !response_id(&input.backend_id) {
                    return Err(invalid("vngcloud: invalid input: volume.ListSnapshotPolicies requires BackendID to match ^[A-Za-z0-9_-]+$"));
                }
                if input.page < 0 || input.size < 0 {
                    return Err(invalid("vngcloud: invalid input: volume.ListSnapshotPolicies requires non-negative Page and Size"));
                }
                self.snapshot_region(POLICIES.id)?;
                let mut evidence = Evidence::new();
                let project = self.project(ctx, &mut evidence).await?;
                let page = input.page.max(1).to_string();
                let size = if input.size == 0 { 10 } else { input.size }.to_string();
                self.read(ctx, POLICIES, &[], &[("backendId", &input.backend_id), ("projectId", &project), ("page", &page), ("size", &size)], &mut evidence,
                    |raw, names| paged(raw, names, "items", "totalPages", "totalItems"),
                ).await
            }.await)
        })
    }

    fn snapshot_region(&self, op: &str) -> Result<(), Box<Error>> {
        if self.session.endpoints().vserver_backup.is_none() {
            return Err(Box::new(Error::new(ErrorClass::InvalidConfig).with_detail(
                format!("vngcloud: invalid config: {op} requires a supported snapshot region"),
            )));
        }
        Ok(())
    }
}

fn snapshot_id(snapshot: &Snapshot) -> &str {
    if snapshot.id.is_empty() {
        &snapshot.uuid
    } else {
        &snapshot.id
    }
}

fn incomplete() -> Box<Error> {
    shape(
        ALL,
        "snapshot inventory is incomplete or invalid; body withheld",
    )
}

async fn pages<T>(
    ctx: &RequestContext,
    client: &Client,
    identity: impl Fn(&T) -> Option<&str>,
    ids: PathIds,
    mut fetch: impl AsyncFnMut(i64) -> Result<PagedList<T>, Box<Error>>,
) -> Result<Vec<T>, Box<Error>> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut totals = None;
    for page in 1..=MAX_PAGES {
        ctx.check(client.session.clock())
            .map_or(Ok(()), |end| Err(Box::new(end.into())))?;
        let out = fetch(page).await?;
        ctx.check(client.session.clock())
            .map_or(Ok(()), |end| Err(Box::new(end.into())))?;
        if !(0..=MAX_PAGES).contains(&out.total_page)
            || out.total_item < 0
            || out.page_size <= 0
            || usize::try_from(out.page_size).is_ok_and(|size| out.items.len() > size)
        {
            return Err(incomplete());
        }
        if page == 1
            && out.total_item == 0
            && out.items.is_empty()
            && out.total_page <= 1
            && (0..=1).contains(&out.page)
        {
            return Ok(items);
        }
        let observed = (out.total_page, out.total_item, out.page_size);
        if out.page != page
            || out.total_page < page
            || out.items.is_empty()
            || *totals.get_or_insert(observed) != observed
        {
            return Err(incomplete());
        }
        for item in out.items {
            let id = identity(&item)
                .filter(|id| ids.allows(id))
                .ok_or_else(incomplete)?;
            if !seen.insert(id.to_owned()) {
                return Err(incomplete());
            }
            items.push(item);
        }
        let count = i64::try_from(items.len()).map_err(|_| incomplete())?;
        if count > out.total_item {
            return Err(incomplete());
        }
        if page == out.total_page {
            return if count == out.total_item {
                Ok(items)
            } else {
                Err(incomplete())
            };
        }
    }
    Err(incomplete())
}
