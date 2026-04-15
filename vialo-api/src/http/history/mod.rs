use std::sync::Arc;

use axum::{
    Router,
    middleware::{self},
    routing::get,
};

use crate::{
    AppState,
    permissions::{AppRole, require_app_role},
};

pub mod handlers;
pub mod models;
pub mod schemas;

pub fn create_router(app_state: Arc<AppState>) -> Router<Arc<AppState>> {
    return Router::new()
        .route("/", get(handlers::list_history))
        .route("/schema", get(handlers::list_schema))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            |state, ext, req, next| require_app_role(AppRole::HistoryViewer, state, ext, req, next),
        ));
}
