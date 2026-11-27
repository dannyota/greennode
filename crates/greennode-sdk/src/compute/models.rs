use std::collections::BTreeMap;

use serde::Serialize;

use crate::model::wire::model;
use crate::{List, PagedList, Secret};

use super::{Flavor, PackageLimit, Server};

model! {
    /// A public SSH key entry.
    pub struct SshKey {
        /// Resource identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Created at.
        created_at: String => "createdAt", "CreatedAt";
        /// Public SSH key material.
        public_key: String => "pubKey", "PublicKey";
        /// Status.
        status: String => "status", "Status";
    }
}

model! {
    /// A server group and its membership, when returned.
    pub struct ServerGroup {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Server group id.
        #[serde(serialize_with = "crate::model::wire::serialize_dynamic")]
        server_group_id: serde_json::Value => "serverGroupId", "ServerGroupID";
        /// Display name.
        name: String => "name", "Name";
        /// Description.
        description: String => "description", "Description";
        /// Policy id.
        policy_id: String => "policyId", "PolicyID";
        /// Policy name.
        policy_name: String => "policyName", "PolicyName";
        /// Created at.
        created_at: String => "createdAt", "CreatedAt";
        /// Servers.
        #[serde(serialize_with = "crate::model::wire::serialize_slice")]
        servers: Option<Vec<ServerGroupMember>> => "servers", "Servers";
    }
}

model! {
    /// A server in a server group.
    pub struct ServerGroupMember {
        /// Display name.
        name: String => "name", "Name";
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
    }
}

model! {
    /// A security group and its associated server.
    pub struct ServerSecurityGroup {
        /// Server id.
        server_id: String => "serverId", "ServerID";
        /// Display name.
        name: String => "name", "Name";
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
    }
}

model! {
    /// A server and its associated server group.
    pub struct ServerGroupMembership {
        /// Server group id.
        server_group_id: String => "serverGroupId", "ServerGroupID";
        /// Display name.
        name: String => "name", "Name";
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
    }
}

model! {
    /// An operating system image from the regional catalog.
    pub struct OsImage {
        /// Resource identifier.
        id: String => "id", "ID";
        /// Image type.
        image_type: String => "imageType", "ImageType";
        /// Image version.
        image_version: String => "imageVersion", "ImageVersion";
        /// Whether the image requires a license.
        licence: Option<bool> => "licence", "Licence";
        /// Flavor zone i ds.
        #[serde(serialize_with = "crate::model::wire::serialize_slice")]
        flavor_zone_ids: Option<Vec<String>> => "flavorZoneIds", "FlavorZoneIDs";
        /// Package limit.
        package_limit: PackageLimit => "packageLimit", "PackageLimit";
        /// License key.
        license_key: Option<String> => "licenseKey", "LicenseKey";
        /// Default tag i ds.
        #[serde(serialize_with = "crate::model::wire::serialize_slice")]
        default_tag_ids: Option<Vec<String>> => "defaultTagIds", "DefaultTagIDs";
        /// Zone id.
        zone_id: String => "zoneId", "ZoneID";
        /// Description.
        description: String => "description", "Description";
    }
}

model! {
    /// A custom image in the selected project.
    pub struct UserImage {
        /// Resource identifier.
        uuid: String => "uuid", "UUID";
        /// Project id.
        project_id: String => "projectId", "ProjectID";
        /// Display name.
        name: String => "name", "Name";
        /// Minimum disk size required by the image.
        min_disk: i64 => "minDisk", "MinDisk";
        /// Size reported by the image service.
        image_size: f64 => "imageSize", "ImageSize";
        /// Status.
        status: String => "status", "Status";
        /// Created at.
        created_at: String => "createdAt", "CreatedAt";
        /// Metadata supplied by the service.
        metadata: String => "metaData", "Metadata";
    }
}

model! {
    /// A flavor category in a network zone.
    pub struct FlavorZone {
        /// Resource identifier.
        id: String => "id", "ID";
        /// Display name.
        name: String => "name", "Name";
        /// Description.
        description: String => "description", "Description";
        /// Zone id.
        zone_id: String => "zoneId", "ZoneID";
    }
}

model! {
    pub(super) struct Policy {
        name: String => "name", "Name";
        uuid: String => "uuid", "UUID";
        status: String => "status", "Status";
        description: String => "description", "Description";
        description_vi: String => "descriptionVi", "DescriptionVI";
    }
}

/// A server-group placement policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ServerGroupPolicy {
    /// Display name.
    #[serde(rename = "Name")]
    pub name: String,
    /// Policy identifier.
    #[serde(rename = "UUID")]
    pub uuid: String,
    /// Service-reported policy status.
    #[serde(rename = "Status")]
    pub status: String,
    /// Original English description field.
    #[serde(rename = "Description")]
    pub description: String,
    /// Original Vietnamese description field.
    #[serde(rename = "DescriptionVI")]
    pub description_vi: String,
    /// Descriptions by language code.
    #[serde(rename = "Descriptions")]
    pub descriptions: BTreeMap<String, String>,
}

crate::wire::product::output::public_fields!(ServerGroupPolicy {
    name,
    uuid,
    status,
    description,
    description_vi,
    descriptions
});

/// Server inventory page selection.
#[derive(Clone, Debug, Default)]
pub struct ListServersInput {
    /// One-based page number. Nonpositive values use the default.
    pub page: i64,
    /// Items per page. Nonpositive values use the default.
    pub size: i64,
}

/// Selects the server to read.
#[derive(Clone, Debug)]
pub struct GetServerInput {
    /// Resource identifier.
    pub server_id: String,
}

impl GetServerInput {
    /// Creates an input from the required identifier.
    pub fn new(server_id: impl Into<String>) -> Self {
        Self {
            server_id: server_id.into(),
        }
    }
}

/// Selects the server whose console log is requested.
#[derive(Clone, Debug)]
pub struct GetServerConsoleLogInput {
    /// Resource identifier.
    pub server_id: String,
}

impl GetServerConsoleLogInput {
    /// Creates an input from the required identifier.
    pub fn new(server_id: impl Into<String>) -> Self {
        Self {
            server_id: server_id.into(),
        }
    }
}

/// SSH key inventory filters and page selection.
#[derive(Clone, Debug, Default)]
pub struct ListSshKeysInput {
    /// Optional exact name filter.
    pub name: String,
    /// One-based page number. Nonpositive values use the default.
    pub page: i64,
    /// Items per page. Nonpositive values use the default.
    pub size: i64,
}

/// Selects the SSH key to read.
#[derive(Clone, Debug)]
pub struct GetSshKeyInput {
    /// Resource identifier.
    pub ssh_key_id: String,
}

impl GetSshKeyInput {
    /// Creates an input from the required identifier.
    pub fn new(ssh_key_id: impl Into<String>) -> Self {
        Self {
            ssh_key_id: ssh_key_id.into(),
        }
    }
}

/// Server group inventory filters and page selection.
#[derive(Clone, Debug, Default)]
pub struct ListServerGroupsInput {
    /// Optional exact name filter.
    pub name: String,
    /// Zero-based index of the first group.
    pub offset: i64,
    /// Maximum number of groups. Nonpositive values use the default.
    pub limit: i64,
}

/// Selects the server group to read.
#[derive(Clone, Debug)]
pub struct GetServerGroupInput {
    /// Resource identifier.
    pub server_group_id: String,
}

impl GetServerGroupInput {
    /// Creates an input from the required identifier.
    pub fn new(server_group_id: impl Into<String>) -> Self {
        Self {
            server_group_id: server_group_id.into(),
        }
    }
}

/// Lists attached security groups across the project.
#[derive(Clone, Debug, Default)]
pub struct ListServerSecurityGroupsInput {}

/// Lists server group membership across the project.
#[derive(Clone, Debug, Default)]
pub struct ListServerGroupMembersInput {}

/// Lists the placement policies in the selected project.
#[derive(Clone, Debug, Default)]
pub struct ListServerGroupPoliciesInput {}

/// Filters the regional operating system image catalog.
#[derive(Clone, Debug, Default)]
pub struct ListOsImagesInput {
    /// Optional network zone identifier.
    pub zone_id: String,
}

/// Lists the GPU images in the selected project.
#[derive(Clone, Debug, Default)]
pub struct ListGpuImagesInput {}

/// User image inventory page selection.
#[derive(Clone, Debug, Default)]
pub struct ListUserImagesInput {
    /// One-based page number. Nonpositive values use the default.
    pub page: i64,
    /// Items per page. Nonpositive values use the default.
    pub size: i64,
}

/// Filters the regional flavor categories by network zone.
#[derive(Clone, Debug, Default)]
pub struct ListFlavorZonesInput {
    /// Optional network zone identifier.
    pub zone_id: String,
}

/// Selects exactly one flavor zone or network zone and optionally filters by name.
#[derive(Clone, Debug, Default)]
pub struct ListFlavorsInput {
    /// Flavor zone identifier.
    pub flavor_zone_id: String,
    /// Optional network zone identifier.
    pub zone_id: String,
    /// Optional exact name filter.
    pub name: String,
}

/// Result of the list servers operation.
pub type ListServersOutput = PagedList<Server>;

/// Result of the get server operation.
#[derive(Clone, Debug, Serialize)]
pub struct GetServerOutput {
    /// The resource returned by the service.
    #[serde(rename = "Server")]
    pub server: Server,
}

crate::wire::product::output::public_fields!(GetServerOutput { server });

/// Result of the get server console log operation.
#[derive(Clone, Debug, Serialize)]
pub struct GetServerConsoleLogOutput {
    /// Console output. Ordinary formatting redacts it.
    #[serde(rename = "Log")]
    pub log: Secret,
}

crate::wire::product::output::public_fields!(GetServerConsoleLogOutput { log });

/// Result of the list ssh keys operation.
pub type ListSshKeysOutput = PagedList<SshKey>;

/// Result of the get ssh key operation.
#[derive(Clone, Debug, Serialize)]
pub struct GetSshKeyOutput {
    /// The resource returned by the service.
    #[serde(rename = "SSHKey")]
    pub ssh_key: SshKey,
}

crate::wire::product::output::public_fields!(GetSshKeyOutput { ssh_key });

/// Result of the list server groups operation.
pub type ListServerGroupsOutput = PagedList<ServerGroup>;

/// Result of the get server group operation.
#[derive(Clone, Debug, Serialize)]
pub struct GetServerGroupOutput {
    /// The resource returned by the service.
    #[serde(rename = "ServerGroup")]
    pub server_group: ServerGroup,
}

crate::wire::product::output::public_fields!(GetServerGroupOutput { server_group });

/// Result of the list server security groups operation.
pub type ListServerSecurityGroupsOutput = List<ServerSecurityGroup>;

/// Result of the list server group members operation.
pub type ListServerGroupMembersOutput = List<ServerGroupMembership>;

/// Result of the list server group policies operation.
pub type ListServerGroupPoliciesOutput = List<ServerGroupPolicy>;

/// Result of the list os images operation.
pub type ListOsImagesOutput = List<OsImage>;

/// Result of the list gpu images operation.
pub type ListGpuImagesOutput = List<OsImage>;

/// Result of the list user images operation.
pub type ListUserImagesOutput = PagedList<UserImage>;

/// Result of the list flavor zones operation.
pub type ListFlavorZonesOutput = List<FlavorZone>;

/// Result of the list flavors operation.
pub type ListFlavorsOutput = List<Flavor>;
