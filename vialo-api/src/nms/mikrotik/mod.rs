use super::{
    models::{NmsError, PpskEntry},
    traits::NmsApi,
};
use anyhow::{Context, Result, anyhow};
use futures::future::join_all;
use reqwest::{Client, Method, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use tracing::warn;
mod models;
use models::*;

trait RouterOsRequest {
    /// Like `error_for_status`, but keeps RouterOS's error body.
    async fn send_checked(self) -> Result<Response>;

    async fn fetch<T: DeserializeOwned>(self) -> Result<T>;
}

impl RouterOsRequest for RequestBuilder {
    async fn send_checked(self) -> Result<Response> {
        let response = self.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let url = response.url().clone();
        let text = response.text().await.unwrap_or_default();
        Err(anyhow!("{url} failed ({status}): {text}"))
    }

    async fn fetch<T: DeserializeOwned>(self) -> Result<T> {
        Ok(self.send_checked().await?.json().await?)
    }
}

#[derive(Debug, Clone)]
pub struct MikrotikApi {
    username: String,
    password: String,
    hostname: String,
    client: Client,
}

impl MikrotikApi {
    pub fn new(
        hostname: impl Into<String>,
        username: impl Into<String>,
        password: impl Into<String>,
        proxy_url: &Option<String>,
    ) -> Result<Self> {
        let hostname: String = hostname.into();
        let mut builder = Client::builder().danger_accept_invalid_certs(true);

        if hostname.starts_with("unix://") {
            #[cfg(unix)]
            {
                let socket_path = hostname.strip_prefix("unix://").unwrap();
                builder = builder.unix_socket(socket_path);
            }
            #[cfg(not(unix))]
            {
                return Err(anyhow!(
                    "Unix socket URL configured for MikroTik but this platform does not support Unix sockets"
                ));
            }
        } else if let Some(proxy) = proxy_url {
            builder = builder.proxy(reqwest::Proxy::all(proxy)?);
        }

        let client = builder.build().context("Failed to create HTTP client")?;

        Ok(Self {
            username: username.into(),
            password: password.into(),
            hostname,
            client,
        })
    }

    /// Same credentials and client, different device.
    pub fn with_hostname(&self, hostname: impl Into<String>) -> Self {
        Self {
            hostname: hostname.into(),
            ..self.clone()
        }
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.client
            .request(method, format!("http://{}{}", self.hostname, path))
            .basic_auth(&self.username, Some(&self.password))
    }

    pub async fn get_bridge_vlans(&self) -> Result<Vec<BridgeVlan>> {
        self.request(Method::GET, "/rest/interface/bridge/vlan")
            .fetch()
            .await
    }

    pub async fn put_bridge_vlan(&self, vlan: &BridgeVlan) -> Result<BridgeVlan> {
        self.request(Method::PUT, "/rest/interface/bridge/vlan")
            .json(vlan)
            .fetch()
            .await
    }

    pub async fn get_neigbors(&self) -> Result<Vec<Neighbor>> {
        self.request(Method::GET, "/rest/ip/neighbor")
            .query(&[("platform", "MikroTik")])
            .fetch()
            .await
    }

    pub async fn get_multi_passphrase_group_passphrases(
        &self,
        group: Option<&str>,
    ) -> Result<Vec<MultiPassphraseGroupPassphraseGet>> {
        self.request(
            Method::GET,
            "/rest/interface/wifi/security/multi-passphrase",
        )
        .query(&[("group", group)])
        .fetch()
        .await
    }

    pub async fn delete_multi_passphrase_group_passphrase(&self, id: &[&str]) -> Result<()> {
        self.request(
            Method::POST,
            "/rest/interface/wifi/security/multi-passphrase/remove",
        )
        .json(&json!({".id": id.join(",")}))
        .send_checked()
        .await?;
        Ok(())
    }

    pub async fn put_multi_passphrase_group_passphrase(
        &self,
        passphrase: &MultiPassphraseGroupPassphrase,
    ) -> Result<MultiPassphraseGroupPassphraseGet> {
        self.request(
            Method::PUT,
            "/rest/interface/wifi/security/multi-passphrase",
        )
        .json(passphrase)
        .fetch()
        .await
    }

    /// Copies the "vialo-template" bridge VLAN once per missing VLAN without untagged ports.
    async fn ensure_bridge_vlans(&self, vlans: &HashSet<u16>) -> Result<()> {
        let bridge_vlans = self.get_bridge_vlans().await?;
        let existing: HashSet<_> = bridge_vlans
            .iter()
            .flat_map(|v| &v.vlan_ids)
            .flat_map(|r| r.0.clone())
            .collect();
        let missing: Vec<_> = vlans.difference(&existing).copied().collect();
        if missing.is_empty() {
            return Ok(());
        }

        let template = bridge_vlans
            .iter()
            .find(|v| v.comment.as_deref() == Some("vialo-template"))
            .ok_or_else(|| anyhow!("no bridge VLAN with comment \"vialo-template\""))?;
        for vlan in missing {
            self.put_bridge_vlan(&BridgeVlan {
                vlan_ids: vec![VlanRange(vlan..=vlan)],
                comment: None,
                untagged: vec![],
                ..template.clone()
            })
            .await?;
        }
        Ok(())
    }

    async fn ensure_neighbor_bridge_vlans(&self, vlans: &HashSet<u16>) -> Result<()> {
        let addresses: HashSet<_> = self
            .get_neigbors()
            .await?
            .into_iter()
            .filter_map(|n| n.address)
            .collect();

        join_all(addresses.into_iter().map(|address| async move {
            if let Err(e) = self
                .with_hostname(address.to_string())
                .ensure_bridge_vlans(vlans)
                .await
            {
                warn!("Couldn't ensure bridge VLANs on neighbor {address}: {e:?}");
            }
        }))
        .await;
        Ok(())
    }
}

impl NmsApi for MikrotikApi {
    /// `group` is the multi-passphrase group name
    async fn set_ppsks(&mut self, group: &str, entries: &[PpskEntry]) -> Result<Vec<NmsError>> {
        let entries: HashSet<_> = entries.iter().cloned().collect();
        let vlans = entries
            .iter()
            .filter_map(|e| u16::try_from(e.vlan).ok())
            .collect();
        self.ensure_neighbor_bridge_vlans(&vlans).await?;

        let current: HashMap<_, _> = self
            .get_multi_passphrase_group_passphrases(Some(group))
            .await?
            .into_iter()
            .map(|p| {
                let entry = PpskEntry {
                    passphrase: p.data.passphrase,
                    vlan: p.data.vlan_id.unwrap_or_default(),
                };
                (entry, p.id)
            })
            .collect();

        // remove extraneous
        let to_remove: Vec<_> = current
            .iter()
            .filter(|(entry, _)| !entries.contains(entry))
            .map(|(_, id)| id.as_str())
            .collect();
        if !to_remove.is_empty() {
            self.delete_multi_passphrase_group_passphrase(&to_remove)
                .await?;
        }

        // insert missing
        for e in entries.iter().filter(|e| !current.contains_key(e)) {
            self.put_multi_passphrase_group_passphrase(&MultiPassphraseGroupPassphrase {
                group: group.into(),
                passphrase: e.passphrase.clone(),
                vlan_id: Some(e.vlan),
                ..Default::default()
            })
            .await?;
        }
        Ok(vec![])
    }
}
