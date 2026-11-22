use std::sync::Arc;

use axum::{
    Router,
    extract::{MatchedPath, Request},
    http::header::AUTHORIZATION,
    middleware::{self, Next},
    response::Response,
    routing::post,
};
use tower_http::trace::TraceLayer;

use crate::helpers::{self, grab_authd_conn_subsystem, people::IdentityModel};
use crate::http::util::{JsonE, VialoError};
use axum::{extract::State, http::StatusCode, response::IntoResponse};

pub mod radius;

pub async fn update_identity(
    State(data): State<Arc<AppState>>,
    JsonE(body): JsonE<IdentityModel>,
) -> Result<impl IntoResponse, VialoError> {
    let mut conn = grab_authd_conn_subsystem(&data.db, "app").await?;
    let mut trans = crate::http::util::grab_trans(&mut conn).await?;
    helpers::people::update_identity(body, &mut trans)
        .await
        .map_err(VialoError::Anyhow)?;
    trans.commit().await?;
    Ok(StatusCode::OK)
}

use super::AppState;

/// Shared secret Kratos and FreeRADIUS send as `Authorization: Bearer <key>`.
/// None (debug builds only) leaves the hooks open.
pub type HooksKey = Option<Arc<str>>;

/// Constant-time, so the key can't be recovered from response timing
fn key_matches(given: &[u8], key: &[u8]) -> bool {
    given.len() == key.len() && given.iter().zip(key).fold(0, |acc, (a, b)| acc | (a ^ b)) == 0
}

async fn require_key(
    State(key): State<HooksKey>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(key) = key {
        let given = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|v| v.as_bytes().strip_prefix(b"Bearer "));
        if !given.is_some_and(|given| key_matches(given, key.as_bytes())) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(next.run(req).await)
}

pub async fn create_router(app_state: Arc<AppState>, hooks_key: HooksKey) -> Router {
    Router::new()
        .route("/hooks/update_identity", post(update_identity))
        .route(
            "/radius/networks/{network_id}/authorize",
            post(radius::authorize),
        )
        .route(
            "/radius/networks/{network_id}/post-auth",
            post(radius::post_auth),
        )
        .layer(middleware::from_fn_with_state(hooks_key, require_key))
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
