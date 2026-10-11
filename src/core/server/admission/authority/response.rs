//! Strict authority response decoding with credential-safe HTTP diagnostics.

use serde::de::DeserializeOwned;

pub(super) async fn read_json<T: DeserializeOwned>(
    response: reqwest::Response,
    method: &str,
    url: &str,
    credentials: &[&str],
) -> Result<T, String> {
    let status = response.status();
    let body = response.bytes().await.map_err(|error| {
        format!("{method} {url} answered {status}; cannot read its response body: {error}")
    })?;
    if !status.is_success() {
        let mut detail = String::from_utf8_lossy(&body).into_owned();
        for &credential in credentials {
            if !credential.is_empty() && detail.contains(credential) {
                detail = detail.replace(credential, "[redacted]");
            }
        }
        return Err(format!("{method} {url} answered {status}: {detail}"));
    }
    serde_json::from_slice(&body).map_err(|error| {
        format!("{method} {url} answered {status} with a body that is not valid JSON: {error}")
    })
}
