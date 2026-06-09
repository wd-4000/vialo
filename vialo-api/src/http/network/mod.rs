use crate::AppState;
use crate::http::rate_limit;
use crate::permissions::{AppRole, require_app_role};
use std::sync::Arc;

use axum::{
    Router,
    middleware::{self},
    routing::{get, patch, post, put},
};

pub mod credentials;
pub mod devices;
pub mod mac;
pub mod models;
pub mod networks;
pub mod nms_connectors;
pub mod realms;
pub mod schemas;

pub fn create_router(app_state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Fully public — needed by the user-ui to list available networks
    let public_routes = Router::new().route("/networks", get(networks::list_networks));

    // Staff-only infrastructure management
    let manager_routes = Router::new()
        .route(
            "/networks/{id}",
            get(networks::get_network).put(networks::put_network).delete(networks::delete_network),
        )
        .route("/networks", post(networks::post_network))
        .route("/realms", get(realms::list_realms))
        .route("/realms/set_default", post(realms::set_default_realm))
        .route("/realms/{id}", put(realms::put_realm))
        .route("/credentials", get(credentials::list_credentials))
        .route(
            "/nms-connectors",
            get(nms_connectors::list).post(nms_connectors::post),
        )
        .route(
            "/nms-connectors/{id}",
            get(nms_connectors::get)
                .put(nms_connectors::put)
                .delete(nms_connectors::delete),
        )
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            |state, ext, req, next| {
                require_app_role(AppRole::NetworkManager, state, ext, req, next)
            },
        ));

    // Per-credential routes: ownership checked inside the handlers.
    // POST /devices is here because CreatedDeviceResponse always includes a
    // plaintext credential — it must share the tight credential rate limit.
    let credential_routes = Router::new()
        .nest(
            "/credentials/{id}",
            Router::new()
                .route(
                    "/",
                    get(credentials::get_credential)
                        .put(credentials::put_credential)
                        .delete(credentials::delete_credential),
                )
                .route(
                    "/mobileconfig/{filename}",
                    get(credentials::get_mobileconfig),
                ),
        )
        .route(
            "/devices/{id}/overview",
            get(devices::handlers::get_device_overview),
        )
        .route("/devices", post(devices::handlers::post_device))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            rate_limit::credential,
        ));

    // Realm read: ownership checked inside the handler
    let realm_read_routes = Router::new().route("/realms/{id}", get(realms::get_realm));

    // Device routes: ownership checked inside the handlers
    let device_routes = Router::new()
        .route("/devices", get(devices::handlers::list_devices))
        .route(
            "/devices/{id}",
            patch(devices::handlers::update_device).delete(devices::handlers::delete_device),
        );

    Router::new()
        .merge(public_routes)
        .merge(manager_routes)
        .merge(credential_routes)
        .merge(realm_read_routes)
        .merge(device_routes)
}
