use crate::{helpers::PgDateTime, http::bookables::models::BookableStatus};
use chrono::NaiveDateTime;
use serde::Serialize;

#[derive(Serialize, Debug)]
pub struct BookableAssetStatusWithConnector {
    pub id: Option<i32>,
    pub status: BookableStatus,
    pub begins: Option<NaiveDateTime>,
    pub ends: Option<PgDateTime>,
    pub connector: i32,
    pub connector_output_id: i32,
}
