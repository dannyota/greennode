use serde::Serialize;

/// A project returned by the project service.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Project {
    /// Project identifier.
    #[serde(rename(serialize = "ID", deserialize = "projectId"))]
    pub id: String,
    /// Project name.
    #[serde(rename(serialize = "Name"))]
    pub name: String,
    /// Project region.
    #[serde(rename(serialize = "Region"))]
    pub region: String,
    /// Project status.
    #[serde(rename(serialize = "Status"))]
    pub status: String,
    /// Whether this is the default project.
    #[serde(rename(serialize = "IsDefault", deserialize = "isDefault"))]
    pub is_default: bool,
    /// Project user identifier.
    #[serde(rename(serialize = "UserID", deserialize = "userId"))]
    pub user_id: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

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
