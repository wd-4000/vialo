// #![deny(unused_crate_dependencies)]
mod bookable_connectors;
mod config;
mod dump;
mod events;
mod health;
mod helpers;
mod hooks;
mod http;
mod permissions;
mod ws;

#[cfg(feature = "ppsk")]
mod ppsk;

#[cfg(feature = "migrate")]
use sqlx::migrate;
#[cfg(feature = "email")]
mod email;
// #[cfg(feature = "printer")]
// mod printer;

use crate::{
    config::{AuthConfig, Config},
    helpers::grab_authd_conn_subsystem,
    http::{bookables::models::BookableAssetStatus, util::grab_trans},
};
use axum::http::{
    HeaderValue, Method,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE},
};
use dotenv::dotenv;
use events::EventChannel;
use sqlx::Executor;
use sqlx::{Pool, Postgres, postgres::PgPoolOptions};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub struct KratosConfigs {
    frontend: ory_kratos_client::apis::configuration::Configuration,
    admin: ory_kratos_client::apis::configuration::Configuration,
}
pub struct AppState {
    pub db: Pool<Postgres>,
    pub event_channel: EventChannel<i32, BookableAssetStatus>,
    pub config: Config,
    pub kratos_config: Option<KratosConfigs>,
}

#[macro_export]
macro_rules! list_i18n_generic {
    // This variant of the macro accepts a custom SQL query and parameters.
    ($db:expr, $query:expr, $opts:expr, $result_type:ty) => {{
        use sqlx::query_as;

        let limit = $opts.limit.unwrap_or(10);
        let langs = $opts.lang.unwrap_or(vec![String::from("en"), String::from("de")]);
        let offset = ($opts.page.unwrap_or(1) - 1) * limit;
        // Execute the query and handle the result
        let record = query_as!($result_type, $query, &langs,
            limit as i32,
            offset as i32)
            .fetch_all($db)
            .await?;

        return Ok((StatusCode::OK, Json(json!({"status": "success","data": record}))));


    }};
}

#[tokio::main]
async fn main() {
    // Read and parse the config
    dotenv().expect("Error reading .env file");
    let config_file = std::fs::read_to_string("vialo.toml").expect("Error reading vialo.toml");
    let config: Config = toml::from_str(&config_file)
        .map_err(|e| {
            let location = e.span().map_or(String::new(), |span| {
                let line = config_file[..span.start].matches('\n').count() + 1;
                format!(" around line {}", line)
            });
            format!("{} in vialo.toml{}", e.message(), location)
        })
        .expect("Error reading config");

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                format!("{}=debug,tower_http=debug", env!("CARGO_CRATE_NAME")).into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Connect to the Postgres DB
    info!("Connecting to the database...");
    let pool = match PgPoolOptions::new()
        .max_connections(32)
        .min_connections(4)
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                conn.execute("SET TIME ZONE 'Europe/Berlin';").await?;
                Ok(())
            })
        })
        .after_release(|conn, _| {
            Box::pin(async move {
                sqlx::query!("RESET app.account_id;")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query!("RESET app.subsystem;")
                    .execute(&mut *conn)
                    .await?;

                Ok(true)
            })
        })
        .connect(&(std::env::var("DATABASE_URL").expect("DATABASE_URL must be set")))
        .await
    {
        Ok(pool) => {
            let pg_version: String = sqlx::query_scalar("SHOW server_version_num;")
                .fetch_one(&pool)
                .await
                .unwrap();
            if pg_version
                .parse::<u32>()
                .expect("Couldn't parse PostgreSQL version")
                < 180000
            {
                panic!("This software requires PostgreSQL 18 or higher.")
            }

            pool
        }
        Err(err) => {
            error!("Failed to connect to the database: {:?}", err);
            std::process::exit(1);
        }
    };

    let kratos_config = if let AuthConfig::Kratos {
        frontend_url,
        admin_url,
    } = &config.auth
    {
        // Detect and remove trailing slash from base_path
        let frontend_url = frontend_url.trim_end_matches('/').to_string();
        let admin_url = admin_url.trim_end_matches('/').to_string();

        let kratos_config = KratosConfigs {
            frontend: ory_kratos_client::apis::configuration::Configuration {
                base_path: frontend_url.clone(),
                ..Default::default()
            },
            admin: ory_kratos_client::apis::configuration::Configuration {
                base_path: admin_url.clone(),
                ..Default::default()
            },
        };
        Some(kratos_config)
    } else {
        None
    };

    let asp = Arc::new(AppState {
        db: pool.clone(),
        event_channel: EventChannel::new("bookables".into()),
        config,
        kratos_config,
    });

    #[cfg(feature = "ppsk")]
    let ppsk_subsystem_task = tokio::spawn({
        let pool = pool.clone();
        let asp = asp.clone();
        async move {
            loop {
                let result = ppsk::main(pool.clone(), asp.clone()).await;
                tracing::warn!(
                    "PPSK subsystem exited unexpectedly ({:?}), restarting...",
                    result
                );
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    });

    // #[cfg(feature = "printer")]
    // let printer_subsystem_set = tokio::task::LocalSet::new();
    // let printer_subsystem_pool = pool.clone();
    // let printer_subsystem_asp = asp.clone();
    // let printer_subsystem_task = printer_subsystem_set.run_until(async move {
    //     #[cfg(feature = "printer")]
    //     loop {
    //         let result = printer::main(
    //             printer_subsystem_pool.clone(),
    //             printer_subsystem_asp.clone(),
    //         )
    //         .await;
    //         tracing::warn!(
    //             "printer subsystem exited unexpectedly ({:?}), restarting...",
    //             result
    //         );
    //         tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    //     }
    // });

    let bookable_connectors_subsystem_set = tokio::task::LocalSet::new();
    let bookable_connectors_subsystem_asp = asp.clone();
    let bookable_connectors_subsystem_task =
        bookable_connectors_subsystem_set.run_until(async move {
            #[cfg(feature = "bookable_connectors")]
            loop {
                let result =
                    bookable_connectors::main(bookable_connectors_subsystem_asp.clone()).await;
                tracing::warn!(
                    "bookable_connectors exited unexpectedly ({:?}), restarting...",
                    result
                );
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });

    let email_subsystem_set = tokio::task::LocalSet::new();
    let email_subsystem_asp = asp.clone();
    let email_subsystem_task = email_subsystem_set.run_until(async move {
        #[cfg(feature = "email")]
        loop {
            let result = email::main(email_subsystem_asp.clone()).await;
            tracing::warn!(
                "email subsystem exited unexpectedly ({:?}), restarting...",
                result
            );
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });

    #[cfg(feature = "migrate")]
    {
        match migrate!().run(&pool).await {
            Ok(()) => {
                info!("Applied migrations.");
            }
            Err(err) => {
                error!("Failed to apply migrations: {:?}", err);
                std::process::exit(1);
            }
        }
    }

    // Try to bind to required ports already so that we fail fast
    let listener = tokio::net::TcpListener::bind(&asp.config.public.listen)
        .await
        .expect("Couldn't bind to public API port!");

    let hook_listener = tokio::net::TcpListener::bind(&asp.config.hooks.listen)
        .await
        .expect("Couldn't bind to hooks API port!");

    // Check that there are users that are able to log in.
    // If not, we need to bootstrap the first admin user.
    if let AuthConfig::Mock { uuid, email } = &asp.config.auth {
        // If we're using mock auth, it's pretty straightforward – we just need to create an account with the same ID as set in the mock Auth config
        // along with a privileged group.

        if !(sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM accounts_people
                WHERE id = $1
            ) AS "allowed!""#,
            uuid
        )
        .fetch_one(&pool)
        .await
        .unwrap())
        {
            let mut conn = grab_authd_conn_subsystem(&pool, "app").await.unwrap();
            let mut trans = grab_trans(&mut conn).await.unwrap();

            // create the account
            sqlx::query!(
                "INSERT INTO accounts_people (id, full_name, email, label, membership_end) VALUES ($1, 'Admin', $2, 'auto-created during setup', 'infinity')",
                uuid,
                email
            ).execute(&mut *trans).await.unwrap();

            // create the group
            let group_id = sqlx::query_scalar!(
                "insert into account_groups (label,public) values ('AdminAG', true) RETURNING id",
            )
            .fetch_one(&mut *trans)
            .await
            .unwrap();

            // add the account to the group
            sqlx::query!(
                "insert into account_group_memberships (group_id, account_id, role) VALUES ($1, $2, 'manager');", group_id, uuid
            )
            .execute(&mut *trans)
            .await
            .unwrap();

            // give the group every app role
            sqlx::query!(
                "INSERT INTO account_group_app_roles (group_id, role) (select $1 as group_id, unnest(enum_range(null, null::app_role)) as role)", group_id
            ).execute(&mut *trans).await.unwrap();

            trans.commit().await.unwrap();

            info!("Created admin account with ID {}", uuid);
        }
    } else {
        // If we're using Kratos, we have to check whether INITIAL_ADMIN_EMAIL has been set first
        if !sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM accounts_people WHERE auth_id IS NOT NULL
            ) AS "exists!""#,
        )
        .fetch_one(&pool)
        .await
        .unwrap()
        {
            if let Ok(initial_admin_email) = std::env::var("INITIAL_ADMIN_EMAIL") {
                info!(
                    "Account with Email {initial_admin_email} will be set up as an admin account as soon as they sign up."
                );
                // See hooks/mod.rs for the implementation of this
                info!("Waiting to receive webhook from Kratos...");
            } else {
                panic!(
                    "Set the INITIAL_ADMIN_EMAIL environment variable for the application to set up your admin account."
                )
            }
        }
    };

    let app = http::create_router(asp.clone()).await;
    let hook_app = hooks::create_router(asp.clone()).await;
    let ws = ws::main(asp.clone());

    info!("Serving.");

    let _ = tokio::join!(
        axum::serve(
            listener,
            app.merge(ws)
                .layer(
                    CorsLayer::new()
                        .allow_origin(
                            asp.config
                                .public
                                .cors_origins
                                .iter()
                                .map(|v| v.parse::<HeaderValue>().unwrap())
                                .collect::<Vec<_>>(),
                        )
                        .allow_methods([
                            Method::GET,
                            Method::POST,
                            Method::PATCH,
                            Method::DELETE,
                            Method::PUT,
                        ])
                        .allow_credentials(true)
                        .allow_headers([AUTHORIZATION, ACCEPT, CONTENT_TYPE])
                )
                .into_make_service_with_connect_info::<SocketAddr>()
        ),
        axum::serve(
            hook_listener,
            hook_app.into_make_service_with_connect_info::<SocketAddr>()
        ),
        // printer_subsystem_task,
        ppsk_subsystem_task,
        email_subsystem_task,
        bookable_connectors_subsystem_task
    );
}
