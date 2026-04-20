use std::sync::Arc;

use axum::handler::Handler;

use axum::{
    Router,
    extract::{MatchedPath, Request},
    routing::post,
};
use tower_http::trace::TraceLayer;

use crate::helpers;
use crate::helpers::grab_authd_conn_subsystem;
use crate::http::util::{JsonE, VialoError};
use axum::{extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, FromRow)]
pub struct IdentityModel {
    pub id: Option<Uuid>,
    pub full_name: Option<String>,
    pub email: Option<String>,
}

pub async fn update_identity(
    State(data): State<Arc<AppState>>,
    JsonE(body): JsonE<IdentityModel>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_subsystem(&data.db, "app").await?;
    let mut trans = crate::http::util::grab_trans(&mut conn).await?;
    helpers::people::update_identity(body, &mut trans)
        .await
        .map_err(|e| VialoError::Anyhow(e))?;
    trans.commit().await?;
    Ok(StatusCode::OK)
}

use super::AppState;

pub async fn create_router(app_state: Arc<AppState>) -> Router {
    Router::new()
        .route("/hooks/update_identity", post(update_identity))
        .layer(
            TraceLayer::new_for_http()
                // Create our own span for the request and include the matched path. The matched
                // path is useful for figuring out which handler the request was routed to.
                .make_span_with(|req: &Request| {
                    let method = req.method();
                    let uri = req.uri();

                    // axum automatically adds this extension.
                    let matched_path = req
                        .extensions()
                        .get::<MatchedPath>()
                        .map(|matched_path| matched_path.as_str());

                    tracing::debug_span!("request", %method, %uri, matched_path)
                })
                // By default `TraceLayer` will log 5xx responses but we're doing our specific
                // logging of errors so disable that
                .on_failure(()),
        )
        .with_state(app_state)
}
