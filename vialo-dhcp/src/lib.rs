use std::net::Ipv4Addr;
use std::time::Duration;

pub mod config;

use anyhow::{Context, Result};
use dora_core::async_trait;
use dora_core::dhcproto::v4::{self, DhcpOption, HType, Message, MessageType, Opcode, OptionCode};
use dora_core::handler::{Action, Plugin};
use dora_core::server::context::MsgContext;
use dora_core::tracing::{debug, info, warn};
use futures::TryStreamExt;
use rtnetlink::packet_route::link::{InfoData, InfoKind, InfoVlan, LinkAttribute, LinkInfo};
use serde::Deserialize;
use sqlx::PgPool;
use sqlx::types::ipnetwork::IpNetwork;
use uuid::Uuid;

/// How to derive a VLAN hint from the option 82 Agent Circuit ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
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

/// Realm network parameters required to hand out a lease.
#[derive(Debug, Clone, Copy)]
struct NetConfig {
    router: Ipv4Addr,
    dns: Option<Ipv4Addr>,
    subnet_mask: Ipv4Addr,
}

/// A registered device and its realm, as seen by `net_device_info`.
#[derive(Debug)]
struct Device {
    realm_id: Uuid,
    device_id: Uuid,
    /// `None` when the realm is missing router or subnet — not servable.
    net: Option<NetConfig>,
}

/// Result of trying to secure an IP for a device.
///
/// `net_ip_assignments` row states:
/// - free:        device_id NULL,     expires NULL or past
/// - leased:      device_id NOT NULL, expires in the future
/// - reclaimable: device_id NOT NULL, expires past (expired lease, sticky until reused)
/// - quarantined: device_id NULL,     expires in the future (post-DECLINE)
/// - static:      device_id NOT NULL, expires NULL (never reclaimed)
#[derive(Debug)]
enum ClaimOutcome {
    Granted(Ipv4Addr),
    /// Device already holds a different IP than the one it insisted on.
    Mismatch(Ipv4Addr),
    /// Nothing to hand out (pool full, or the requested IP can't be granted).
    Unavailable,
}

pub struct VialoDhcp {
    pool: PgPool,
    siaddr: Ipv4Addr,
    lease: Duration,
    renew: Duration,
    rebind: Duration,
    probation: Duration,
    circuit_id_mode: CircuitIdMode,
}

impl VialoDhcp {
    pub fn new(
        pool: PgPool,
        siaddr: Ipv4Addr,
        lease: Duration,
        probation: Duration,
        circuit_id_mode: CircuitIdMode,
    ) -> Self {
        Self {
            pool,
            siaddr,
            lease,
            renew: lease / 2,      // T1
            rebind: lease * 7 / 8, // T2
            probation,
            circuit_id_mode,
        }
    }

    /// Build the response skeleton (same as `util::new_msg` in dora's MsgType)
    fn build_response(&self, req: &Message) -> Message {
        let mut msg = Message::new_with_id(
            req.xid(),
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::UNSPECIFIED,
            self.siaddr,
            req.giaddr(),
            req.chaddr(),
        );
        msg.set_opcode(Opcode::BootReply)
            .set_htype(req.htype())
            .set_flags(req.flags())
            .set_hops(req.hops());
        msg.opts_mut()
            .insert(DhcpOption::ServerIdentifier(self.siaddr));
        msg
    }

    /// Extract a VLAN hint from option 82
    fn vlan_from_opts(&self, opts: &v4::DhcpOptions) -> Option<i32> {
        if self.circuit_id_mode == CircuitIdMode::Off {
            return None;
        }
        let Some(DhcpOption::RelayAgentInformation(relay)) =
            opts.get(OptionCode::RelayAgentInformation)
        else {
            return None;
        };
        let Some(v4::relay::RelayInfo::AgentCircuitId(raw)) =
            relay.get(v4::relay::RelayCode::AgentCircuitId)
        else {
            return None;
        };
        match self.circuit_id_mode {
            CircuitIdMode::Off => None,
            CircuitIdMode::Ascii => {
                let s = std::str::from_utf8(raw).ok()?;
                let digits: &str = s.split(|c: char| !c.is_ascii_digit()).next()?;
                digits.parse::<i32>().ok()
            }
            CircuitIdMode::BinaryU16 => {
                let bytes: [u8; 2] = raw.get(..2)?.try_into().ok()?;
                Some(i32::from(u16::from_be_bytes(bytes)))
            }
        }
    }
}

#[async_trait]
impl Plugin<Message> for VialoDhcp {
    async fn handle(&self, ctx: &mut MsgContext<Message>) -> Result<Action> {
        let req = ctx.msg();
        if req.opcode() != Opcode::BootRequest {
            debug!(opcode = ?req.opcode(), "ignoring non-BootRequest message");
            return Ok(Action::NoResponse);
        }
        if req.htype() != HType::Eth || req.chaddr().len() != 6 {
            debug!(htype = ?req.htype(), hlen = req.hlen(), "ignoring non-ethernet client");
            return Ok(Action::NoResponse);
        }

        let req_opts = req.opts();
        let msg_type = req_opts.msg_type();
        // Option 82 (relay) takes precedence; fall back to the interface VLAN
        // for packets arriving directly on a VLAN subinterface.
        let mut vlan_hint = self.vlan_from_opts(req_opts);
        if vlan_hint.is_none() {
            vlan_hint = vlan_of_ifindex(ctx.meta().ifindex).await;
        }

        debug!(
            msg_type = ?msg_type,
            chaddr = ?req.chaddr(),
            vlan_hint = ?vlan_hint,
            "dhcp request"
        );

        match msg_type {
            Some(MessageType::Discover) => {
                let mut resp = self.build_response(req);
                resp.opts_mut()
                    .insert(DhcpOption::MessageType(MessageType::Offer));
                ctx.set_resp_msg(resp);
                self.discover(ctx, vlan_hint).await
            }
            Some(MessageType::Request) => {
                // A REQUEST carrying another server's identifier means the client
                // selected a different server. We must stay silent
                if let Some(DhcpOption::ServerIdentifier(sid)) =
                    req_opts.get(OptionCode::ServerIdentifier)
                    && *sid != self.siaddr
                {
                    debug!(server_id = %sid, "request addressed to another server, dropping");
                    return Ok(Action::NoResponse);
                }
                let mut resp = self.build_response(req);
                if req.giaddr().is_unspecified() {
                    resp.set_flags(req.flags().set_broadcast());
                }
                resp.opts_mut()
                    .insert(DhcpOption::MessageType(MessageType::Ack));
                ctx.set_resp_msg(resp);
                self.request(ctx, vlan_hint).await
            }
            Some(MessageType::Release) => {
                self.release(ctx).await?;
                Ok(Action::NoResponse)
            }
            Some(MessageType::Decline) => self.decline(ctx).await,
            _ => {
                debug!("unsupported dhcp message type, dropping");
                Ok(Action::NoResponse)
            }
        }
    }
}

// -- DHCP message handlers --

impl VialoDhcp {
    async fn discover(
        &self,
        ctx: &mut MsgContext<Message>,
        vlan_hint: Option<i32>,
    ) -> Result<Action> {
        let mac = mac_to_macaddr(ctx.msg().chaddr())?;
        // Option 50 is only a preference here; the device's existing assignment wins.
        let requested = requested_ip(ctx.msg());

        let Some(device) = self.lookup_device(mac, vlan_hint).await? else {
            debug!(%mac, "unknown device, dropping discover");
            return Ok(Action::NoResponse);
        };
        let Some(net) = device.net else {
            warn!(realm_id = %device.realm_id, "realm has no router/subnet, cannot serve dhcp");
            return Ok(Action::NoResponse);
        };

        let ip = match self
            .claim_ip(device.realm_id, device.device_id, requested, false)
            .await?
        {
            ClaimOutcome::Granted(ip) | ClaimOutcome::Mismatch(ip) => ip,
            ClaimOutcome::Unavailable => {
                warn!(realm_id = %device.realm_id, "ip pool full");
                return Ok(Action::NoResponse);
            }
        };

        self.touch_last_seen(device.device_id).await;
        self.set_lease(ctx, ip, net);
        Ok(Action::Continue)
    }

    async fn request(
        &self,
        ctx: &mut MsgContext<Message>,
        vlan_hint: Option<i32>,
    ) -> Result<Action> {
        let mac = mac_to_macaddr(ctx.msg().chaddr())?;
        // SELECTING/INIT-REBOOT carry option 50; RENEWING/REBINDING carry ciaddr.
        let requested = requested_ip(ctx.msg()).or_else(|| {
            let ciaddr = ctx.msg().ciaddr();
            (!ciaddr.is_unspecified()).then_some(ciaddr)
        });
        let Some(requested) = requested else {
            debug!(%mac, "request without option 50 or ciaddr, dropping");
            return Ok(Action::NoResponse);
        };

        let Some(device) = self.lookup_device(mac, vlan_hint).await? else {
            // Not NAK: don't fight other DHCP servers over clients we don't know.
            info!(%mac, "unknown device sent a request, dropping");
            return Ok(Action::NoResponse);
        };
        let Some(net) = device.net else {
            warn!(realm_id = %device.realm_id, "realm has no router/subnet, cannot serve dhcp");
            return Ok(Action::NoResponse);
        };

        let ip = match self
            .claim_ip(device.realm_id, device.device_id, Some(requested), true)
            .await?
        {
            ClaimOutcome::Granted(ip) => ip,
            ClaimOutcome::Mismatch(assigned) => {
                info!(%mac, %requested, %assigned, "requested ip doesn't match assignment, nak");
                ctx.update_resp_msg(MessageType::Nak);
                return Ok(Action::Respond);
            }
            ClaimOutcome::Unavailable => {
                info!(%mac, %requested, "requested ip not grantable, nak");
                ctx.update_resp_msg(MessageType::Nak);
                return Ok(Action::Respond);
            }
        };

        self.touch_last_seen(device.device_id).await;
        self.set_lease(ctx, ip, net);
        Ok(Action::Continue)
    }

    async fn release(&self, ctx: &mut MsgContext<Message>) -> Result<()> {
        let mac = mac_to_macaddr(ctx.msg().chaddr())?;
        let Some(device_id) = self.device_id_by_mac(mac).await? else {
            return Ok(());
        };
        if let Some(ip) = self.end_lease(device_id).await? {
            debug!(?ip, %mac, "released lease");
        }
        Ok(())
    }

    async fn decline(&self, ctx: &mut MsgContext<Message>) -> Result<Action> {
        let Some(DhcpOption::RequestedIpAddress(declined_ip)) =
            ctx.msg().opts().get(OptionCode::RequestedIpAddress)
        else {
            debug!("decline without option 50, dropping");
            return Ok(Action::NoResponse);
        };
        let declined_ip = *declined_ip;
        let mac = mac_to_macaddr(ctx.msg().chaddr())?;

        let Some(device_id) = self.device_id_by_mac(mac).await? else {
            return Ok(Action::NoResponse);
        };
        if self.quarantine_ip(device_id, declined_ip).await? {
            info!(%declined_ip, %mac, "declined ip quarantined");
        } else {
            debug!(%declined_ip, %mac, "decline for ip not assigned to sender, ignoring");
        }
        Ok(Action::NoResponse)
    }
}

// -- Database helpers --

impl VialoDhcp {
    /// Look up a registered device by MAC + optional VLAN hint.
    async fn lookup_device(
        &self,
        mac: mac_address::MacAddress,
        vlan_hint: Option<i32>,
    ) -> Result<Option<Device>> {
        let Some(row) = sqlx::query!(
            r#"
            SELECT ndi.realm_id,
                   ndi.id AS device_id,
                   nr.ipv4_router AS "ipv4_router: IpNetwork",
                   nr.ipv4_dns AS "ipv4_dns: IpNetwork",
                   nr.ipv4_subnet AS "ipv4_subnet: IpNetwork"
            FROM net_device_info ndi
            JOIN net_realms nr ON nr.id = ndi.realm_id
            WHERE ndi.mac = $1
              AND ($2::int IS NULL OR nr.vlan = $2)
            LIMIT 1
            "#,
            mac as mac_address::MacAddress,
            vlan_hint,
        )
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let router = row.ipv4_router.and_then(host_v4);
        let dns = row.ipv4_dns.and_then(host_v4);
        let subnet_mask = match row.ipv4_subnet.map(|s| s.mask()) {
            Some(std::net::IpAddr::V4(m)) => Some(m),
            _ => None,
        };

        Ok(Some(Device {
            realm_id: row.realm_id.context("device has no realm assignment")?,
            device_id: row.device_id.context("device has no id")?,
            net: router
                .zip(subnet_mask)
                .map(|(router, subnet_mask)| NetConfig {
                    router,
                    dns,
                    subnet_mask,
                }),
        }))
    }

    async fn device_id_by_mac(&self, mac: mac_address::MacAddress) -> Result<Option<Uuid>> {
        Ok(sqlx::query_scalar!(
            "SELECT id FROM net_devices WHERE mac = $1",
            mac as mac_address::MacAddress,
        )
        .fetch_optional(&self.pool)
        .await?)
    }

    /// End a device's lease while keeping the binding (sticky): the row stays
    /// attributed to the device but becomes reclaimable. Returns the freed IP.
    async fn end_lease(&self, device_id: Uuid) -> Result<Option<Ipv4Addr>> {
        let released = sqlx::query_scalar!(
            r#"
            UPDATE net_ip_assignments
            SET expires = NOW()
            WHERE device_id = $1
            RETURNING ipv4_addr AS "ipv4_addr: IpNetwork"
            "#,
            device_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(released.and_then(host_v4))
    }

    /// Quarantine an IP the client declined (found in use elsewhere): unassign it
    /// but keep it out of the pool until probation passes. Only acts when the IP
    /// is actually assigned to `device_id`, so a spoofed decline can't strip other
    /// devices' leases. Returns whether a row was quarantined.
    async fn quarantine_ip(&self, device_id: Uuid, ip: Ipv4Addr) -> Result<bool> {
        let res = sqlx::query!(
            "UPDATE net_ip_assignments
             SET (device_id, expires) = (NULL, NOW() + make_interval(secs => $3))
             WHERE host(ipv4_addr) = host($1) AND device_id = $2",
            IpNetwork::from(std::net::IpAddr::V4(ip)),
            device_id,
            self.probation.as_secs_f64(),
        )
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Secure an IP for the device in its realm, in one transaction:
    /// reuse + extend its existing assignment, else claim `requested` if grantable,
    /// else (unless `enforce_requested`) allocate any free/reclaimable address.
    ///
    /// With `enforce_requested` (REQUEST semantics) the device's existing
    /// assignment must match `requested` — otherwise `Mismatch` — and no
    /// fallback allocation happens (`Unavailable` → caller NAKs).
    async fn claim_ip(
        &self,
        realm_id: Uuid,
        device_id: Uuid,
        requested: Option<Ipv4Addr>,
        enforce_requested: bool,
    ) -> Result<ClaimOutcome> {
        let lease_secs = self.lease.as_secs_f64();
        let mut tx = self.pool.begin().await?;

        // If the device moved realms, its old assignment lingers (nothing else
        // clears it) and would trip the device_id UNIQUE constraint below.
        sqlx::query!(
            "UPDATE net_ip_assignments SET (device_id, expires) = (NULL, NULL)
             WHERE device_id = $1 AND realm_id != $2",
            device_id,
            realm_id,
        )
        .execute(&mut *tx)
        .await?;

        // Existing assignment (any expiry — bindings are sticky): extend it.
        let existing = sqlx::query_scalar!(
            r#"
            SELECT ipv4_addr AS "ipv4_addr: IpNetwork"
            FROM net_ip_assignments
            WHERE realm_id = $1 AND device_id = $2
            FOR UPDATE
            "#,
            realm_id,
            device_id,
        )
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(current) = existing.and_then(host_v4) {
            if enforce_requested && requested != Some(current) {
                tx.commit().await?;
                return Ok(ClaimOutcome::Mismatch(current));
            }
            sqlx::query!(
                "UPDATE net_ip_assignments SET expires = NOW() + make_interval(secs => $3)
                 WHERE realm_id = $1 AND device_id = $2",
                realm_id,
                device_id,
                lease_secs,
            )
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(ClaimOutcome::Granted(current));
        }

        // Try the address the client asked for, if it's free or reclaimable.
        if let Some(ip) = requested {
            let claimed = sqlx::query!(
                r#"
                UPDATE net_ip_assignments
                SET (device_id, expires) = ($1, NOW() + make_interval(secs => $4))
                WHERE host(ipv4_addr) = host($2) AND realm_id = $3
                  AND ((device_id IS NULL AND (expires IS NULL OR expires <= NOW()))
                       OR (device_id IS NOT NULL AND expires <= NOW()))
                "#,
                device_id,
                IpNetwork::from(std::net::IpAddr::V4(ip)),
                realm_id,
                lease_secs,
            )
            .execute(&mut *tx)
            .await?;
            if claimed.rows_affected() > 0 {
                tx.commit().await?;
                return Ok(ClaimOutcome::Granted(ip));
            }
            if enforce_requested {
                tx.commit().await?;
                return Ok(ClaimOutcome::Unavailable);
            }
        }

        // Allocate: prefer untouched addresses over reclaiming expired leases.
        // SKIP LOCKED keeps concurrent discovers from being offered the same row.
        let candidate = sqlx::query_scalar!(
            r#"
            SELECT ipv4_addr AS "ipv4_addr: IpNetwork"
            FROM net_ip_assignments
            WHERE realm_id = $1
              AND ((device_id IS NULL AND (expires IS NULL OR expires <= NOW()))
                   OR (device_id IS NOT NULL AND expires <= NOW()))
            ORDER BY (device_id IS NULL) DESC, ipv4_addr
            LIMIT 1
            FOR UPDATE SKIP LOCKED
            "#,
            realm_id,
        )
        .fetch_optional(&mut *tx)
        .await?;

        let Some(ip) = candidate.and_then(host_v4) else {
            tx.commit().await?;
            return Ok(ClaimOutcome::Unavailable);
        };

        sqlx::query!(
            "UPDATE net_ip_assignments SET (device_id, expires) = ($1, NOW() + make_interval(secs => $3))
             WHERE host(ipv4_addr) = host($2)",
            device_id,
            IpNetwork::from(std::net::IpAddr::V4(ip)),
            lease_secs,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(ClaimOutcome::Granted(ip))
    }

    /// Best-effort `last_seen` bump — an ACK shouldn't fail on it.
    async fn touch_last_seen(&self, device_id: Uuid) {
        if let Err(err) = sqlx::query!(
            "UPDATE net_devices SET last_seen = NOW() WHERE id = $1",
            device_id
        )
        .execute(&self.pool)
        .await
        {
            warn!(?err, %device_id, "failed to update last_seen");
        }
    }

    /// Populate the DHCP response with the assigned IP and options.
    fn set_lease(&self, ctx: &mut MsgContext<Message>, ip: Ipv4Addr, net: NetConfig) {
        if let Some(resp) = ctx.resp_msg_mut() {
            resp.set_yiaddr(ip);
            let opts = resp.opts_mut();
            opts.insert(DhcpOption::SubnetMask(net.subnet_mask));
            opts.insert(DhcpOption::Router(vec![net.router]));
            if let Some(dns) = net.dns {
                opts.insert(DhcpOption::DomainNameServer(vec![dns]));
            }
        }

        // Copies opt 82 / client id from the request and inserts lease/T1/T2.
        ctx.populate_opts_lease(
            &v4::DhcpOptions::default(),
            self.lease,
            self.renew,
            self.rebind,
        );
    }
}

/// The host address of an `inet`/`cidr` value as an `Ipv4Addr`. Decoding these
/// columns straight to `IpAddr` is lossy (and errors) whenever the stored value
/// carries a prefix — which it does: `expand_cidr` keeps the subnet's masklen on
/// pool addresses (e.g. `10.0.0.3/24`), so we decode as `IpNetwork` and take `.ip()`.
fn host_v4(net: IpNetwork) -> Option<Ipv4Addr> {
    match net.ip() {
        std::net::IpAddr::V4(ip) => Some(ip),
        _ => None,
    }
}

/// Option 50 (Requested IP Address), if present.
fn requested_ip(msg: &Message) -> Option<Ipv4Addr> {
    match msg.opts().get(OptionCode::RequestedIpAddress) {
        Some(DhcpOption::RequestedIpAddress(ip)) => Some(*ip),
        _ => None,
    }
}

#[cfg(test)]
mod tests;

/// Convert DHCP chaddr bytes to a Postgres-compatible MacAddress.
/// `handle` only admits ethernet messages, so `mac` is always 6 bytes.
fn mac_to_macaddr(mac: &[u8]) -> Result<mac_address::MacAddress> {
    Ok(mac_address::MacAddress::new(
        mac.try_into().context("chaddr is not 6 bytes")?,
    ))
}

/// Ask the kernel via netlink for the VLAN ID of a VLAN-type interface.
/// Returns `None` for non-VLAN interfaces, netlink failures, etc. The
/// caller treats `None` as "no VLAN hint" and falls back to unfiltered lookup.
async fn vlan_of_ifindex(ifindex: u32) -> Option<i32> {
    let (conn, handle, _) = rtnetlink::new_connection().ok()?;
    dora_core::tokio::spawn(conn);

    let mut stream = handle.link().get().match_index(ifindex).execute();
    let msg = stream.try_next().await.ok()??;

    for attr in &msg.attributes {
        let LinkAttribute::LinkInfo(infos) = attr else {
            continue;
        };
        let is_vlan = infos
            .iter()
            .any(|info| matches!(info, LinkInfo::Kind(InfoKind::Vlan)));
        if !is_vlan {
            return None;
        }
        for info in infos {
            if let LinkInfo::Data(InfoData::Vlan(nlas)) = info {
                for nla in nlas {
                    if let InfoVlan::Id(id) = nla {
                        return Some(i32::from(*id));
                    }
                }
            }
        }
    }
    None
}
