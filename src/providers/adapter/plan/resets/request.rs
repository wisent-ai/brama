//! Authenticated capacity operations using Brama's independent provider grant.

use crate::providers::adapter::call::credential::{authorize_provider, credential_key};
use crate::providers::adapter::registry::{provider, provider_base_url};
use serde_json::Value;

pub async fn request(
    provider_id: &str,
    item: &str,
    secret: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Value, String> {
    let descriptor = provider(provider_id)
        .ok_or_else(|| format!("reset provider {provider_id} is not registered"))?;
    let base = reqwest::Url::parse(&provider_base_url(descriptor)?)
        .map_err(|error| format!("reset provider origin: {error}"))?;
    let url = base
        .join(path)
        .map_err(|error| format!("reset endpoint: {error}"))?;
    if url.origin() != base.origin() {
        return Err("reset declaration must keep the provider's authenticated origin".into());
    }
    let key = credential_key(item, secret)?;
    let client = crate::providers::adapter::control_client()?;
    let (operation, request) = match body {
        Some(body) => ("POST", client.post(url.clone()).json(body)),
        None => ("GET", client.get(url.clone())),
    };
    let request = authorize_provider(
        request.header("accept", "application/json"),
        descriptor,
        &key,
        secret,
    );
    let response = stado_wait::http::request(request).await.map_err(|error| {
        format!("{operation} {url} for {item} failed before a provider response: {error}")
    })?;
    let status = response.status();
    let answer: Value = stado_wait::until(
        stado_wait::Kind::Network,
        format!("decode reset {operation} HTTP {status}"),
        url.to_string(),
        response.json(),
    )
    .await
    .map_err(|error| {
        format!("{operation} {url} for {item} HTTP {status}: invalid JSON: {error}")
    })?;
    if !status.is_success() {
        return Err(format!(
            "{operation} {url} for {item} HTTP {status}: {answer}"
        ));
    }
    Ok(answer)
}
