pub mod appointments;
pub mod connectors;
pub mod handlers;
pub mod models;
pub mod schema_assignments;
pub mod schemas;
pub mod slots;

mod asset_types;

use std::sync::Arc;

use axum::{
    Router,
    handler::Handler,
    middleware::{self},
    routing::{delete, get, post, put},
};

use crate::AppState;

pub fn create_router(app_state: Arc<AppState>) -> Router<Arc<AppState>> {
    return Router::new()
        .route(
            "/",
            post(handlers::post_bookable).get(handlers::list_bookables),
        )
        .route("/types", post(asset_types::post).get(asset_types::list))
        .route(
            "/types/{id}",
            put(asset_types::put)
                .get(asset_types::get)
                .delete(asset_types::delete),
        )
        .route("/appointments", get(appointments::list))
        .route("/connectors", get(connectors::list).post(connectors::post))
        .route(
            "/connectors/{id}",
            get(connectors::get)
                .put(connectors::put)
                .delete(connectors::delete),
        )
        .route("/schemas", get(schemas::list).post(schemas::post))
        .route(
            "/schemas/{id}",
            get(schemas::get).put(schemas::put).delete(schemas::delete),
        )
        .route("/appointments/{id}/activate", post(appointments::activate))
        .route("/slots/taken", get(slots::taken_slots))
        .route("/slots/schemas", get(slots::slot_schemas))
        .route("/slots/book", post(slots::book_slots))
        .route(
            "/{id}",
            get(handlers::get_bookable).put(handlers::put_bookable),
        )
        .route(
            "/{asset_id}/schema_assignments",
            get(schema_assignments::list).post(schema_assignments::post),
        )
        .route(
            "/{asset_id}/schema_assignments/{begins}",
            delete(schema_assignments::delete),
        )
        .route(
            "/{id}/quick-unlock",
            post(handlers::quick_unlock.layer(middleware::from_fn_with_state(
                app_state.clone(),
                super::util::middleware::auth_middleware,
            ))),
        );
}
