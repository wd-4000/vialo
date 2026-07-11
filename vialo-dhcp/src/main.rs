use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use dora_core::Server;
use dora_core::dhcproto::v4;
use dora_core::pnet::datalink;
use dora_core::tokio;
use dora_core::tracing::info;
use sqlx::postgres::PgPoolOptions;
use vialo_dhcp::{CircuitIdMode, VialoDhcp};

#[derive(Parser)]
#[command(name = "vialo-dhcp", about = "Vialo DHCP server")]
struct Cli {
    /// Postgres database URL
    #[arg(long, env)]
    database_url: String,

    /// DHCP server listen address (default 0.0.0.0:67)
    #[arg(long, env, default_value = "0.0.0.0:67")]
    listen: SocketAddr,

    /// Server identifier IP — must be an IP on the listen interface
    #[arg(long, env)]
    siaddr: Ipv4Addr,

    /// Comma-separated interface names to bind. Empty = bind all interfaces.
    #[arg(long, env, default_value = "")]
    interfaces: String,

    /// Lease duration in seconds (T1/renew and T2/rebind are derived from it).
    #[arg(long, env, default_value_t = 10800)]
    lease_time: u64,

    /// How long a declined address stays quarantined, in seconds.
    #[arg(long, env, default_value_t = 3600)]
    probation: u64,

    /// How to read a VLAN from the option 82 Agent Circuit ID: off, ascii, or binary.
    #[arg(long, env, default_value = "ascii")]
    circuit_id_vlan: CircuitIdMode,

    /// Maximum size of the Postgres connection pool.
    #[arg(long, env, default_value_t = 5)]
    pg_max_connections: u32,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async move {
        let pool = PgPoolOptions::new()
            .max_connections(cli.pg_max_connections)
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
            .connect(&cli.database_url)
            .await
            .expect("failed to connect to postgres");

        // Enumerate interfaces
        let ifaces: Vec<_> = if cli.interfaces.is_empty() {
            datalink::interfaces()
        } else {
            let names: Vec<&str> = cli.interfaces.split(',').map(|s| s.trim()).collect();
            datalink::interfaces()
                .into_iter()
                .filter(|i| names.contains(&i.name.as_str()))
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
            v4_addr: cli.listen,
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
            cli.siaddr,
            Duration::from_secs(cli.lease_time),
            Duration::from_secs(cli.probation),
            cli.circuit_id_vlan,
        );

        let mut server = Server::<v4::Message>::new(dora_cfg, ifaces)?;
        server.plugin(plugin);

        info!(listen = %cli.listen, siaddr = %cli.siaddr, "starting DHCP server");

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
