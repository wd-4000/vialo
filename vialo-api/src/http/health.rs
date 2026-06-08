use crate::{
    AppState,
    http::{
        history::models::Subsystem,
        util::{ListOptions, VialoError, clamp_pagination},
    },
    permissions::{AppRole, require_app_role},
};
use axum::{Json, extract::State, middleware, response::IntoResponse};
use axum::{Router, routing::get};
use axum_extra::extract::Query;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

#[utoipa::path(get, path = "/health/status", responses((status = 200, description = "OK", body=i64)))]
pub async fn get_health_status(
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let badness = sqlx::query_scalar!(
        r#"SELECT sum(badness) FROM health_events WHERE (NOW() - created_at >= '1 hour' AND badness > 0 AND resolved = false AND read = false) OR badness > 99"#,
    )
    .fetch_optional(&data.db)
    .await?.flatten().unwrap_or(0);

    Ok(Json(badness))
}

#[derive(Deserialize, Serialize, ToSchema)]
pub struct HealthEvent {
    pub id: i32,
    pub created_at: DateTime<Utc>,
    pub last_updated: Option<DateTime<Utc>>,
    pub subsystem: Option<Subsystem>,
    pub label: String,
    pub data: Option<serde_json::Value>,
    pub badness: i32,
    pub read: bool,
    pub resolved: bool,
}

#[utoipa::path(get, path = "/health", params(ListOptions), responses((status = 200, description = "OK", body=Vec<HealthEvent>)))]
pub async fn get_health_events(
    Query(opts): Query<ListOptions>,
    State(data): State<Arc<AppState>>,
) -> Result<impl IntoResponse, VialoError> {
    let (offset, limit) = clamp_pagination(opts.limit, opts.page)?;

    let events = sqlx::query_as!(
        HealthEvent,
        r#"SELECT id, created_at, last_updated, subsystem AS "subsystem: Subsystem", label, data, badness, read, resolved
        FROM health_events ORDER BY id DESC LIMIT $1 OFFSET $2"#,
        limit,
        offset
    )
    .fetch_all(&data.db)
    .await?;

    Ok(Json(events))
}

pub fn create_router(app_state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(get_health_events))
        .route("/status", get(get_health_status))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            |state, ext, req, next| require_app_role(AppRole::HealthViewer, state, ext, req, next),
        ))
}
