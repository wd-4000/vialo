use std::sync::Arc;

use axum::{
    Router,
    middleware::{self},
    routing::{delete, get, post, put},
};

use crate::{
    AppState,
    permissions::{AppRole, require_app_role},
};

pub mod groups;
pub mod handlers;
pub mod identities;
pub mod models;
pub mod rooms;
pub mod schemas;
pub mod transactions;

#[cfg(feature = "printer")]
pub mod printer;

pub fn create_router(app_state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Routes accessible to any authenticated user about themselves
    let me_routes = Router::new()
        .route("/me", get(handlers::get_me))
        .route("/me/roles", get(handlers::get_person_roles_me))
        .route("/me/capabilities", get(handlers::get_person_capabilities_me));

    // Routes gated behind CreditManager
    let credit_routes = Router::new()
        .route("/transactions", get(transactions::list).post(transactions::post))
        .route("/transactions/{id}", get(transactions::get))
        .route("/transactions/{id}/undo", post(transactions::undo))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            |state, ext, req, next| require_app_role(AppRole::CreditManager, state, ext, req, next),
        ));

    // Routes gated behind AccountManager
    let account_routes = Router::new()
        .route("/", post(handlers::add_person).get(handlers::list_people))
        .route("/rooms", post(rooms::add_room).get(rooms::list_rooms))
        .route(
            "/rooms/{id}",
            get(rooms::get_room)
                .put(rooms::put_room)
                .delete(rooms::delete_room),
        )
        .route(
            "/identities/{id}",
            get(identities::get).delete(identities::delete),
        )
        .route("/identities/{id}/merge", post(identities::merge))
        .route("/{id}", get(handlers::get_person))
        .route(
            "/{id}",
            put(handlers::put_person).delete(handlers::delete_person),
        )
        .route("/{id}/overview", get(handlers::get_person_overview))
        .route("/{id}/roles", get(handlers::get_person_roles_by_id))
        .route("/{id}/groups", get(handlers::get_person_groups))
        .route("/{id}/app_roles", get(handlers::get_person_app_roles))
        .route(
            "/{id}/credits",
            post(handlers::enable_credits).get(handlers::get_credits),
        )
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            |state, ext, req, next| {
                require_app_role(AppRole::AccountManager, state, ext, req, next)
            },
        ));

    // Group read routes — accessible to any authenticated user.
    // Handlers themselves filter private groups for non-managers.
    let group_read_routes = Router::new()
        .route("/groups", get(groups::list_groups))
        .route("/groups/members", get(groups::list_all_group_members))
        .route("/groups/{id}", get(groups::get_group))
        .route(
            "/groups/{group_id}/members",
            get(groups::list_group_members),
        );

    // Group write routes — AccountManager OR group manager (checked per-handler).
    let group_write_routes = Router::new()
        .route("/groups", post(groups::post_group))
        .route(
            "/groups/{id}",
            put(groups::put_group).delete(groups::delete_group),
        )
        .route("/groups/{group_id}/members", post(groups::add_account))
        .route(
            "/groups/{group_id}/members/{account_id}",
            delete(groups::delete_account).patch(groups::patch_account),
        );

    let mut builder = Router::new()
        .merge(me_routes)
        .merge(credit_routes)
        .merge(account_routes)
        .merge(group_read_routes)
        .merge(group_write_routes);

    #[cfg(feature = "printer")]
    {
        builder = builder
            .route("/printer", get(printer::list))
            .route(
                "/{id}/printer",
                get(printer::get)
                    .post(printer::link_or_create)
                    .delete(printer::delete),
            )
            .route("/{id}/printer/unlink", post(printer::unlink));
    }

    builder
}
