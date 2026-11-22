use std::sync::Arc;

use axum::{
    Router,
    extract::{MatchedPath, Request},
    middleware::{self},
    routing::get,
};
use tower_http::trace::TraceLayer;
mod apps;
pub mod bookables;
mod config;
pub mod docs;
mod etc;
mod health;
pub mod history;
mod home;
pub mod media;
pub mod network;
pub mod people;
pub mod posts;
pub mod rate_limit;
pub mod util;

use super::AppState;

pub async fn create_router(app_state: Arc<AppState>) -> Router {
    Router::new()
        .nest("/network", network::create_router(app_state.clone()))
        .nest("/history", history::create_router(app_state.clone()))
        .nest("/health", health::create_router(app_state.clone()))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            util::middleware::auth_required,
        ))
        .nest("/people", people::create_router(app_state.clone()))
        .nest("/bookables", bookables::create_router(app_state.clone()))
        .nest("/posts", posts::create_router(app_state.clone()))
        .nest("/media", media::create_router(app_state.clone()))
        .nest("/home", home::create_router(app_state.clone()))
        .nest("/apps", apps::create_router(app_state.clone()))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            rate_limit::authenticated,
        ))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            rate_limit::observe,
        ))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            util::middleware::auth_middleware,
        ))
        .route("/config", get(config::get_config))
        .route("/etc/mail-recipients", get(etc::list_mail_recipients))
        // TODO expose openapi.json (Maybe) .route("/openapi.json", get(docs::openapi_json))
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            rate_limit::global,
        ))
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
