use crate::config::PublicConfig;
use crate::helpers::LangVariant;
use crate::http::posts::models::BoardPostIdModel;
use crate::http::util::{JsonE, User, VialoError, grab_trans};
use crate::permissions::{AppRole, check_app_role, check_manager_of_group};
use crate::{AppState, list_i18n_generic};
use axum::Extension;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use sqlx::{query_as, types::JsonValue};
use sqlx_conditional_queries::conditional_query_as;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

#[utoipa::path(get, path = "/config", responses((status = 200, description = "OK",  body = PublicConfig)))]
pub async fn get_config(
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    Ok(Json(PublicConfig::from(&data.config)))
}
