//! CLI-env provider: shells out to locally-installed coding CLIs using an
//! OAuth token stored as a router-level env var. No HMAC, no Supabase lookup,
//! no per-agent subscription — one shared token per CLI, provisioned on the
//! Cloud Run service. This is the "no API key" Claude path for callers that
//! don't have their own donation/agent identity.

use std::env;

use async_trait::async_trait;

use crate::provider::ModelProvider;
use crate::subscription_dispatch::engines;
use crate::types::{ModelRequest, ModelResponse};

pub struct ClaudeOAuthProvider;

impl ClaudeOAuthProvider {
    pub fn new() -> Self {
        Self
    }

    fn supported_models() -> Vec<String> {
        vec!["claude".into()]
    }
}

impl Default for ClaudeOAuthProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for ClaudeOAuthProvider {
    async fn complete(&self, request: &ModelRequest) -> ModelResponse {
        let token = match env::var("CLAUDE_CODE_OAUTH_TOKEN") {
            Ok(t) if !t.is_empty() => t,
            _ => {
                return ModelResponse::failure(
                    &request.model,
                    "CLAUDE_CODE_OAUTH_TOKEN not set on router — donate a Claude Code subscription token to the service env to enable this model".into(),
                );
            }
        };
        engines::run_claude_code(request, &token).await
    }

    fn estimate_cost(&self, _input_tokens: u32, _output_tokens: u32) -> f64 {
        0.0
    }

    async fn is_available(&self) -> bool {
        env::var("CLAUDE_CODE_OAUTH_TOKEN")
            .map(|t| !t.is_empty())
            .unwrap_or(false)
    }

    fn name(&self) -> &str {
        "claude-oauth"
    }

    fn models(&self) -> Vec<String> {
        Self::supported_models()
    }
}
