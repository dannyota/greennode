use serde::Serialize;

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
