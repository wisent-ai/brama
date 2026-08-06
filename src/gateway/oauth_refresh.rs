use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Serialize;
use serde_json::{json, Value};
use zeroize::{Zeroize, Zeroizing};

use crate::capability::Secret;

const EXPIRY_KEYS: &[&str] = &["expiresAt", "expires_at", "expires", "expiry"];

#[derive(Clone, Copy)]
enum OAuthWire {
    Json,
    Form,
}

struct OAuthProvider {
    token_endpoint: &'static str,
    client_id: &'static str,
    wire: OAuthWire,
}

fn oauth_provider(provider: &str) -> Option<OAuthProvider> {
    match provider {
        "claude-code" => Some(OAuthProvider {
            token_endpoint: "https://claude.ai/v1/oauth/token",
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
            wire: OAuthWire::Json,
        }),
        "codex" => Some(OAuthProvider {
            token_endpoint: "https://auth.openai.com/oauth/token",
            client_id: "app_EMoamEEZ73f0CkXaXp7hrann",
            wire: OAuthWire::Form,
        }),
        "kimi" => Some(OAuthProvider {
            token_endpoint: "https://auth.kimi.com/api/oauth/token",
            client_id: "17e5f671-d194-4dfb-9706-5516cb48c098",
            wire: OAuthWire::Form,
        }),
        _ => None,
    }
}

#[derive(Serialize)]
struct OAuthRefreshRequest<'a> {
    grant_type: &'static str,
    refresh_token: &'a str,
    client_id: &'static str,
}

struct RefreshGrant {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

impl Drop for RefreshGrant {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(token) = self.refresh_token.as_mut() {
            token.zeroize();
        }
        if let Some(token) = self.id_token.as_mut() {
            token.zeroize();
        }
    }
}

fn expiry_margin_seconds() -> i64 {
    "60".parse().expect("valid OAuth expiry margin")
}

fn refresh_timeout() -> Duration {
    Duration::from_secs("15".parse().expect("valid OAuth refresh timeout"))
}

fn max_response_bytes() -> usize {
    "65536".parse().expect("valid OAuth response limit")
}

fn max_credential_bytes() -> usize {
    "8192".parse().expect("valid credential size limit")
}

fn epoch_millis_threshold() -> f64 {
    "100000000000"
        .parse()
        .expect("valid epoch millisecond threshold")
}

fn millis_per_second_f64() -> f64 {
    "1000".parse().expect("valid milliseconds per second")
}

fn millis_per_second_i64() -> i64 {
    "1000".parse().expect("valid milliseconds per second")
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

fn normalize_epoch(epoch: f64) -> i64 {
    if epoch.abs() >= epoch_millis_threshold() {
        (epoch / millis_per_second_f64()) as i64
    } else {
        epoch as i64
    }
}

fn expiry_epoch_seconds(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_f64().map(normalize_epoch),
        Value::String(text) => {
            let text = text.trim();
            if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(text) {
                return Some(parsed.timestamp());
            }
            text.parse::<i64>()
                .ok()
                .map(|epoch| normalize_epoch(epoch as f64))
        }
        _ => None,
    }
}

fn expiry_in_value(value: &Value) -> Option<i64> {
    match value {
        Value::Object(fields) => EXPIRY_KEYS
            .iter()
            .find_map(|key| fields.get(*key).and_then(expiry_epoch_seconds))
            .or_else(|| fields.values().find_map(expiry_in_value)),
        Value::Array(values) => values.iter().find_map(expiry_in_value),
        _ => None,
    }
}

fn access_token<'a>(blob: &'a Value, provider: &str) -> Option<&'a str> {
    let value = match provider {
        "claude-code" => blob.get("claudeAiOauth")?.get("accessToken")?,
        "codex" => blob.get("tokens")?.get("access_token")?,
        "kimi" => blob.get("access_token")?,
        _ => return None,
    };
    value.as_str().filter(|token| !token.is_empty())
}

fn jwt_expiry(token: &str) -> Option<i64> {
    let mut segments = token.split('.');
    segments.next()?;
    let payload = segments.next()?;
    let mut decoded = Zeroizing::new(URL_SAFE_NO_PAD.decode(payload).ok()?);
    let mut claims: Value = serde_json::from_slice(&decoded).ok()?;
    decoded.zeroize();
    let expiry = claims.get("exp").and_then(expiry_epoch_seconds);
    zeroize_json_strings(&mut claims);
    expiry
}

pub(super) fn needs_refresh(secret: &Secret, provider: &str) -> bool {
    if oauth_provider(provider).is_none() {
        return false;
    }
    let raw = match secret.expose_utf8() {
        Ok(raw) => raw,
        Err(_) => return false,
    };
    let mut blob: Value = match serde_json::from_str(raw) {
        Ok(Value::Object(fields)) => Value::Object(fields),
        _ => return false,
    };
    let expiry =
        expiry_in_value(&blob).or_else(|| access_token(&blob, provider).and_then(jwt_expiry));
    zeroize_json_strings(&mut blob);
    expiry
        .map(|expiry| now_seconds() + expiry_margin_seconds() >= expiry)
        .unwrap_or(false)
}

fn oauth_refresh_token(blob: &Value, provider: &str) -> Option<Zeroizing<String>> {
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

fn parse_refresh_grant(body: &Value) -> Option<RefreshGrant> {
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

async fn request_refresh_grant(
    config: &OAuthProvider,
    refresh_token: &str,
) -> Result<RefreshGrant, String> {
    // One client for every refresh. A fresh `Client` per call brings a fresh
    // connection pool with it and strands the previous one's sockets.
    static REFRESH_CLIENT: std::sync::OnceLock<Result<reqwest::Client, String>> =
        std::sync::OnceLock::new();
    let client = REFRESH_CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(refresh_timeout())
                .build()
                .map_err(|_| "OAuth refresh client configuration failed".to_owned())
        })
        .clone()?;
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
    .map_err(|_| "OAuth refresh transport failure".to_owned())?;
    if !response.status().is_success() {
        return Err(format!(
            "OAuth refresh rejected with HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_response_bytes() as u64)
    {
        return Err("OAuth refresh response is too large".to_owned());
    }
    let mut encoded = Zeroizing::new(Vec::new());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "OAuth refresh response read failed".to_owned())?
    {
        if encoded.len().saturating_add(chunk.len()) > max_response_bytes() {
            return Err("OAuth refresh response is too large".to_owned());
        }
        encoded.extend_from_slice(&chunk);
    }
    let mut body: Value = serde_json::from_slice(&encoded)
        .map_err(|_| "OAuth refresh response is not JSON".to_owned())?;
    encoded.zeroize();
    let grant = parse_refresh_grant(&body);
    zeroize_json_strings(&mut body);
    grant.ok_or_else(|| "OAuth refresh response has no access token".to_owned())
}

fn patch_oauth_blob(blob: &mut Value, provider: &str, grant: &RefreshGrant, now: i64) -> bool {
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

fn zeroize_json_strings(value: &mut Value) {
    match value {
        Value::String(text) => text.zeroize(),
        Value::Array(values) => values.iter_mut().for_each(zeroize_json_strings),
        Value::Object(fields) => fields.values_mut().for_each(zeroize_json_strings),
        _ => {}
    }
}

pub(super) async fn refresh(secret: &Secret, provider: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let config = oauth_provider(provider)
        .ok_or_else(|| "provider does not support OAuth refresh".to_owned())?;
    let raw = secret
        .expose_utf8()
        .map_err(|_| "OAuth credential is not UTF-8".to_owned())?;
    let mut blob: Value =
        serde_json::from_str(raw).map_err(|_| "OAuth credential is not JSON".to_owned())?;
    if !blob.is_object() {
        zeroize_json_strings(&mut blob);
        return Err("OAuth credential is not an object".to_owned());
    }
    let refresh_token = match oauth_refresh_token(&blob, provider) {
        Some(token) => token,
        None => {
            zeroize_json_strings(&mut blob);
            return Err("OAuth credential has no refresh token".to_owned());
        }
    };
    let result = async {
        let grant = request_refresh_grant(&config, &refresh_token).await?;
        if !patch_oauth_blob(&mut blob, provider, &grant, now_seconds()) {
            return Err("OAuth credential shape mismatch".to_owned());
        }
        let fresh = Zeroizing::new(
            serde_json::to_vec(&blob)
                .map_err(|_| "refreshed OAuth credential is not serializable".to_owned())?,
        );
        if fresh.is_empty() || fresh.len() > max_credential_bytes() {
            return Err("refreshed OAuth credential size is invalid".to_owned());
        }
        Ok(fresh)
    }
    .await;
    zeroize_json_strings(&mut blob);
    result
}
