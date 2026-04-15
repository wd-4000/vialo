use reqwest::RequestBuilder;
use std::sync::Arc;
pub mod models;

pub struct NetioApi {
    client: Arc<reqwest::Client>,
    username: String,
    password: String,
    url: String,
}

impl NetioApi {
    pub fn new(
        url: String,
        username: String,
        password: String,
        proxy_url: &Option<String>,
    ) -> NetioApi {
        let mut builder = reqwest::Client::builder();
        if let Some(proxy) = proxy_url {
            builder = builder.proxy(reqwest::Proxy::all(proxy).unwrap());
        }

        return NetioApi {
            url,
            username,
            password,
            client: Arc::new(builder.build().unwrap()),
        };
    }

    pub fn get(&mut self) -> RequestBuilder {
        return self
            .client
            .get(self.url.as_str())
            .header("Content-Type", "application/json")
            .basic_auth(&self.username, Some(&self.password));
    }

    pub fn post(&mut self) -> RequestBuilder {
        return self
            .client
            .post(self.url.as_str())
            .header("Content-Type", "application/json")
            .basic_auth(&self.username, Some(&self.password));
    }
}
