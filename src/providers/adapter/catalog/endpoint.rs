//! Where a catalog provider stands, and how its path is composed when the chat
//! route and the listing route are siblings rather than parent and child.

use super::super::registry::{endpoint, provider_base_url_override, validated_provider_base_url};
use crate::subscription_dispatch::model_catalog::CatalogProvider;

pub(in crate::providers::adapter) fn catalog_endpoint(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with(path) {
        return base.to_string();
    }
    if path == "/models" {
        if let Some(prefix) = base
            .strip_suffix("/chat/completions")
            .or_else(|| base.strip_suffix("/messages"))
        {
            return endpoint(prefix, path);
        }
    }
    endpoint(base, path)
}

/// The endpoint each public-catalogue provider Brama may execute is sent to,
/// by its models.dev id. The roster's own providers (claude-code, together…)
/// carry their address in their roster entry instead.
fn trusted_catalog_base_url(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        // https://docs.abliteration.ai/api/introduction: OpenAI-compatible
        // chat and model discovery, authenticated with the provider bearer.
        "abliteration-ai" => Some("https://api.abliteration.ai/v1"),
        "anthropic" => Some("https://api.anthropic.com/v1"),
        "kimi" => Some("https://api.kimi.com/coding/v1"),
        "openai" => Some("https://api.openai.com/v1"),
        "codex" => Some("https://chatgpt.com/backend-api/codex"),
        "openrouter" => Some("https://openrouter.ai/api/v1"),
        "groq" => Some("https://api.groq.com/openai/v1"),
        "mistral" => Some("https://api.mistral.ai/v1"),
        "xai" => Some("https://api.x.ai/v1"),
        "deepseek" => Some("https://api.deepseek.com"),
        "cerebras" => Some("https://api.cerebras.ai/v1"),
        "fireworks" => Some("https://api.fireworks.ai/inference/v1"),
        "togetherai" => Some("https://api.together.xyz/v1"),
        "nvidia" => Some("https://integrate.api.nvidia.com/v1"),
        "moonshot" => Some("https://api.moonshot.ai/v1"),
        "zai" => Some("https://api.z.ai/api/paas/v4"),
        "qwen" => Some("https://dashscope-intl.aliyuncs.com/compatible-mode/v1"),
        "huggingface" => Some("https://router.huggingface.co/v1"),
        "featherless" => Some("https://api.featherless.ai/v1"),
        "venice" => Some("https://api.venice.ai/api/v1"),
        "novita" => Some("https://api.novita.ai/openai/v1"),
        "synthetic" => Some("https://api.synthetic.new/v1"),
        "perplexity" => Some("https://api.perplexity.ai"),
        "deepinfra" => Some("https://api.deepinfra.com/v1/openai"),
        "google" => Some("https://generativelanguage.googleapis.com/v1beta"),
        _ => None,
    }
}

pub(in crate::providers::adapter) fn catalog_provider_base_url(
    descriptor: &CatalogProvider,
) -> Result<String, String> {
    let declared = trusted_catalog_base_url(&descriptor.id)
        .ok_or_else(|| format!("provider `{}` has no trusted endpoint", descriptor.id))?;
    if let Some(configured) = provider_base_url_override(&descriptor.id)? {
        return validated_provider_base_url(&descriptor.id, &configured, declared, true);
    }
    validated_provider_base_url(&descriptor.id, declared, declared, false)
}
