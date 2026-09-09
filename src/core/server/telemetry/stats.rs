//! `GET /stats`: the counters, the provider roster as this build sees it, the
//! per-model latency history, and the limits every request is held to.

use std::sync::atomic::Ordering;

use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::core::server::chat::request::{max_output_tokens, request_deadline};

use super::{
    STARTED_AT, TOTAL_FAILURES, TOTAL_INPUT_TOKENS, TOTAL_OUTPUT_TOKENS, TOTAL_PROVIDER_ATTEMPTS,
    TOTAL_REQUESTS,
};

/// How one provider's request dialect is spelled to a reader. Shared with the
/// console's snapshot, which answers the same question about the same roster.
pub(in crate::core::server) fn wire_protocol_name(
    protocol: crate::providers::adapter::WireProtocol,
) -> &'static str {
    use crate::providers::adapter::WireProtocol;

    match protocol {
        WireProtocol::OpenAiChat => "openai-chat",
        WireProtocol::AnthropicMessages => "anthropic-messages",
        WireProtocol::OpenAiResponses => "openai-responses",
    }
}

pub(in crate::core::server) async fn get_stats() -> impl IntoResponse {
    let provider_descriptors = crate::providers::adapter::providers();
    let configured_direct_providers = provider_descriptors
        .iter()
        .filter(|provider| crate::gateway::broker::provider_capability_configured(provider.id))
        .count();
    let providers = provider_descriptors
        .iter()
        .map(|provider| {
            json!({
                "id": provider.id,
                "displayName": provider.display_name,
                "wireProtocol": wire_protocol_name(provider.wire),
                "configured": crate::gateway::broker::provider_capability_configured(provider.id),
            })
        })
        .collect::<Vec<_>>();
    let models = crate::core::perf::snapshot()
        .into_iter()
        .map(|model| {
            json!({
                "model": model.model,
                "count": model.count,
                "latencyMs": model.latency_ms,
                "tps": model.tps,
                "lastLatencyMs": model.last_latency_ms,
                "lastTps": model.last_tps,
            })
        })
        .collect::<Vec<_>>();

    Json(json!({
        "build": crate::build_info::current(),
        "total_requests": TOTAL_REQUESTS.load(Ordering::Relaxed),
        "total_failures": TOTAL_FAILURES.load(Ordering::Relaxed),
        "total_provider_attempts": TOTAL_PROVIDER_ATTEMPTS.load(Ordering::Relaxed),
        "total_input_tokens": TOTAL_INPUT_TOKENS.load(Ordering::Relaxed),
        "total_output_tokens": TOTAL_OUTPUT_TOKENS.load(Ordering::Relaxed),
        "perfModels": models.len(),
        "configuredDirectProviders": configured_direct_providers,
        "uptimeSeconds": STARTED_AT.elapsed().as_secs(),
        "providers": providers,
        "models": models,
        "limits": {
            "maxOutputTokens": max_output_tokens(),
            "requestDeadlineSeconds": request_deadline().as_secs(),
        },
        "dependencyPolicy": {
            "catalog": "lazy",
            "capabilityBroker": "final-use",
            "subscriptions": "lazy",
        },
    }))
}
