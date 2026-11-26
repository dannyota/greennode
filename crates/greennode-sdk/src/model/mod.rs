pub(crate) mod pricing;
pub(crate) mod server;
pub(crate) mod wire;

use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::wire::strict_json::field_matches;

/// A project returned by the project service.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Project {
    /// Project identifier.
    #[serde(rename = "ID")]
    pub id: String,
    /// Project name.
    #[serde(rename = "Name")]
    pub name: String,
    /// Project region.
    #[serde(rename = "Region")]
    pub region: String,
    /// Project status.
    #[serde(rename = "Status")]
    pub status: String,
    /// Whether this is the default project.
    #[serde(rename = "IsDefault")]
    pub is_default: bool,
    /// Project user identifier.
    #[serde(rename = "UserID")]
    pub user_id: i64,
}

impl<'de> Deserialize<'de> for Project {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ProjectVisitor;

        impl<'de> Visitor<'de> for ProjectVisitor {
            type Value = Project;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a project object")
            }

            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Project, M::Error> {
                let fields = [
                    "projectId",
                    "name",
                    "region",
                    "status",
                    "isDefault",
                    "userId",
                ];
                let mut seen = [false; 6];
                let mut project = Project::default();
                while let Some(key) = map.next_key::<String>()? {
                    let Some((field, seen)) = fields
                        .into_iter()
                        .zip(&mut seen)
                        .find(|(field, _)| field_matches(field, &key))
                    else {
                        map.next_value::<IgnoredAny>()?;
                        continue;
                    };
                    if std::mem::replace(seen, true) {
                        return Err(serde::de::Error::duplicate_field(field));
                    }
                    match field {
                        "projectId" => {
                            project.id = map.next_value::<Option<_>>()?.unwrap_or_default()
                        }
                        "name" => project.name = map.next_value::<Option<_>>()?.unwrap_or_default(),
                        "region" => {
                            project.region = map.next_value::<Option<_>>()?.unwrap_or_default()
                        }
                        "status" => {
                            project.status = map.next_value::<Option<_>>()?.unwrap_or_default()
                        }
                        "isDefault" => {
                            project.is_default = map.next_value::<Option<_>>()?.unwrap_or_default()
                        }
                        "userId" => {
                            project.user_id = map.next_value::<Option<_>>()?.unwrap_or_default()
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(project)
            }
        }

        deserializer.deserialize_map(ProjectVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_server_keeps_wire_shapes_and_go_projection() {
        let wire = r#"{"uuid":"server_1","secGroups":[],"serverGroupId":{"key":true},"flavor":{"cpu":-0},"internalInterfaces":[{"floatingIp":"::ffff:192.0.2.1"},{"floatingIp":"192.0.2.1"}]}"#;
        let server: crate::model::server::Server = serde_json::from_str(wire).unwrap();
        let output = serde_json::to_value(server).unwrap();
        assert_eq!(output["UUID"], "server_1");
        assert_eq!(output["PublicIP"], "192.0.2.1");
        assert_eq!(output["SecurityGroups"], serde_json::json!([]));
        assert_eq!(output["ExternalInterfaces"], serde_json::json!([]));
        assert_eq!(output["Flavor"]["CPU"], 0);
        assert_eq!(output["ServerGroupID"]["key"], true);
    }

    #[test]
    fn shared_server_rejects_ambiguous_and_non_numeric_wire_values() {
        for wire in [
            r#"{"zone":{"name":"one","NAME":"two"}}"#,
            r#"{"flavor":{"cpu":{"$serde_json::private::Number":"1"}}}"#,
        ] {
            assert!(serde_json::from_str::<crate::model::server::Server>(wire).is_err());
        }
    }

    #[test]
    fn project_list_matches_go_golden() {
        let wire = br#"{"projectId":"project-1","name":"prod","region":"hcm-3","status":"ACTIVE","isDefault":true,"userId":100}"#;
        let project: Project = serde_json::from_slice(wire).unwrap();
        let actual = serde_json::to_value(List {
            items: vec![project],
        })
        .unwrap();
        let golden: serde_json::Value = serde_json::from_str(r#"{"Items":[{"ID":"project-1","Name":"prod","Region":"hcm-3","Status":"ACTIVE","IsDefault":true,"UserID":100}]}"#).unwrap();
        assert_eq!(actual, golden);
        let empty = serde_json::to_value(Project::default()).unwrap();
        assert_eq!(empty.as_object().unwrap().len(), 6);
    }
}

/// Default one-based page number.
pub const DEFAULT_PAGE: i64 = 1;
/// Default maximum number of items requested per page.
pub const DEFAULT_PAGE_SIZE: i64 = 10_000;

/// Items returned without pagination metadata; serialized keys use the service format.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct List<T> {
    /// Items in this result.
    #[serde(rename = "Items")]
    pub items: Vec<T>,
}

/// Items and pagination metadata, serialized with service-format keys.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PagedList<T> {
    /// Items in this result.
    #[serde(rename = "Items")]
    pub items: Vec<T>,
    /// One-based page number.
    #[serde(rename = "Page")]
    pub page: i64,
    /// Requested number of items per page.
    #[serde(rename = "PageSize")]
    pub page_size: i64,
    /// Total number of pages.
    #[serde(rename = "TotalPage")]
    pub total_page: i64,
    /// Total number of items across pages.
    #[serde(rename = "TotalItem")]
    pub total_item: i64,
}
