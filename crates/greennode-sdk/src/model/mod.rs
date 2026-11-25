use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct List<T> {
    #[serde(rename = "Items")]
    pub items: Vec<T>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PagedList<T> {
    #[serde(rename = "Items")]
    pub items: Vec<T>,
    #[serde(rename = "Page")]
    pub page: i64,
    #[serde(rename = "PageSize")]
    pub page_size: i64,
    #[serde(rename = "TotalPage")]
    pub total_page: i64,
    #[serde(rename = "TotalItem")]
    pub total_item: i64,
}
