use std::sync::Arc;

use axum::handler::Handler;
use axum::{
    Router,
    middleware::{self},
    routing::{delete, get, post, put},
};

use crate::AppState;
use crate::http::util;

pub mod boards;
pub mod handlers;
pub mod models;
pub mod schemas;

pub fn create_router(app_state: Arc<AppState>) -> Router<Arc<AppState>> {
    return Router::new()
        .route(
            "/{id}",
            put(handlers::update_post).delete(handlers::delete_post),
        )
        .route("/boards", get(boards::list_boards).post(boards::add_board))
        .route(
            "/boards/{id}",
            get(boards::get_board).put(boards::put_board),
        )
        .route("/boards/{id}/permissions", get(boards::get_permissions))
        .route(
            "/boards/{board_id}/permissions/{group_id}",
            put(boards::put_permission).delete(boards::delete_permission),
        )
        .route("/boards/{id}/pin", post(boards::pin_post))
        .route(
            "/boards/{board_id}/pin/{post_id}",
            delete(boards::unpin_post),
        )
        .route("/", post(handlers::add_post))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            util::middleware::auth_required,
        ))
        .route("/", get(handlers::list_posts))
        .route("/{id}", get(handlers::get_post));
}
