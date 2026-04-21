use std::sync::Arc;

use axum::{
    Router,
    extract::{MatchedPath, Request},
    middleware::{self},
    routing::get,
};
use tower_http::trace::TraceLayer;
pub mod bookables;
mod config;
pub mod docs;
mod etc;
mod health;
pub mod history;
mod home;
mod network;
mod people;
pub mod posts;
pub mod util;

use super::AppState;

pub async fn create_router(app_state: Arc<AppState>) -> Router {
    // let file1 =
    //     std::fs::File::open("sampledata/nachnamen.json").expect("file should open read only");
    // let json1: serde_json::Value =
    //     serde_json::from_reader(file1).expect("file should be proper JSON");
    // let file2 =
    //     std::fs::File::open("sampledata/vornamen_w.json").expect("file should open read only");
    // let mut json2: serde_json::Value =
    //     serde_json::from_reader(file2).expect("file should be proper JSON");
    // let file3 =
    //     std::fs::File::open("sampledata/vornamen_m.json").expect("file should open read only");
    // let mut json3: serde_json::Value =
    //     serde_json::from_reader(file3).expect("file should be proper JSON");

    // if let (Some(nachnamen), Some(vornamen_w), Some(vornamen_m)) =
    //     (json1.as_array(), json2.as_array_mut(), json3.as_array_mut())
    // {
    //     vornamen_w.extend_from_slice(vornamen_m);
    //     for i in 1..15 {
    //         for j in 1..15 {
    //             if let (Some(vorname), Some(nachname)) = (
    //                 vornamen_w.choose(&mut rand::rng()).unwrap().as_str(),
    //                 nachnamen.choose(&mut rand::rng()).unwrap().as_str(),
    //             ) {
    //                 let result = people::handlers::add_person(
    //                     State(app_state.clone()),
    //                     axum::Extension(util::User {
    //                         id: uuid::uuid!("77159bd3-0919-4279-979c-d830762627d6"),
    //                     }),
    //                     util::JsonE(people::schemas::CreateUserSchema {
    //                         full_name: format!("{} {}", vorname, nachname),
    //                         label: None,
    //                         auth_id: None,
    //                         membership_end: PgDate::DateTime(Utc::now().date_naive()),
    //                         email: Some(format!("{}.{}@uni.example.com", vorname, nachname)),
    //                         contract: Some(people::schemas::ContractInfo {
    //                             room: format!("{:02}{:02}", i, j),
    //                             begins: Utc::now().date_naive(),
    //                             ends: Utc::now().date_naive(),
    //                             lessor: None,
    //                         }),
    //                     }),
    //                 )
    //                 .await;
    //                 println!("{:?}", result.err())
    //             }
    //         }
    //     }
    // }

    // println!("Inserterd stuff");

    Router::new()
        .nest("/people", people::create_router(app_state.clone()))
        .nest("/bookables", bookables::create_router(app_state.clone()))
        .nest("/network", network::create_router(app_state.clone()))
        .nest("/history", history::create_router(app_state.clone()))
        .nest("/health", health::create_router(app_state.clone()))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            util::middleware::auth_required,
        ))
        .nest("/posts", posts::create_router(app_state.clone()))
        .nest("/home", home::create_router(app_state.clone()))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            util::middleware::auth_middleware,
        ))
        .route("/config", get(config::get_config))
        .route("/etc/mail-recipients", get(etc::list_mail_recipients))
        // TODO expose openapi.json (Maybe) .route("/openapi.json", get(docs::openapi_json))
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
