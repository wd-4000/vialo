
use serde::Deserialize;

#[derive(Deserialize, Debug, Default)]
pub struct HistoryFilterOptions {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub table: Option<Vec<String>>,
    pub search: Option<String>,
}
