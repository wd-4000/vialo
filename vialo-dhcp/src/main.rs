use std::path::PathBuf;

use anyhow::{Context, Result};
use dora_core::Server;
use dora_core::dhcproto::v4;
use dora_core::pnet::datalink;
use dora_core::tokio;
use dora_core::tracing::info;
use sqlx::postgres::PgPoolOptions;
use vialo_dhcp::{VialoDhcp, config::Config};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // The `[dhcp]` section of vialo.toml, shared with the rest of the stack.
    // The database URL stays in the environment: it's a secret, and vialo-api
    // reads it the same way.
    let cfg = vialo_common::load::<Config>()?.dhcp;
    let database_url = std::env::var("DATABASE_URL").context("DATABASE_URL must be set")?;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async move {
        let pool = PgPoolOptions::new()
            .max_connections(cfg.pg_max_connections)
            // Attribute every connection's writes (e.g. net_devices.last_seen) to the
            // `dhcp` subsystem so the audit trigger (migration 20) permits them.
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::query!("SELECT set_config('app.subsystem', 'dhcp', false)")
                        .fetch_all(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&database_url)
            .await
            .expect("failed to connect to postgres");

        // Enumerate interfaces
        let ifaces: Vec<_> = if cfg.interfaces.is_empty() {
            datalink::interfaces()
        } else {
            datalink::interfaces()
                .into_iter()
                .filter(|i| cfg.interfaces.contains(&i.name))
                .collect()
        };

        if ifaces.is_empty() {
            anyhow::bail!("no matching network interfaces found");
        }
        info!(
            interfaces = ?ifaces.iter().map(|i| &i.name).collect::<Vec<_>>(),
            "bound to interfaces"
        );

        // Build dora's CLI config with our settings. `config_path`, `v6_addr` and
        // `external_api` are inert here: we run an embedded v4-only `Server` and never
        // start dora's file-config loader, v6 listener, or external API (only dora's
        // `bin` crate wires those up).
        let dora_cfg = dora_core::config::cli::Config {
            config_path: PathBuf::from("/dev/null"),
            v4_addr: cfg.listen,
            v6_addr: "[::]:547".parse().unwrap(),
            external_api: "[::]:3333".parse().unwrap(),
            timeout: 3,
            max_live_msgs: 1000,
            channel_size: 10000,
            threads: None,
            thread_name: "vialo-dhcp".into(),
            dora_id: "vialo-dhcp".into(),
            dora_log: "info".into(),
            database_url: String::new(),
        };

        let plugin = VialoDhcp::new(
            pool,
            cfg.siaddr,
            cfg.lease(),
            cfg.probation(),
            cfg.circuit_id_vlan,
        );

        let mut server = Server::<v4::Message>::new(dora_cfg, ifaces)?;
        server.plugin(plugin);

        info!(listen = %cfg.listen, siaddr = %cfg.siaddr, "starting DHCP server");

        server
            .start(async {
                tokio::signal::ctrl_c().await?;
                Ok::<_, anyhow::Error>(())
            })
            .await?;

        Ok(())
    })?;

    info!("shutdown complete");
    Ok(())
}
