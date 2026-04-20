use axum::{
    Router,
    routing::get,
};
use std::sync::Arc;

use crate::AppState;


pub mod handlers;
pub mod models;
pub mod schemas;

pub fn create_router(_app_state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(handlers::get_home_aggregated))
        .route("/quicklinks", get(handlers::list_quicklinks))
        .route("/jumbo", get(handlers::list_jumbo))
}
