use super::*;
use mac_address::MacAddress;

fn dhcp(pool: PgPool) -> VialoDhcp {
    VialoDhcp::new(
        pool,
        Ipv4Addr::new(10, 200, 0, 1),
        Duration::from_secs(3600),
        Duration::from_secs(600),
        CircuitIdMode::Off,
    )
}

fn mac(last: u8) -> MacAddress {
    MacAddress::new([2, 0, 0, 0, 0, last])
}

fn net(ip: &str) -> IpNetwork {
    ip.parse().unwrap()
}

/// A host address carried at `subnet`'s prefix length, matching how realms
/// store their router/dns (e.g. `10.0.0.1/24` in a /24) — `expand_cidr`
/// excludes them from the pool by exact `inet` equality, which is masklen-aware.
fn host_in(subnet: IpNetwork, addr: &str) -> IpNetwork {
    IpNetwork::new(addr.parse().unwrap(), subnet.prefix()).unwrap()
}

/// A pooled connection whose writes are attributed to the `dhcp` subsystem, as
/// the audit trigger (migration 20) requires for changes to audited tables.
/// (`net_ip_assignments` is exempt, so `claim_ip` needs no attribution.)
async fn attributed(pool: &PgPool) -> sqlx::pool::PoolConnection<sqlx::Postgres> {
    let mut c = pool.acquire().await.unwrap();
    sqlx::query!("SELECT set_config('app.subsystem', 'dhcp', false)")
        .fetch_all(&mut *c)
        .await
        .unwrap();
    c
}

/// Insert a realm; the `sync_realm_ip` trigger fills its IP pool.
async fn seed_realm(pool: &PgPool, subnet: &str, router: &str, dns: &str, vlan: i32) -> Uuid {
    let mut c = attributed(pool).await;
    let subnet = net(subnet);
    sqlx::query_scalar!(
        "INSERT INTO net_realms (ipv4_subnet, ipv4_router, ipv4_dns, vlan)
            VALUES ($1, $2, $3, $4) RETURNING id",
        subnet,
        host_in(subnet, router),
        host_in(subnet, dns),
        vlan,
    )
    .fetch_one(&mut *c)
    .await
    .unwrap()
}

/// Realm with no router — devices in it are looked up but not servable.
async fn seed_realm_no_router(pool: &PgPool, subnet: &str, vlan: i32) -> Uuid {
    let mut c = attributed(pool).await;
    sqlx::query_scalar!(
        "INSERT INTO net_realms (ipv4_subnet, ipv4_router, ipv4_dns, vlan)
            VALUES ($1, NULL, NULL, $2) RETURNING id",
        net(subnet),
        vlan,
    )
    .fetch_one(&mut *c)
    .await
    .unwrap()
}

/// Insert a registered device (account → network → cred → device) with its
/// realm set directly, so `net_device_info` resolves a realm for it.
async fn seed_device(pool: &PgPool, m: MacAddress, realm_id: Uuid) -> Uuid {
    let mut c = attributed(pool).await;
    let account = sqlx::query_scalar!("INSERT INTO accounts (full_name) VALUES ('t') RETURNING id")
        .fetch_one(&mut *c)
        .await
        .unwrap();
    let network = sqlx::query_scalar!(
        "INSERT INTO net_networks (label, wired, multi_device, auto_add_on_auth, auto_add_via_dhcp)
            VALUES ($1, false, false, false, false) RETURNING id",
        format!("net-{m}"),
    )
    .fetch_one(&mut *c)
    .await
    .unwrap();
    let cred = sqlx::query_scalar!(
        "INSERT INTO net_cred (account_id, network_id) VALUES ($1, $2) RETURNING id",
        account,
        network,
    )
    .fetch_one(&mut *c)
    .await
    .unwrap();
    sqlx::query_scalar!(
        "INSERT INTO net_devices (cred_id, mac, realm_id) VALUES ($1, $2, $3) RETURNING id",
        cred,
        m as MacAddress,
        realm_id,
    )
    .fetch_one(&mut *c)
    .await
    .unwrap()
}

async fn move_device(pool: &PgPool, device_id: Uuid, realm_id: Uuid) {
    let mut c = attributed(pool).await;
    sqlx::query!(
        "UPDATE net_devices SET realm_id = $1 WHERE id = $2",
        realm_id,
        device_id,
    )
    .execute(&mut *c)
    .await
    .unwrap();
}

/// `device_id` currently attributed to an IP (NULL if free/quarantined).
async fn row_device(pool: &PgPool, ip: &str) -> Option<Uuid> {
    sqlx::query_scalar!(
        "SELECT device_id FROM net_ip_assignments WHERE host(ipv4_addr) = host($1)",
        net(ip),
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

fn granted(o: ClaimOutcome) -> Ipv4Addr {
    match o {
        ClaimOutcome::Granted(ip) => ip,
        ClaimOutcome::Mismatch(ip) => panic!("expected Granted, got Mismatch({ip})"),
        ClaimOutcome::Unavailable => panic!("expected Granted, got Unavailable"),
    }
}

#[test]
fn circuit_id_mode_parses() {
    assert_eq!("off".parse::<CircuitIdMode>().unwrap(), CircuitIdMode::Off);
    assert_eq!(
        "ascii".parse::<CircuitIdMode>().unwrap(),
        CircuitIdMode::Ascii
    );
    assert_eq!(
        "binary".parse::<CircuitIdMode>().unwrap(),
        CircuitIdMode::BinaryU16
    );
    assert!("nonsense".parse::<CircuitIdMode>().is_err());
}

// A device holding a lease must get the *same* IP back on re-discover, not a
// second allocation that trips the device_id UNIQUE constraint (lockout bug).
#[sqlx::test(migrations = "../vialo-api/migrations")]
async fn discover_is_sticky(pool: PgPool) {
    let d = dhcp(pool.clone());
    let realm = seed_realm(&pool, "10.200.0.0/29", "10.200.0.1", "10.200.0.2", 200).await;
    let dev = seed_device(&pool, mac(1), realm).await;

    let first = granted(d.claim_ip(realm, dev, None, false).await.unwrap());
    let second = granted(d.claim_ip(realm, dev, None, false).await.unwrap());
    assert_eq!(first, Ipv4Addr::new(10, 200, 0, 3));
    assert_eq!(first, second);
}

// With a single free address, two different devices cannot both be granted it.
#[sqlx::test(migrations = "../vialo-api/migrations")]
async fn concurrent_claims_never_share_an_ip(pool: PgPool) {
    let d = dhcp(pool.clone());
    // /30 with router == dns == .1 leaves exactly one usable address: .2
    let realm = seed_realm(&pool, "10.200.0.0/30", "10.200.0.1", "10.200.0.1", 200).await;
    let d1 = seed_device(&pool, mac(1), realm).await;
    let d2 = seed_device(&pool, mac(2), realm).await;

    let (a, b) = dora_core::tokio::join!(
        d.claim_ip(realm, d1, None, false),
        d.claim_ip(realm, d2, None, false),
    );
    let outcomes = [a.unwrap(), b.unwrap()];
    let grants = outcomes
        .iter()
        .filter(|o| matches!(o, ClaimOutcome::Granted(_)))
        .count();
    let unavail = outcomes
        .iter()
        .filter(|o| matches!(o, ClaimOutcome::Unavailable))
        .count();
    assert_eq!(grants, 1, "exactly one device may win the last IP");
    assert_eq!(unavail, 1);
}

// Moving a device to a new realm must not leave its old assignment dangling
// (which would trip device_id UNIQUE on the new grant).
#[sqlx::test(migrations = "../vialo-api/migrations")]
async fn realm_move_clears_stale_assignment(pool: PgPool) {
    let d = dhcp(pool.clone());
    let realm_a = seed_realm(&pool, "10.210.0.0/29", "10.210.0.1", "10.210.0.2", 210).await;
    let realm_b = seed_realm(&pool, "10.211.0.0/29", "10.211.0.1", "10.211.0.2", 211).await;
    let dev = seed_device(&pool, mac(1), realm_a).await;

    let ip_a = granted(d.claim_ip(realm_a, dev, None, false).await.unwrap());
    assert_eq!(ip_a, Ipv4Addr::new(10, 210, 0, 3));

    move_device(&pool, dev, realm_b).await;
    let ip_b = granted(d.claim_ip(realm_b, dev, None, false).await.unwrap());
    assert_eq!(ip_b, Ipv4Addr::new(10, 211, 0, 3));
    assert_eq!(
        row_device(&pool, "10.210.0.3").await,
        None,
        "old realm row freed"
    );
}

// REQUEST for an IP that isn't the device's assignment must NAK (Mismatch),
// and REQUEST for the matching IP must be granted.
#[sqlx::test(migrations = "../vialo-api/migrations")]
async fn request_enforces_assigned_ip(pool: PgPool) {
    let d = dhcp(pool.clone());
    let realm = seed_realm(&pool, "10.200.0.0/29", "10.200.0.1", "10.200.0.2", 200).await;
    let dev = seed_device(&pool, mac(1), realm).await;

    let ip = granted(d.claim_ip(realm, dev, None, false).await.unwrap());
    let wrong = Ipv4Addr::new(10, 200, 0, 9);
    match d.claim_ip(realm, dev, Some(wrong), true).await.unwrap() {
        ClaimOutcome::Mismatch(assigned) => assert_eq!(assigned, ip),
        other => panic!("expected Mismatch, got {other:?}"),
    }
    assert_eq!(
        granted(d.claim_ip(realm, dev, Some(ip), true).await.unwrap()),
        ip
    );
}

// RELEASE keeps the device↔IP binding so the address stays sticky.
#[sqlx::test(migrations = "../vialo-api/migrations")]
async fn release_keeps_binding(pool: PgPool) {
    let d = dhcp(pool.clone());
    let realm = seed_realm(&pool, "10.200.0.0/29", "10.200.0.1", "10.200.0.2", 200).await;
    let dev = seed_device(&pool, mac(1), realm).await;

    let ip = granted(d.claim_ip(realm, dev, None, false).await.unwrap());
    assert_eq!(d.end_lease(dev).await.unwrap(), Some(ip));
    // Binding preserved: the row is still attributed to the device...
    assert_eq!(row_device(&pool, "10.200.0.3").await, Some(dev));
    // ...and a re-discover hands the same address back.
    assert_eq!(
        granted(d.claim_ip(realm, dev, None, false).await.unwrap()),
        ip
    );
}

// A declined IP is quarantined (not reallocated) and only the holder can
// quarantine it.
#[sqlx::test(migrations = "../vialo-api/migrations")]
async fn decline_quarantines_ip(pool: PgPool) {
    let d = dhcp(pool.clone());
    // Single usable IP: .2
    let realm = seed_realm(&pool, "10.200.0.0/30", "10.200.0.1", "10.200.0.1", 200).await;
    let d1 = seed_device(&pool, mac(1), realm).await;
    let d2 = seed_device(&pool, mac(2), realm).await;

    let ip = granted(d.claim_ip(realm, d1, None, false).await.unwrap());
    assert_eq!(ip, Ipv4Addr::new(10, 200, 0, 2));
    assert!(d.quarantine_ip(d1, ip).await.unwrap());
    // Spoof guard: a device that doesn't hold the IP can't quarantine it.
    assert!(!d.quarantine_ip(d2, ip).await.unwrap());
    // The only address is quarantined, so another device gets nothing.
    assert!(matches!(
        d.claim_ip(realm, d2, None, false).await.unwrap(),
        ClaimOutcome::Unavailable
    ));
}

#[sqlx::test(migrations = "../vialo-api/migrations")]
async fn lookup_unknown_device_is_none(pool: PgPool) {
    let d = dhcp(pool.clone());
    assert!(d.lookup_device(mac(9), None).await.unwrap().is_none());
}

// A realm missing its router yields a device with no servable NetConfig.
#[sqlx::test(migrations = "../vialo-api/migrations")]
async fn lookup_realm_without_router_has_no_netconfig(pool: PgPool) {
    let d = dhcp(pool.clone());
    let realm = seed_realm_no_router(&pool, "10.202.0.0/29", 202).await;
    seed_device(&pool, mac(1), realm).await;

    let device = d.lookup_device(mac(1), None).await.unwrap().unwrap();
    assert!(device.net.is_none());
}
