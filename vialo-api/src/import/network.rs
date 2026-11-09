use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::types::{ipnetwork::IpNetwork, mac_address::MacAddress};
use uuid::Uuid;

use super::{Importer, Ref, api_error};
use crate::{
    helpers::{
        encryption::{self, Encrypted},
        network::generate_credentials,
    },
    http::network::{models::NetAuth, realms::insert_realm, schemas::PostOrPutRealmSchema},
    nms::unifi::{ClientSummary, UniFiApi},
};

/// How a device gets onto a network
pub enum DeviceCred {
    Existing(Uuid),
    /// Shared account cred on multi-device networks, a fresh one per device otherwise
    Generate(i32),
}

pub struct DeviceImport {
    pub account: Uuid,
    pub label: Option<String>,
    pub hostname: Option<String>,
    pub last_seen: Option<DateTime<Utc>>,
    pub mac: Option<MacAddress>,
    pub creds: Vec<DeviceCred>,
    /// Overrides the account's realm
    pub realm_id: Option<Uuid>,
}

pub struct ImportedDevice {
    pub id: Uuid,
    /// (cred, network) pairs the device was linked to
    pub creds: Vec<(Uuid, i32)>,
}

impl Importer {
    /// Clients known to any of vialo's UniFi connectors: the history, then the currently connected ones
    pub async fn unifi_clients(&mut self, proxy: &Option<String>) -> Result<Vec<ClientSummary>> {
        let connectors = sqlx::query!(
            r#"SELECT hostname, api_key AS "api_key: Encrypted<String>", site_id FROM net_nms_connectors WHERE type = 'unifi'"#
        )
        .fetch_all(self.conn())
        .await?;

        let mut clients = Vec::new();
        for c in connectors {
            let (Some(key), Some(site)) = (c.api_key, c.site_id) else {
                self.warn(
                    None,
                    format!("UniFi connector {} lacks an API key or site", c.hostname),
                );
                continue;
            };
            let api = UniFiApi::new(c.hostname.clone(), key.expose(), site, proxy)?;
            // Unofficial, so only worth a warning
            match api.get_client_history().await {
                Ok(history) => clients.extend(history),
                Err(e) => self.warn(None, format!("UniFi {} client history: {e:#}", c.hostname)),
            }
            clients.extend(
                api.get_clients()
                    .await
                    .with_context(|| format!("UniFi {}", c.hostname))?,
            );
        }
        Ok(clients)
    }

    pub async fn network_by_label(&mut self, label: &str) -> Result<Option<i32>> {
        Ok(
            sqlx::query_scalar!("SELECT id FROM net_networks WHERE label = $1", label)
                .fetch_optional(self.conn())
                .await?,
        )
    }

    /// Imports a cred with known plaintext credentials.
    pub async fn cred(
        &mut self,
        r: &Ref,
        account: Uuid,
        network_id: i32,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<Uuid> {
        if let Some(id) = self.existing(r, "cred").await? {
            return Ok(id);
        }

        if let Some(username) = username
            && let Some(holder) = sqlx::query_scalar!(
                "SELECT account_id FROM net_cred WHERE network_id = $1 AND username = $2",
                network_id,
                username
            )
            .fetch_optional(self.conn())
            .await?
        {
            self.warn(
                Some(r),
                format!("username {username} already used by {holder}"),
            );
        }

        let id = self
            .insert_cred(account, network_id, username, password)
            .await?;
        self.remember(Some(r), "cred", id).await?;
        Ok(id)
    }

    async fn insert_cred(
        &mut self,
        account: Uuid,
        network_id: i32,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<Uuid> {
        let password = password.map(|p| encryption::encrypt(&p)).transpose()?;
        Ok(sqlx::query_scalar!(
            "INSERT INTO net_cred (username, password, account_id, network_id, last_updated) VALUES ($1, $2, $3, $4, NOW()) RETURNING id",
            username,
            password,
            account,
            network_id
        )
        .fetch_one(self.conn())
        .await?)
    }

    async fn generated_cred(&mut self, account: Uuid, network_id: i32) -> Result<(Uuid, i32)> {
        let network = sqlx::query!(
            r#"SELECT multi_device, auth AS "auth: NetAuth", gen_username, gen_password FROM net_networks WHERE id = $1"#,
            network_id
        )
        .fetch_one(self.conn())
        .await?;

        if network.multi_device
            && let Some(id) = sqlx::query_scalar!(
                "SELECT id FROM net_cred WHERE account_id = $1 AND network_id = $2 LIMIT 1",
                account,
                network_id
            )
            .fetch_optional(self.conn())
            .await?
        {
            return Ok((id, network_id));
        }

        let (username, password) = generate_credentials(
            network.auth.as_ref(),
            network.gen_username,
            network.gen_password,
        );
        let id = self
            .insert_cred(
                account,
                network_id,
                username.as_deref(),
                password.as_deref(),
            )
            .await?;
        *self.report.created.entry("cred").or_default() += 1;
        Ok((id, network_id))
    }

    async fn device_creds(&mut self, device: Uuid) -> Result<Vec<(Uuid, i32)>> {
        Ok(sqlx::query!(
            "SELECT cred_id, network_id FROM net_device_creds WHERE device_id = $1",
            device
        )
        .fetch_all(self.conn())
        .await?
        .into_iter()
        .map(|c| (c.cred_id, c.network_id))
        .collect())
    }

    /// Imports a device. One whose MAC is on a device of the same account joins it instead,
    /// one whose MAC another account holds is skipped, as is one left without networks.
    pub async fn device(&mut self, r: &Ref, d: DeviceImport) -> Result<Option<ImportedDevice>> {
        if let Some(id) = self.existing::<Uuid>(r, "device").await? {
            let creds = self.device_creds(id).await?;
            return Ok(Some(ImportedDevice { id, creds }));
        }

        // A MAC may repeat across one device's creds, never across devices
        let holder = match d.mac {
            Some(mac) => sqlx::query!(
                "SELECT nd.id, nd.account_id FROM net_device_creds ndc JOIN net_devices nd ON nd.id = ndc.device_id
                 WHERE ndc.mac = $1 LIMIT 1",
                mac
            )
            .fetch_optional(self.conn())
            .await?
            .map(|h| (h.id, h.account_id)),
            None => None,
        };
        if let Some((other, account)) = holder
            && account != d.account
        {
            self.warn(
                Some(r),
                format!("MAC already on device {other} of account {account}, skipped"),
            );
            return Ok(None);
        }

        let mut links = Vec::with_capacity(d.creds.len());
        for cred in d.creds {
            links.push(match cred {
                DeviceCred::Existing(id) => {
                    let network_id = sqlx::query_scalar!(
                        "SELECT network_id FROM net_cred WHERE id = $1 AND account_id = $2",
                        id,
                        d.account
                    )
                    .fetch_one(self.conn())
                    .await?;
                    (id, network_id)
                }
                DeviceCred::Generate(network_id) => {
                    self.generated_cred(d.account, network_id).await?
                }
            });
        }

        if links.is_empty() {
            self.warn(Some(r), "no networks, skipped");
            return Ok(None);
        }

        let (id, target_kind) = match holder {
            Some((id, _)) => (id, "device_merged"),
            None => (
                sqlx::query_scalar!(
                    "INSERT INTO net_devices (account_id, label, hostname, last_seen, realm_id, last_updated) VALUES ($1, $2, $3, $4, $5, NOW()) RETURNING id",
                    d.account,
                    d.label,
                    d.hostname,
                    d.last_seen,
                    d.realm_id
                )
                .fetch_one(self.conn())
                .await?,
                "device",
            ),
        };

        // A joined device may already be on the cred or the network
        for &(cred_id, network_id) in &links {
            sqlx::query!(
                "INSERT INTO net_device_creds (device_id, cred_id, network_id, account_id, mac) VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT DO NOTHING",
                id,
                cred_id,
                network_id,
                d.account,
                d.mac
            )
            .execute(self.conn())
            .await?;
        }

        self.remember(Some(r), target_kind, id).await?;
        let creds = self.device_creds(id).await?;
        Ok(Some(ImportedDevice { id, creds }))
    }

    /// Creates a realm through the API's insert, the realm trigger then fills its IP pool.
    pub async fn realm(&mut self, r: &Ref, realm: PostOrPutRealmSchema) -> Result<Uuid> {
        if let Some(id) = self.existing(r, "realm").await? {
            return Ok(id);
        }

        let id = insert_realm(self.conn(), realm)
            .await
            .map_err(api_error)?
            .id;

        self.remember(Some(r), "realm", id).await?;
        Ok(id)
    }

    pub async fn realm_assignment(&mut self, account: Uuid, realm: Uuid) -> Result<()> {
        sqlx::query!(
            "INSERT INTO net_realm_assignments (account_id, realm_id) VALUES ($1, $2)
             ON CONFLICT (account_id) DO UPDATE SET realm_id = $2",
            account,
            realm
        )
        .execute(self.conn())
        .await?;
        Ok(())
    }

    /// Leases a pool address to a device cred until `expires`, `None` = static. Returns false (and warns) if it can't.
    pub async fn assign_ip(
        &mut self,
        r: &Ref,
        addr: IpNetwork,
        device: Uuid,
        cred: Uuid,
        expires: Option<DateTime<Utc>>,
    ) -> Result<bool> {
        let holds_one = sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM net_ip_assignments WHERE device_id = $1 AND cred_id = $2) AS "exists!""#,
            device,
            cred
        )
        .fetch_one(self.conn())
        .await?;
        if holds_one {
            self.warn(
                Some(r),
                format!("device {device} already has an address, {addr} not assigned"),
            );
            return Ok(false);
        }

        let reserved = sqlx::query!(
            "UPDATE net_ip_assignments SET (device_id, cred_id, expires) = ($2, $3, COALESCE($4::timestamptz, 'infinity'))
             WHERE host(ipv4_addr) = host($1) AND device_id IS NULL",
            addr,
            device,
            cred,
            expires
        )
        .execute(self.conn())
        .await?
        .rows_affected()
            == 1;

        if !reserved {
            self.warn(Some(r), format!("{addr} is not a free pool address"));
        }
        Ok(reserved)
    }
}
