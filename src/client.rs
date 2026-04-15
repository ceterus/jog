use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde_json::Value;

use crate::config::Credentials;

pub async fn get_json(client: &Client, creds: &Credentials, path: &str) -> Result<Value> {
    let url = format!("{}{}", creds.base_url, path);
    let resp = client
        .get(&url)
        .header("Authorization", creds.auth_header())
        .header("Accept", "application/json")
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("GET {} -> {}: {}", url, status, text));
    }
    serde_json::from_str(&text).with_context(|| format!("parse {}", url))
}

pub async fn post_json(
    client: &Client,
    creds: &Credentials,
    path: &str,
    body: &Value,
) -> Result<Value> {
    let url = format!("{}{}", creds.base_url, path);
    let resp = client
        .post(&url)
        .header("Authorization", creds.auth_header())
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        return Err(anyhow!("POST {} -> {}: {}", url, status, text));
    }
    serde_json::from_str(&text).with_context(|| format!("parse {}", url))
}
