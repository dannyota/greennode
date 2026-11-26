use super::*;

descriptor!(
    LIST_ALL_SNAPSHOTS_INPUT,
    volume::ListAllSnapshotsInput,
    check_input_source,
    []
);

pub(super) const LIST_ALL_SNAPSHOTS: Operation = read!(
    "list-all-snapshots",
    "volume.ListAllSnapshots",
    "List all snapshots",
    &[],
    &[],
    &[],
    "greennode volume list-all-snapshots",
    &[
        "Visits up to 50 pages per inventory. Conflicting or incomplete pages fail without an aggregate."
    ],
    LIST_ALL_SNAPSHOTS_INPUT,
    list_all_snapshots,
    DEFAULT_VIEW
);

handler!(
    list_all_snapshots,
    volume::ListAllSnapshotsInput,
    projection::all_snapshots
);

descriptor!(
    LIST_SNAPSHOT_BACKENDS_INPUT,
    volume::ListSnapshotBackendsInput,
    check_input_source,
    [field!(
        "Name",
        flag("name", "string", true),
        as_str,
        String,
        name,
        Requirement::Nonzero(|input| !input.name.is_empty())
    )]
);

pub(super) const LIST_SNAPSHOT_BACKENDS: Operation = read!(
    "list-snapshot-backends",
    "volume.ListSnapshotBackends",
    "List snapshot backends",
    &[flag("name", "string", true)],
    &[],
    &[],
    "greennode volume list-snapshot-backends --name <name>",
    &["Supported regions are `hcm-3` and `han-1`. Use `HCM-03` or `HAN-01` to find a backend ID."],
    LIST_SNAPSHOT_BACKENDS_INPUT,
    list_snapshot_backends,
    DEFAULT_VIEW
);

handler!(
    list_snapshot_backends,
    volume::ListSnapshotBackendsInput,
    std::convert::identity
);

descriptor!(
    LIST_SNAPSHOT_POLICIES_INPUT,
    volume::ListSnapshotPoliciesInput,
    check_input_source,
    [
        field!(
            "BackendID",
            flag("backend-id", "string", true),
            as_str,
            String,
            backend_id,
            Requirement::Nonzero(|input| !input.backend_id.is_empty())
        ),
        field!(
            "Page",
            flag("page", "int", false),
            as_i64,
            i64,
            page,
            Requirement::Optional
        ),
        field!(
            "Size",
            flag("size", "int", false),
            as_i64,
            i64,
            size,
            Requirement::Optional
        )
    ]
);

pub(super) const LIST_SNAPSHOT_POLICIES: Operation = read!(
    "list-snapshot-policies",
    "volume.ListSnapshotPolicies",
    "List snapshot policies",
    &[
        flag("backend-id", "string", true),
        flag("page", "int", false),
        flag("size", "int", false)
    ],
    &[],
    &[],
    "greennode volume list-snapshot-policies --backend-id <backend-id>",
    &[
        "Use a backend ID from `list-snapshot-backends`. The default page is 1 and the default size is 10."
    ],
    LIST_SNAPSHOT_POLICIES_INPUT,
    list_snapshot_policies,
    DEFAULT_VIEW
);

handler!(
    list_snapshot_policies,
    volume::ListSnapshotPoliciesInput,
    std::convert::identity
);

descriptor!(
    LIST_SNAPSHOTS_INPUT,
    volume::ListSnapshotsInput,
    check_input_source,
    [
        field!(
            "VolumeID",
            flag("volume-id", "string", true),
            as_str,
            String,
            volume_id,
            Requirement::Nonzero(|input| !input.volume_id.is_empty())
        ),
        field!(
            "Page",
            flag("page", "int", false),
            as_i64,
            i64,
            page,
            Requirement::Optional
        ),
        field!(
            "Size",
            flag("size", "int", false),
            as_i64,
            i64,
            size,
            Requirement::Optional
        )
    ]
);

pub(super) const LIST_SNAPSHOTS: Operation = read!(
    "list-snapshots",
    "volume.ListSnapshots",
    "List snapshots",
    &[
        flag("volume-id", "string", true),
        flag("page", "int", false),
        flag("size", "int", false)
    ],
    &[],
    &[],
    "greennode volume list-snapshots --volume-id <volume-id>",
    &[],
    LIST_SNAPSHOTS_INPUT,
    list_snapshots,
    DEFAULT_VIEW
);

handler!(
    list_snapshots,
    volume::ListSnapshotsInput,
    projection::snapshots
);
