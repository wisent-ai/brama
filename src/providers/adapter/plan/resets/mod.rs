//! Provider reset protocols share authentication, not credentials with harnesses.

mod claude;
mod codex;
mod request;

use crate::subscription_dispatch::acquire::resets::{
    declaration::{ResetDeclaration, ResetProtocol},
    model::{ResetCredit, ResetOffer},
};
use request::request;
use serde_json::{json, Value};

pub async fn read(
    provider: &str,
    item: &str,
    secret: &str,
    declared: &ResetDeclaration,
) -> Result<ResetOffer, String> {
    let now = chrono::Utc::now().timestamp_millis();
    let body = request(provider, item, secret, &declared.listing_path, None).await?;
    match declared.protocol {
        ResetProtocol::CodexWham => {
            let mut offer = codex::parse(body, now)?;
            let path = declared.usage_path.as_deref().ok_or_else(|| {
                "Codex reset declaration lacks its usage admission path".to_owned()
            })?;
            let usage = request(provider, item, secret, path, None).await?;
            let allowed = usage
                .pointer("/rate_limit/allowed")
                .and_then(Value::as_bool);
            let limited = usage
                .pointer("/rate_limit/limit_reached")
                .and_then(Value::as_bool);
            offer.limit_reached = match (allowed, limited) {
                (Some(true), Some(false)) => Some(false),
                (Some(false), Some(true)) => Some(true),
                _ => return Err("Codex usage report lacks consistent rate_limit.allowed and rate_limit.limit_reached; reset eligibility is unknown".into()),
            };
            Ok(offer)
        }
        ResetProtocol::ClaudePrograms => {
            let cedar = claude::cedar(&body, now)?;
            let path = declared.alternate_listing_path.as_deref().ok_or_else(|| {
                "Claude reset declaration lacks its Juniper listing path".to_owned()
            })?;
            let body = request(provider, item, secret, path, None).await?;
            let juniper = claude::juniper(&body, now)?;
            claude::combine(cedar, juniper)
        }
    }
}

/// One request, never a mutation retry. The caller durably records its request id first.
pub async fn redeem(
    provider: &str,
    item: &str,
    secret: &str,
    declared: &ResetDeclaration,
    credit: &ResetCredit,
    request_id: &str,
) -> Result<Value, String> {
    let (path, payload, result_field) = match declared.protocol {
        ResetProtocol::CodexWham => (
            declared.redemption_path.clone(),
            json!({"credit_id": credit.id, "redeem_request_id": request_id}),
            "code",
        ),
        ResetProtocol::ClaudePrograms => {
            let profile_path = declared.profile_path.as_deref().ok_or_else(|| {
                "Claude reset declaration lacks its organization profile path".to_owned()
            })?;
            let profile = request(provider, item, secret, profile_path, None).await?;
            let organization = profile
                .pointer("/organization/uuid")
                .and_then(Value::as_str)
                .filter(|value| {
                    !value.is_empty()
                        && value
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                })
                .ok_or_else(|| {
                    "Claude reset profile has no safe organization identity".to_owned()
                })?;
            let payload = match credit.program.as_str() {
                "cedar_ember" => {
                    json!({"program": credit.program, "grant_id": credit.id, "request_id": request_id})
                }
                "juniper_tide" => json!({"program": credit.program}),
                _ => {
                    return Err(format!(
                        "Claude does not declare reset program {}",
                        credit.program
                    ))
                }
            };
            (
                declared
                    .redemption_path
                    .replace("{organization}", organization),
                payload,
                "result",
            )
        }
    };
    let answer = request(provider, item, secret, &path, Some(&payload)).await?;
    let result = answer
        .get(result_field)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!("provider reset response lacks {result_field}; redemption is unconfirmed")
        })?;
    Ok(json!({"ok": result == "reset", "code": result, "provider_result": answer}))
}
