use super::models::{NetioGet, NetioPost, OutputPost};
use crate::bookables::connectors::error::HttpStatus;
use anyhow::{Context, Result, anyhow};
use reqwest::RequestBuilder;
use std::sync::Arc;

#[derive(Clone)]
pub struct NetioApi {
    client: Arc<reqwest::Client>,
    username: String,
    password: String,
    url: String,
}

/// Same box and login
impl PartialEq for NetioApi {
    fn eq(&self, other: &Self) -> bool {
        self.url == other.url && self.username == other.username && self.password == other.password
    }
}

impl NetioApi {
    pub fn new(
        url: String,
        username: String,
        password: String,
        proxy_url: &Option<String>,
    ) -> Self {
        let mut builder = reqwest::Client::builder();
        if let Some(proxy) = proxy_url {
            builder = builder.proxy(reqwest::Proxy::all(proxy).unwrap());
        }

        Self {
            url,
            username,
            password,
            client: Arc::new(builder.build().unwrap()),
        }
    }

    pub async fn get(&self) -> Result<NetioGet> {
        self.send(self.client.get(self.url.as_str())).await
    }

    pub async fn post(&self, outputs: &[OutputPost]) -> Result<NetioGet> {
        let body = NetioPost {
            outputs: outputs.to_vec(),
        };
        self.send(self.client.post(self.url.as_str()).json(&body))
            .await
    }

    async fn send(&self, builder: RequestBuilder) -> Result<NetioGet> {
        let response = builder
            .header("Content-Type", "application/json")
            .basic_auth(&self.username, Some(&self.password))
            .send()
            .await
            .context("Failed to send request")?;
        let status = response.status();
        let url = response.url().clone();
        let text = response
            .text()
            .await
            .context("Failed to read response body")?;

        if !status.is_success() {
            return Err(HttpStatus {
                status,
                url,
                body: text,
            }
            .into());
        }
        serde_json::from_str(&text)
            .map_err(|e| anyhow!("Failed to parse response from {url} ({e}): {text}"))
    }
}
