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

pub(crate) async fn refresh(
    secret: &Secret,
    provider: &str,
) -> Result<Zeroizing<Vec<u8>>, Failure> {
    // Every arm below describes credential material this deployment stored in a
    // shape the refresh cannot use, which is `config`: waiting does not change
    // it and the provider was never asked.
    let config = oauth_provider(provider)
        .ok_or_else(|| refresh_failure(Code::Config, "provider does not support OAuth refresh"))?;
    let raw = secret
        .expose_utf8()
        .map_err(|_| refresh_failure(Code::Config, "OAuth credential is not UTF-8"))?;
    let mut blob: Value = serde_json::from_str(raw)
        .map_err(|_| refresh_failure(Code::Config, "OAuth credential is not JSON"))?;
    if !blob.is_object() {
        zeroize_json_strings(&mut blob);
        return Err(refresh_failure(
            Code::Config,
            "OAuth credential is not an object",
        ));
    }
    let refresh_token = match oauth_refresh_token(&blob, provider) {
        Some(token) => token,
        None => {
            zeroize_json_strings(&mut blob);
            return Err(refresh_failure(
                Code::Config,
                "OAuth credential has no refresh token",
            ));
        }
    };
    let result = async {
        let grant = request_refresh_grant(&config, &refresh_token).await?;
        if !patch_oauth_blob(&mut blob, provider, &grant, now_seconds()) {
            return Err(refresh_failure(
                Code::Config,
                "OAuth credential shape mismatch",
            ));
        }
        let fresh = Zeroizing::new(serde_json::to_vec(&blob).map_err(|_| {
            refresh_failure(
                Code::Config,
                "refreshed OAuth credential is not serializable",
            )
        })?);
        if fresh.is_empty() || fresh.len() > max_credential_bytes() {
            return Err(refresh_failure(
                Code::Config,
                "refreshed OAuth credential size is invalid",
            ));
        }
        Ok(fresh)
    }
    .await;
    zeroize_json_strings(&mut blob);
    result
}
