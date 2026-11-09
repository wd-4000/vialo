//! This config bit lives here because vialo-dhcp won't build on Mac.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use schemars::JsonSchema;
use serde::Deserialize;

/// How to derive a VLAN hint from the option 82 Agent Circuit ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CircuitIdMode {
    /// Ignore the circuit ID.
    Off,
    /// ASCII "vlan.module.port" (Cisco-style), leading digit is the VLAN.
    #[default]
    Ascii,
    /// First two bytes as a big-endian u16.
    #[serde(rename = "binary")]
    BinaryU16,
}

/// The slice of `vialo.toml` vialo-dhcp reads. Everything else in the file
/// belongs to other processes and is ignored.
#[derive(Deserialize, JsonSchema)]
pub struct Config {
    pub dhcp: DhcpConfig,
}

fn default_listen() -> SocketAddr {
    "0.0.0.0:67".parse().unwrap()
}

fn default_lease_time() -> u64 {
    10800
}

fn default_probation() -> u64 {
    3600
}

fn default_pg_max_connections() -> u32 {
    5
}

/// `[dhcp]`. Unknown keys are rejected: a typo'd key would otherwise silently
/// leave the default in place, which for `lease_time` or `siaddr` is the kind
/// of thing you find out about from users, not logs.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DhcpConfig {
    /// Server identifier IP. Must be an IP on the listen interface. Realms with
    /// their own DHCP server address use that instead.
    pub siaddr: Ipv4Addr,

    /// DHCP server listen address (UDP).
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,

    /// Interface names to bind. Empty binds every interface; pin a single one
    /// in production, which is what enables SO_BINDTODEVICE.
    #[serde(default)]
    pub interfaces: Vec<String>,

    /// Lease duration in seconds. T1 = lease/2, T2 = lease × 7/8.
    #[serde(default = "default_lease_time")]
    pub lease_time: u64,

    /// How long a declined address stays quarantined, in seconds.
    #[serde(default = "default_probation")]
    pub probation: u64,

    /// How to read a VLAN from the option 82 Agent Circuit ID.
    #[serde(default)]
    pub circuit_id_vlan: CircuitIdMode,

    /// Maximum size of the Postgres connection pool.
    #[serde(default = "default_pg_max_connections")]
    pub pg_max_connections: u32,
}

impl DhcpConfig {
    pub fn lease(&self) -> Duration {
        Duration::from_secs(self.lease_time)
    }

    pub fn probation(&self) -> Duration {
        Duration::from_secs(self.probation)
    }
}
