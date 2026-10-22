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

    pub fn get(&mut self) -> RequestBuilder {
        self.client
            .get(self.url.as_str())
            .header("Content-Type", "application/json")
            .basic_auth(&self.username, Some(&self.password))
    }

    pub fn post(&mut self) -> RequestBuilder {
        self.client
            .post(self.url.as_str())
            .header("Content-Type", "application/json")
            .basic_auth(&self.username, Some(&self.password))
    }
}
