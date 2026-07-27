use axum::http::{
    HeaderValue, Method,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE},
};
use dotenv::dotenv;
use sqlx::postgres::PgPoolOptions;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::broadcast;
use tokio::sync::watch;
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use vialo_api::{
    AppState, EventChannels, KratosConfigs,
    bookables::BookableChannel,
    config::{AuthConfig, Config},
    helpers::{encryption, find_upward, grab_authd_conn_subsystem},
    http::util::grab_trans,
};

#[cfg(unix)]
use std::os::unix::io::FromRawFd;

#[cfg(feature = "migrate")]
use sqlx::migrate;

/// How long every task gets to wind down after a shutdown signal before we give up
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

enum Listener {
    Tcp(TcpListener),
    Unix(UnixListener),
}

#[cfg(unix)]
fn try_socket_activation() -> Option<(Listener, Listener)> {
    let pid: u32 = std::process::id();
    let listen_pid: u32 = std::env::var("LISTEN_PID").ok()?.parse().ok()?;
    if listen_pid != pid {
        return None;
    }

    let nfds: usize = std::env::var("LISTEN_FDS").ok()?.parse().ok()?;
    if nfds < 2 {
        return None;
    }

    let fdnames = std::env::var("LISTEN_FDNAMES").unwrap_or_default();
    let names: Vec<&str> = fdnames.split(':').collect();

    let mut public: Option<Listener> = None;
    let mut hooks: Option<Listener> = None;

    for (i, name) in names.iter().enumerate() {
        if i >= nfds {
            break;
        }
        let fd = 3 + i as i32;
        match *name {
            "public" => {
                let std_l = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) };
                let _ = std_l.set_nonblocking(true);
                public = Some(Listener::Unix(
                    tokio::net::UnixListener::from_std(std_l).unwrap(),
                ));
            }
            "hooks" => {
                let std_l = unsafe { std::os::unix::net::UnixListener::from_raw_fd(fd) };
                let _ = std_l.set_nonblocking(true);
                hooks = Some(Listener::Unix(
                    tokio::net::UnixListener::from_std(std_l).unwrap(),
                ));
            }
            _ => {}
        }
    }

    // Prevent child processes from inheriting these env vars.
    unsafe {
        std::env::remove_var("LISTEN_FDS");
        std::env::remove_var("LISTEN_PID");
        std::env::remove_var("LISTEN_FDNAMES");
    }

    match (public, hooks) {
        (Some(p), Some(h)) => Some((p, h)),
        _ => None,
    }
}

async fn bind_listener(addr: &str) -> Listener {
    if addr.starts_with('/') {
        let path = std::path::Path::new(addr);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::remove_file(addr);
        Listener::Unix(UnixListener::bind(addr).expect("Couldn't bind to Unix socket!"))
    } else if addr.starts_with('@') {
        Listener::Unix(UnixListener::bind(addr).expect("Couldn't bind to abstract Unix socket!"))
    } else {
        Listener::Tcp(
            TcpListener::bind(addr)
                .await
                .expect("Couldn't bind to TCP port!"),
        )
    }
}

async fn serve_api(
    listener: Listener,
    app: axum::Router,
    mut shutdown_rx: watch::Receiver<bool>,
) -> std::io::Result<()> {
    match listener {
        Listener::Tcp(l) => {
            axum::serve(l, app.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(async move {
                    while shutdown_rx.changed().await.is_ok() && !*shutdown_rx.borrow() {}
                })
                .await
        }
        Listener::Unix(l) => {
            axum::serve(l, app.into_make_service())
                .with_graceful_shutdown(async move {
                    while shutdown_rx.changed().await.is_ok() && !*shutdown_rx.borrow() {}
                })
                .await
        }
    }
}

/// run a subsystem, restart it if it fails, shut it down upon shutdown signal
async fn run_subsystem<F, Fut>(
    name: &'static str,
    mut shutdown: watch::Receiver<bool>,
    mut start: F,
) where
    F: FnMut(watch::Receiver<bool>) -> Fut,
    Fut: Future<Output = Result<(), anyhow::Error>>,
{
    loop {
        if *shutdown.borrow() {
            break;
        }

        let result = start(shutdown.clone()).await;
        if *shutdown.borrow() {
            break;
        }

        warn!("{name} subsystem exited unexpectedly ({result:?}), restarting...");
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            changed = shutdown.changed() => {
                if changed.is_ok() && *shutdown.borrow() {
                    break;
                }
            }
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                format!("{}=debug,tower_http=debug", env!("CARGO_CRATE_NAME")).into()
            }),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    match dotenv() {
        Err(dotenv::Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => panic!("Error reading .env file: {err}"),
        _ => {}
    }

    // Load config
    let config_path = find_upward("vialo.toml")
        .expect("Could not find vialo.toml in current directory or any parent");

    let config_file = std::fs::read_to_string(&config_path).expect("Error reading vialo.toml");
    let config: Config = toml::from_str(&config_file)
        .map_err(|e| {
            let location = e.span().map_or(String::new(), |span| {
                let line = config_file[..span.start].matches('\n').count() + 1;
                format!(" around line {}", line)
            });
            format!("{} in vialo.toml{}", e.message(), location)
        })
        .expect("Error reading config");

    // Load encryption key
    let encryption_key: chacha20poly1305::Key = if let Ok(s) = std::env::var("ENCRYPTION_KEY") {
        let bytes: [u8; 32] = hex::decode(s.trim())
            .unwrap_or_else(|e| panic!("ENCRYPTION_KEY: invalid hex: {e}"))
            .try_into()
            .expect("ENCRYPTION_KEY: must be 32 bytes (64 hex chars)");
        chacha20poly1305::Key::from(bytes)
    } else if let Ok(key_path) = std::env::var("ENCRYPTION_KEY_PATH") {
        let bytes: [u8; 32] = hex::decode(
            std::fs::read_to_string(&key_path)
                .expect("Error reading encryption key file")
                .trim(),
        )
        .unwrap_or_else(|e| panic!("ENCRYPTION_KEY_PATH: invalid hex: {e}"))
        .try_into()
        .expect("ENCRYPTION_KEY_PATH: the file must be 32 bytes (64 hex chars)");
        chacha20poly1305::Key::from(bytes)
    } else if cfg!(debug_assertions) {
        warn!("¡Inseguro! Using default encryption key.");
        warn!("This better not be production!!!");
        ([104; 32]).into()
    } else {
        panic!("ENCRYPTION_KEY or ENCRYPTION_KEY_PATH must be set in prod.")
    };

    encryption::init(encryption_key);

    info!("Connecting to the database...");
    let db_timezone = config.timezone.clone();
    let pool = match PgPoolOptions::new()
        .max_connections(32)
        .min_connections(4)
        .after_connect(move |conn, _meta| {
            let db_timezone = db_timezone.clone();
            Box::pin(async move {
                sqlx::query!("SELECT set_config('TimeZone', $1, false)", db_timezone)
                    .fetch_one(&mut *conn)
                    .await?;
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

    let kratos_config = if let AuthConfig::Kratos {
        frontend_url,
        admin_url,
    } = &config.auth
    {
        let frontend_url = frontend_url.trim_end_matches('/').to_string();
        let admin_url = admin_url.trim_end_matches('/').to_string();
        Some(KratosConfigs {
            frontend: ory_kratos_client::apis::configuration::Configuration {
                base_path: frontend_url.clone(),
                ..Default::default()
            },
            admin: ory_kratos_client::apis::configuration::Configuration {
                base_path: admin_url.clone(),
                ..Default::default()
            },
        })
    } else {
        None
    };

    let (posts_tx, _) = broadcast::channel::<i32>(256);
    let (expired_appointments_tx, _) =
        broadcast::channel::<vialo_api::bookables::ExpiredAppointmentMessage>(256);
    let asp = Arc::new(AppState {
        db: pool.clone(),
        event_channels: EventChannels {
            bookables: BookableChannel::new("bookables".into(), pool.clone()),
            posts_tx,
            expired_appointments_tx,
        },
        config,
        kratos_config,
        rate_limiters: vialo_api::http::rate_limit::RateLimiters::new(),
    });

    // bootstrap (Not like web dev) admin account
    if let AuthConfig::Mock { uuid, email } = &asp.config.auth {
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

            sqlx::query!(
                "INSERT INTO accounts_people (id, full_name, email, label, membership_end) VALUES ($1, 'Admin', $2, 'auto-created during setup', 'infinity')",
                uuid,
                email
            ).execute(&mut *trans).await.unwrap();

            let group_id = sqlx::query_scalar!(
                "insert into account_groups (label,public) values ('AdminAG', true) RETURNING id",
            )
            .fetch_one(&mut *trans)
            .await
            .unwrap();

            sqlx::query!(
                "insert into account_group_memberships (group_id, account_id, role) VALUES ($1, $2, 'manager');", group_id, uuid
            )
            .execute(&mut *trans)
            .await
            .unwrap();

            sqlx::query!(
                "INSERT INTO account_group_app_roles (group_id, role) (select $1 as group_id, unnest(enum_range(null, null::app_role)) as role)", group_id
            ).execute(&mut *trans).await.unwrap();

            trans.commit().await.unwrap();

            info!("Created admin account with ID {}", uuid);
        }
    } else {
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
                info!("Waiting to receive webhook from Kratos...");
            } else {
                error!(
                    "Set the INITIAL_ADMIN_EMAIL environment variable for the application to set up your admin account."
                );
                std::process::exit(1);
            }
        }
    };

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let shutdown_task = tokio::spawn({
        let shutdown_tx = shutdown_tx.clone();
        async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};

                let mut terminate =
                    signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = terminate.recv() => {},
                }
            }

            #[cfg(not(unix))]
            {
                tokio::signal::ctrl_c()
                    .await
                    .expect("failed to install Ctrl+C handler (why are you running this on non-unix anyway?)");
            }

            let _ = shutdown_tx.send(true);
            info!("Shutting down...");
        }
    });

    // Task per subsystem
    let mut subsystems: Vec<(&'static str, tokio::task::JoinHandle<()>)> = Vec::new();

    #[cfg(feature = "ppsk")]
    subsystems.push(("ppsk", {
        let pool = pool.clone();
        let asp = asp.clone();
        tokio::spawn(run_subsystem(
            "ppsk",
            shutdown_rx.clone(),
            move |shutdown| vialo_api::ppsk::main(pool.clone(), asp.clone(), shutdown),
        ))
    }));

    // Printer is !Send so it gets special treatment
    #[cfg(feature = "printer")]
    subsystems.push(("printer", {
        let pool = pool.clone();
        let asp = asp.clone();
        let shutdown = shutdown_rx.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            let local = tokio::task::LocalSet::new();
            handle.block_on(
                local.run_until(run_subsystem("printer", shutdown, move |shutdown| {
                    vialo_api::printer::main(pool.clone(), asp.clone(), shutdown)
                })),
            );
        })
    }));

    #[cfg(feature = "email")]
    subsystems.push(("email", {
        let asp = asp.clone();
        tokio::spawn(run_subsystem(
            "email",
            shutdown_rx.clone(),
            move |shutdown| vialo_api::email::main(asp.clone(), shutdown),
        ))
    }));

    subsystems.push(("bookable_connectors", {
        let asp = asp.clone();
        tokio::spawn(run_subsystem(
            "bookable_connectors",
            shutdown_rx.clone(),
            move |shutdown| vialo_api::bookables::main(asp.clone(), shutdown),
        ))
    }));

    let (public_listener, hooks_listener) = {
        #[cfg(unix)]
        if let Some((p, h)) = try_socket_activation() {
            info!("Received listeners from systemd socket activation.");
            (p, h)
        } else {
            let p = bind_listener(&asp.config.public.listen).await;
            let h = bind_listener(&asp.config.hooks.listen).await;
            (p, h)
        }
        #[cfg(not(unix))]
        {
            let p = bind_listener(&asp.config.public.listen).await;
            let h = bind_listener(&asp.config.hooks.listen).await;
            (p, h)
        }
    };

    let app = vialo_api::http::create_router(asp.clone()).await;
    let hook_app = vialo_api::hooks::create_router(asp.clone()).await;
    let ws = vialo_api::ws::main(asp.clone());

    let cors = CorsLayer::new()
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
        .allow_headers([AUTHORIZATION, ACCEPT, CONTENT_TYPE]);

    let fatal = Arc::new(AtomicBool::new(false));

    // clean shutdown if server dies
    let serve = |name: &'static str, listener, app, shutdown_tx: watch::Sender<bool>| {
        let shutdown_rx = shutdown_rx.clone();
        let fatal = fatal.clone();
        tokio::spawn(async move {
            if let Err(err) = serve_api(listener, app, shutdown_rx).await {
                error!("{name} server failed: {err}");
                fatal.store(true, Ordering::Relaxed);
                let _ = shutdown_tx.send(true);
            }
        })
    };

    let public_api_task = serve(
        "public API",
        public_listener,
        app.merge(ws).layer(cors),
        shutdown_tx.clone(),
    );
    let hooks_api_task = serve("hooks API", hooks_listener, hook_app, shutdown_tx.clone());

    info!("Serving.");

    let mut shutdown_signalled = shutdown_rx.clone();
    while !*shutdown_signalled.borrow() {
        if shutdown_signalled.changed().await.is_err() {
            break;
        }
    }

    let drain = async move {
        for (name, task) in [
            ("public API", public_api_task),
            ("hooks API", hooks_api_task),
        ] {
            if let Err(err) = task.await {
                error!("{name} server task did not exit cleanly: {err}");
            }
        }
        for (name, task) in subsystems {
            if let Err(err) = task.await {
                error!("{name} subsystem task did not exit cleanly: {err}");
            }
        }
        if let Err(err) = shutdown_task.await {
            error!("signal handler task did not exit cleanly: {err}");
        }
    };

    if tokio::time::timeout(SHUTDOWN_TIMEOUT, drain).await.is_err() {
        error!("Killing tasks not exited within {SHUTDOWN_TIMEOUT:?}");
    }

    info!("Bye now!");

    // exit, a return would cause a panic in tokio
    std::process::exit(if fatal.load(Ordering::Relaxed) { 1 } else { 0 });
}
