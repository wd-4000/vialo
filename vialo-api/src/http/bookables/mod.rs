pub mod appointments;
pub mod connectors;
pub mod handlers;
pub mod models;
pub mod permissions;
pub mod schema_assignments;
pub mod schemas;
pub mod slots;

pub mod asset_types;

use std::sync::Arc;

use axum::{
    Router,
    middleware::{self},
    routing::{delete, get, post, put},
};

use crate::{AppState, http::rate_limit};

pub fn create_router(app_state: Arc<AppState>) -> Router<Arc<AppState>> {
    let booking_routes = Router::new()
        .route("/slots/book", post(slots::book_slots))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            rate_limit::credits,
        ));

    Router::new()
        .route("/", post(handlers::post_bookable))
        .route("/types", post(asset_types::post))
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
        .route(
            "/appointments/{id}",
            delete(appointments::delete_appointment),
        )
        .route("/slots/taken", get(slots::taken_slots))
        .route("/slots/schemas", get(slots::slot_schemas))
        .merge(booking_routes)
        .route("/{id}", put(handlers::put_bookable))
        .route("/{id}/schema_assignments", post(schema_assignments::post))
        .route(
            "/{id}/schema_assignments/{begins}",
            delete(schema_assignments::delete),
        )
        .route("/{id}/schema_assignments", get(schema_assignments::list))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            super::util::middleware::auth_required,
        ))
        .route("/", get(handlers::list_bookables))
        .route("/types", get(asset_types::list))
        .route("/queues", get(handlers::list_queues))
        .route("/{id}", get(handlers::get_bookable))
        .route("/{id}/quick-unlock", post(handlers::quick_unlock))
        .route("/appointments/{id}/activate", post(appointments::activate))
}
