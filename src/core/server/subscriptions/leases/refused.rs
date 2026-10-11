//! `POST /v1/subscription-pool/leases/refused`: a lease holder reports that
//! the provider refused the member it leased, until the instant the provider
//! named. A harness that keeps its own sign-in (OMP) meets the refusal
//! without this gateway seeing it; the report blocks the member and releases
//! the session's leases on that provider, so the session's next lease lands
//! on another member of the same provider, or the shortage starts an
//! acquisition.

use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::refusal::{api_error, ApiError};
use crate::subscription_dispatch::leases::refused::{report, ReportRefused};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct ReportRequest {
    session_id: String,
    provider: String,
    /// The instant the provider said it may be asked again, RFC 3339.
    retry_at: String,
    /// The provider's own sentence.
    reason: String,
}

/// Block every member the session leased on the provider until `retry_at`
/// and release those leases. An agent reports only the leases it took.
pub(in crate::core::server) async fn report_refused(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let scope = super::super::subscription_pool_scope(&client_identity, &headers, &body).await?;
    let request: ReportRequest = serde_json::from_slice(&body).map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "invalid refusal report: {error}; a report names session_id, provider, retry_at (RFC 3339) and reason"
            ),
        )
    })?;
    let until_ms = chrono::DateTime::parse_from_rfc3339(request.retry_at.trim())
        .map_err(|error| {
            api_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "the refusal report's retry_at {:?} is not an RFC 3339 instant: {error}",
                    request.retry_at
                ),
            )
        })?
        .timestamp_millis();
    let reported = report(
        &request.session_id,
        &request.provider,
        until_ms,
        &request.reason,
        scope.agent(),
        chrono::Utc::now().timestamp_millis(),
    );
    match reported {
        Ok(reported) => Ok(Json(json!({
            "blocked": reported.blocked,
            "until_ms": reported.until_ms,
        }))),
        // Nothing is known to be refused: the session holds no lease on the
        // provider (it started on its harness's own sign-in), so the answer
        // blocks nothing and says why.
        Err(ReportRefused::NotHeld(detail)) => Ok(Json(json!({
            "blocked": [],
            "until_ms": until_ms,
            "detail": detail,
        }))),
        Err(ReportRefused::Invalid(detail)) => Err(api_error(StatusCode::BAD_REQUEST, &detail)),
        Err(ReportRefused::Unavailable(detail)) => {
            Err(api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))
        }
    }
}
