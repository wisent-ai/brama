//! Part of `oauth_refresh`, split out to keep every file inside the line limit.
#![allow(unused_imports)]

use super::*;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;
use serde_json::{json, Value};
use zeroize::{Zeroize, Zeroizing};
use crate::capability::Secret;
use crate::core::failure::{self, IMPACT_CREDENTIAL_REFRESH, POINT_OAUTH_REFRESH};
use wisent_errors::{Code, Failure};

pub(crate) fn oauth_refresh_token(blob: &Value, provider: &str) -> Option<Zeroizing<String>> {
    let value = match provider {
        "claude-code" => blob.get("claudeAiOauth")?.get("refreshToken")?,
        "codex" => blob.get("tokens")?.get("refresh_token")?,
        "kimi" => blob.get("refresh_token")?,
        _ => return None,
    };
    value
        .as_str()
        .filter(|token| !token.is_empty())
        .map(|token| Zeroizing::new(token.to_owned()))
}

pub(crate) fn parse_refresh_grant(body: &Value) -> Option<RefreshGrant> {
    Some(RefreshGrant {
        access_token: body
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())?
            .to_owned(),
        refresh_token: body
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_owned),
        id_token: body
            .get("id_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_owned),
        expires_in: body
            .get("expires_in")
            .and_then(Value::as_u64)
            .filter(|seconds| *seconds > u64::default()),
    })
}

/// The provider's own words for a refused refresh.
///
/// OAuth 2.0 states the reason in `error` and `error_description`, and that
/// pair is the sentence an operator needs: `invalid_grant -- Refresh token not
/// found or invalid` says the grant is gone, which no retry repairs. A body
/// shaped some other way is carried through as it stands. Nothing here is
/// paraphrased; the provider's text is data.
pub(crate) fn provider_rejection_text(body: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(body).ok()?;
    let field = |key: &str| {
        parsed
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let code = field("error");
    let description = field("error_description")
        .or_else(|| {
            parsed
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| field("message"));
    match (code, description) {
        (Some(code), Some(description)) => Some(format!("{code} -- {description}")),
        (Some(code), None) => Some(code),
        (None, Some(description)) => Some(description),
        (None, None) => None,
    }
}

/// Read a refused response's body under the same bound the success path uses. A
/// provider that answers with a megabyte of HTML does not get to fill the log.
pub(crate) async fn bounded_error_body(response: &mut reqwest::Response) -> String {
    let mut text = String::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if text.len().saturating_add(chunk.len()) > max_response_bytes() {
            break;
        }
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    text
}

/// What a refused refresh reports: the fleet's classification of the status the
/// provider answered with, and the provider's own sentence as the detail.
///
/// The status alone was all this used to log, and the status alone is what a day
/// went into supplementing by hand. The body says which of `invalid_grant`, a
/// revoked client or a throttle it was, so it travels with the failure.
pub(crate) fn rejection_failure(status: u16, body: &str) -> Failure {
    let stated = provider_rejection_text(body).unwrap_or_else(|| body.trim().to_owned());
    let detail = if stated.is_empty() {
        format!("OAuth refresh rejected with HTTP {status}")
    } else {
        format!("OAuth refresh rejected with HTTP {status}: {stated}")
    };
    refresh_failure(Code::from_upstream_status(status), detail)
}

pub(crate) async fn request_refresh_grant(
    config: &OAuthProvider,
    refresh_token: &str,
) -> Result<RefreshGrant, Failure> {
    // One client for every refresh. A fresh `Client` per call brings a fresh
    // connection pool with it and strands the previous one's sockets.
    static REFRESH_CLIENT: std::sync::LazyLock<Result<reqwest::Client, String>> =
        std::sync::LazyLock::new(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(refresh_timeout())
                .build()
                .map_err(|_| "OAuth refresh client configuration failed".to_owned())
        });
    let client = REFRESH_CLIENT
        .clone()
        .map_err(|detail| refresh_failure(Code::Config, detail))?;
    let parameters = OAuthRefreshRequest {
        grant_type: "refresh_token",
        refresh_token,
        client_id: config.client_id,
    };
    let request = client
        .post(config.token_endpoint)
        .header(reqwest::header::ACCEPT, "application/json");
    let mut response = match config.wire {
        OAuthWire::Json => request.json(&parameters),
        OAuthWire::Form => request.form(&parameters),
    }
    .send()
    .await
    .map_err(|error| {
        let code = if error.is_timeout() {
            Code::Timeout
        } else {
            Code::InfraDown
        };
        refresh_failure(code, "OAuth refresh transport failure")
    })?;
    // The status alone was all this returned, and the status alone is what a
    // day went into supplementing by hand. The body says which of `invalid_grant`,
    // a revoked client or a throttle it was, so it travels with the failure.
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = bounded_error_body(&mut response).await;
        return Err(rejection_failure(status, &body));
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_response_bytes() as u64)
    {
        return Err(refresh_failure(
            Code::Unknown,
            "OAuth refresh response is too large",
        ));
    }
    let mut encoded = Zeroizing::new(Vec::new());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| refresh_failure(Code::InfraDown, "OAuth refresh response read failed"))?
    {
        if encoded.len().saturating_add(chunk.len()) > max_response_bytes() {
            return Err(refresh_failure(
                Code::Unknown,
                "OAuth refresh response is too large",
            ));
        }
        encoded.extend_from_slice(&chunk);
    }
    let mut body: Value = serde_json::from_slice(&encoded)
        .map_err(|_| refresh_failure(Code::Unknown, "OAuth refresh response is not JSON"))?;
    encoded.zeroize();
    let grant = parse_refresh_grant(&body);
    zeroize_json_strings(&mut body);
    grant
        .ok_or_else(|| refresh_failure(Code::Unknown, "OAuth refresh response has no access token"))
}

pub(crate) fn patch_oauth_blob(blob: &mut Value, provider: &str, grant: &RefreshGrant, now: i64) -> bool {
    match provider {
        "claude-code" => {
            let Some(oauth) = blob.get_mut("claudeAiOauth").and_then(Value::as_object_mut) else {
                return false;
            };
            oauth.insert("accessToken".to_owned(), json!(grant.access_token));
            if let Some(token) = &grant.refresh_token {
                oauth.insert("refreshToken".to_owned(), json!(token));
            }
            if let Some(expires_in) = grant.expires_in {
                oauth.insert(
                    "expiresAt".to_owned(),
                    json!((now + expires_in as i64) * millis_per_second_i64()),
                );
            }
            true
        }
        "codex" => {
            {
                let Some(tokens) = blob.get_mut("tokens").and_then(Value::as_object_mut) else {
                    return false;
                };
                tokens.insert("access_token".to_owned(), json!(grant.access_token));
                if let Some(token) = &grant.refresh_token {
                    tokens.insert("refresh_token".to_owned(), json!(token));
                }
                if let Some(token) = &grant.id_token {
                    tokens.insert("id_token".to_owned(), json!(token));
                }
            }
            if blob.get("last_refresh").is_some() {
                if let Some(stamp) = chrono::DateTime::from_timestamp(now, Default::default())
                    .map(|at| at.to_rfc3339())
                {
                    blob["last_refresh"] = json!(stamp);
                }
            }
            true
        }
        "kimi" => {
            let Some(fields) = blob.as_object_mut() else {
                return false;
            };
            fields.insert("access_token".to_owned(), json!(grant.access_token));
            if let Some(token) = &grant.refresh_token {
                fields.insert("refresh_token".to_owned(), json!(token));
            }
            if let Some(expires_in) = grant.expires_in {
                fields.insert("expires_at".to_owned(), json!(now + expires_in as i64));
            }
            true
        }
        _ => false,
    }
}

pub(crate) fn zeroize_json_strings(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(values) => values.iter_mut().for_each(zeroize_json_strings),
        Value::Object(fields) => fields.values_mut().for_each(zeroize_json_strings),
        _ => {}
    }
}
