//! `vialo seed`: seeds rooms, realms and residents

use std::{collections::HashSet, net::Ipv4Addr};

use chrono::{Months, TimeDelta, Utc};
use ipnetwork::{IpNetwork, Ipv4Network};
use rand::{RngExt, prelude::IndexedRandom};
use vialo_api::{
    config::Config,
    helpers::PgDate,
    http::{
        network::schemas::PostOrPutRealmSchema,
        people::schemas::{ContractInfoSchema, CreateRoomSchema, CreateUserSchema},
    },
    import::{Importer, Ref, Report, people::PersonImport},
};

mod first_names;
mod last_names;
use first_names::FIRST_NAMES;
use last_names::LAST_NAMES;

/// Fills rooms 0101..1414 with random people and NAT
pub async fn seed(database_uri: &str) -> anyhow::Result<Report> {
    let config: Config = vialo_common::load()?;
    let pool = sqlx::PgPool::connect(database_uri).await?;
    let mut imp = Importer::begin(&pool, &config.timezone, false).await?;

    let mut rng = rand::rng();
    let today = Utc::now().date_naive();
    let term = Months::new(36);
    let term_days = (today - (today - term)).num_days();
    let host = |net: Ipv4Network, n: u32| {
        IpNetwork::V4(Ipv4Network::new(net.nth(n).unwrap(), 32).unwrap())
    };

    // Take NAT addresses from TEST-NET-2 & allocate one /26 per resident from 10.64.0.0
    for k in 0..14 * 14u32 {
        let subnet = Ipv4Network::new(Ipv4Addr::from(0x0A40_0000 + k * 64), 26)?;
        imp.realm(
            &Ref::new("seed", "realm", k),
            PostOrPutRealmSchema {
                ipv4_subnet: Some(IpNetwork::V4(subnet)),
                ipv4_nat: Some(IpNetwork::V4(Ipv4Network::new(
                    Ipv4Addr::from(0xC633_6401 + k),
                    32,
                )?)),
                ipv4_router: Some(host(subnet, 1)),
                ipv4_dns: Some(host(subnet, 1)),
                ipv4_dhcp_server: Some(host(subnet, 2)),
                ipv6_prefix: None,
                vlan: None,
            },
        )
        .await?;
    }

    anyhow::ensure!(
        FIRST_NAMES.len() * LAST_NAMES.len() >= 14 * 14,
        "not enough name pairs for every room"
    );
    let mut taken = HashSet::new();

    for floor in 1..15 {
        for n in 1..15 {
            let room = format!("{floor:02}{n:02}");
            imp.room(CreateRoomSchema {
                label: room.clone(),
                capacity: 1,
                floor: Some(floor),
            })
            .await?;

            // 3-year contracts that started up to nearly 3 years ago
            let begins = today - TimeDelta::days(rng.random_range(0..term_days));
            let ends = begins + term;

            let (first, last) = loop {
                let pair = (
                    FIRST_NAMES.choose(&mut rng).unwrap(),
                    LAST_NAMES.choose(&mut rng).unwrap(),
                );
                if taken.insert(pair) {
                    break pair;
                }
            };
            let email = format!("{first}.{last}@example.com")
                .to_lowercase()
                .replace(' ', "-");

            let person = CreateUserSchema {
                full_name: format!("{first} {last}"),
                display_name: None,
                email: Some(email),
                auth_id: None,
                contract: Some(ContractInfoSchema {
                    room: room.clone(),
                    begins,
                    ends,
                    lessor_id: None,
                }),
                label: None,
                membership_end: PgDate::DateTime(ends),
                phone: None,
                auto_setup_network: Some(true),
                enable_amenities_login: Some(false),
            };
            imp.person(
                &Ref::new("seed", "room", &room),
                PersonImport {
                    person,
                    membership_start: Some(begins),
                    manually_suspended: false,
                },
            )
            .await?;
        }
    }

    imp.finish().await
}
