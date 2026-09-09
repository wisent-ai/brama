//! Which providers publish a usage report, at what address, and in what shape.

/// How one provider publishes what its plan has left.
///
/// A usage report is the provider's own statement about the quota it owns, and
/// reading it spends none of that quota: no completion, no output tokens, no
/// line in anybody's bill. Each shape names the fields this gateway reads out of
/// one provider's report, so the reader never guesses at a field the provider
/// does not publish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlanUsageShape {
    /// Anthropic's OAuth usage report: `five_hour`, `seven_day`,
    /// `seven_day_opus` and `seven_day_sonnet`, each an object carrying a
    /// `utilization` percentage and an RFC 3339 `resets_at`.
    AnthropicOauth,
    /// The ChatGPT backend's usage report: `rate_limit.primary_window` and
    /// `rate_limit.secondary_window`, each carrying `used_percent`,
    /// `limit_window_seconds`, and either `reset_at` or `reset_after_seconds`.
    CodexWham,
    /// Kimi's coding-plan report: a `usage` object and a `limits` array, each
    /// entry a `limit` with a `used` or a `remaining`, and an optional `window`
    /// naming its duration and its reset.
    KimiUsages,
}

/// One provider's usage report route, declared beside its chat route above so
/// both endpoints of a provider are read in one place.
pub(super) struct PlanUsageEndpoint {
    provider_id: &'static str,
    /// Absolute, because a usage report is not always a sibling of the chat
    /// route: Codex answers chat under `/backend-api/codex` and publishes usage
    /// under `/backend-api/wham`. Only the path is used when a deployment
    /// overrides the provider's base URL.
    pub(super) url: &'static str,
    pub(super) shape: PlanUsageShape,
}

/// The usage reports Brama can read freely with provider credentials.
///
/// Each is issued to exactly the credential the chat route already presents, so
/// nothing new is provisioned to learn a plan window. Other provider credentials
/// have no supported free report here. That says nothing about separately
/// privileged organization billing APIs a vendor may expose.
const PLAN_USAGE_ENDPOINTS: &[PlanUsageEndpoint] = &[
    PlanUsageEndpoint {
        provider_id: "claude-code",
        url: "https://api.anthropic.com/api/oauth/usage",
        shape: PlanUsageShape::AnthropicOauth,
    },
    PlanUsageEndpoint {
        provider_id: "codex",
        url: "https://chatgpt.com/backend-api/wham/usage",
        shape: PlanUsageShape::CodexWham,
    },
    PlanUsageEndpoint {
        provider_id: "kimi",
        url: "https://api.kimi.com/coding/v1/usages",
        shape: PlanUsageShape::KimiUsages,
    },
];

pub(super) fn plan_usage_endpoint(provider_id: &str) -> Option<&'static PlanUsageEndpoint> {
    PLAN_USAGE_ENDPOINTS
        .iter()
        .find(|endpoint| endpoint.provider_id == provider_id)
}
