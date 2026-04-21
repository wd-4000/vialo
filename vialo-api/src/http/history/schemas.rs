use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, Debug, Default, ToSchema, IntoParams)]
pub struct HistoryFilterOptions {
    pub page: Option<i64>,
    pub limit: Option<i64>,
    pub table: Option<Vec<String>>,
    pub search: Option<String>,
}
