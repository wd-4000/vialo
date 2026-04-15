use anyhow::{Context, Result, anyhow};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct UniFiApi {
    api_key: String,
    site_id: String,
    hostname: String,
    client: Client,
}

impl UniFiApi {
    pub fn new(
        hostname: impl Into<String>,
        api_key: impl Into<String>,
        site_id: impl Into<String>,
    ) -> Result<Self, anyhow::Error> {
        let client = Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            api_key: api_key.into(),
            site_id: site_id.into(),
            hostname: hostname.into(),
            client,
        })
    }

    pub async fn authenticated_request<T: for<'de> Deserialize<'de> + std::fmt::Debug>(
        &self,
        path: &str,
        method: Method,
        data: Option<&impl Serialize>,
    ) -> Result<T> {
        let url = format!("https://{}{}", self.hostname, path);

        let mut request = self
            .client
            .request(method, &url)
            .header("X-API-KEY", &self.api_key)
            .header("Content-Type", "application/json");

        if let Some(body) = data {
            request = request.json(body);
        }

        let response = request.send().await.context("Failed to send request")?;
        let text = response
            .text()
            .await
            .context("Failed to read response body")?;
        let body: T = serde_json::from_str(&text)
            .map_err(|e| anyhow!("Failed to parse response ({e}): {text}"))?;

        Ok(body)
    }

    async fn fetch_all<T: for<'de> Deserialize<'de> + std::fmt::Debug>(
        &self,
        base_path: &str,
    ) -> Result<Vec<T>> {
        let mut all = vec![];
        let mut offset = 0usize;
        let limit = 100usize;
        loop {
            let path = format!("{}?offset={}&limit={}", base_path, offset, limit);
            let resp: UniFiListResponse<T> = self
                .authenticated_request(&path, Method::GET, None::<&()>)
                .await?;
            let fetched = resp.data.len();
            all.extend(resp.data);
            let total = resp.total_count.unwrap_or(all.len() as i64) as usize;
            if all.len() >= total || fetched < limit {
                break;
            }
            offset += limit;
        }
        Ok(all)
    }

    pub async fn get_network_conf(&self) -> Result<Vec<Network>> {
        self.fetch_all(&format!(
            "/proxy/network/integration/v1/sites/{}/networks",
            self.site_id
        ))
        .await
    }

    pub async fn get_wlans(&self) -> Result<Vec<WlanSummary>> {
        self.fetch_all(&format!(
            "/proxy/network/integration/v1/sites/{}/wifi/broadcasts",
            self.site_id
        ))
        .await
    }

    pub async fn get_wlan(&self, id: &str) -> Result<Wlan> {
        self.authenticated_request(
            &format!(
                "/proxy/network/integration/v1/sites/{}/wifi/broadcasts/{}",
                self.site_id, id
            ),
            Method::GET,
            None::<&()>,
        )
        .await
    }

    pub async fn set_wlan(&self, wlan: &Wlan) -> Result<Value> {
        let path = format!(
            "/proxy/network/integration/v1/sites/{}/wifi/broadcasts/{}",
            self.site_id, wlan.id
        );
        let mut body = serde_json::to_value(wlan)?;
        if let Some(obj) = body.as_object_mut() {
            obj.remove("id");
            obj.remove("metadata");
        }
        self.authenticated_request(&path, Method::PUT, Some(&body))
            .await
    }
}

/* Type definitions */

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UniFiListResponse<T> {
    pub data: Vec<T>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
    pub count: Option<i64>,
    pub total_count: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub id: String,
    pub name: String,
    pub enabled: Option<bool>,
    pub vlan_id: i64,
    pub default: Option<bool>,
    pub management: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WlanSummary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(rename = "type")]
    pub wlan_type: String,
    pub metadata: Option<Value>,
    pub network: Option<Value>,
    pub security_configuration: WlanSecurityConfigSummary,
    pub broadcasting_device_filter: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WlanSecurityConfigSummary {
    #[serde(rename = "type")]
    pub config_type: String,
}

/// Full Wlan object for GET/PUT. Uses flatten to round-trip unknown fields.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Wlan {
    pub id: String,
    pub name: String,
    pub security_configuration: WlanSecurityConfig,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WlanSecurityConfig {
    pub preshared_keys: Option<Vec<WlanPresharedKey>>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WlanPresharedKey {
    pub network: WlanPresharedKeyNetwork,
    pub passphrase: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WlanPresharedKeyNetworkType {
    Native,
    Specific,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WlanPresharedKeyNetwork {
    #[serde(rename = "type")]
    pub network_type: WlanPresharedKeyNetworkType,
    pub network_id: Option<String>,
}
