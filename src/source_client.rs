use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

#[derive(Clone)]
pub struct UpstreamClient {
    client: Client,
}

impl UpstreamClient {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(12))
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self { client })
    }

    pub async fn get_json(&self, base_url: &str, path: &str) -> Result<Value> {
        let url = join_url(base_url, path);
        Ok(self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn post_json(&self, base_url: &str, path: &str, body: Value) -> Result<Value> {
        let url = join_url(base_url, path);
        Ok(self
            .client
            .post(url)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
}

fn join_url(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}
