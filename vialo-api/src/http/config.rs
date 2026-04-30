use crate::config::PublicConfig;
use crate::http::util::VialoError;
use crate::AppState;
use axum::{
    Json,
    extract::State,
    response::IntoResponse,
};
use std::sync::Arc;

#[utoipa::path(get, path = "/config", responses((status = 200, description = "OK",  body = PublicConfig)))]
pub async fn get_config(
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    Ok(Json(PublicConfig::from(&data.config)))
}
