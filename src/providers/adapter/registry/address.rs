//! Which URL a request is allowed to leave for.
//!
//! A declaration carries an origin, a deployment may point a provider at its
//! own proxy, and both are checked against the same trusted-host policy before
//! anything is sent.

use super::ProviderDescriptor;

pub(in crate::providers::adapter) fn endpoint(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

fn trusted_provider_hosts(provider_id: &str) -> Option<&'static [&'static str]> {
    match provider_id {
        "anthropic" | "claude-code" => Some(&["api.anthropic.com"]),
        "kimi" => Some(&["api.kimi.com"]),
        "openai" => Some(&["api.openai.com"]),
        "codex" => Some(&["chatgpt.com"]),
        "openrouter" => Some(&["openrouter.ai"]),
        "groq" => Some(&["api.groq.com"]),
        "mistral" => Some(&["api.mistral.ai"]),
        "xai" => Some(&["api.x.ai"]),
        "deepseek" => Some(&["api.deepseek.com"]),
        "cerebras" => Some(&["api.cerebras.ai"]),
        "fireworks" => Some(&["api.fireworks.ai"]),
        "together" | "togetherai" => Some(&["api.together.xyz"]),
        "nvidia" => Some(&["integrate.api.nvidia.com"]),
        "moonshot" => Some(&["api.moonshot.ai"]),
        "zai" => Some(&["api.z.ai"]),
        "qwen" => Some(&["dashscope-intl.aliyuncs.com"]),
        "huggingface" => Some(&["router.huggingface.co"]),
        "featherless" => Some(&["api.featherless.ai"]),
        "venice" => Some(&["api.venice.ai"]),
        "novita" => Some(&["api.novita.ai"]),
        "synthetic" => Some(&["api.synthetic.new"]),
        "perplexity" => Some(&["api.perplexity.ai"]),
        "deepinfra" => Some(&["api.deepinfra.com"]),
        "google" => Some(&["generativelanguage.googleapis.com"]),
        "local-openai" => Some(&[]),
        _ => None,
    }
}

pub(in crate::providers::adapter) fn provider_base_url_override(
    provider_id: &str,
) -> Option<String> {
    let suffix = provider_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    std::env::var(format!("BRAMA_PROVIDER_{suffix}_BASE_URL"))
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub(in crate::providers::adapter) fn validated_provider_base_url(
    provider_id: &str,
    candidate: &str,
    allow_explicit_loopback: bool,
) -> Result<String, String> {
    if candidate.trim() != candidate {
        return Err(format!(
            "provider `{provider_id}` base URL must not contain surrounding whitespace"
        ));
    }
    let url = reqwest::Url::parse(candidate)
        .map_err(|error| format!("provider `{provider_id}` has an invalid base URL: {error}"))?;
    if url.cannot_be_a_base() || !url.username().is_empty() || url.password().is_some() {
        return Err(format!(
            "provider `{provider_id}` base URL must be an absolute URL without user info"
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(format!(
            "provider `{provider_id}` base URL must not contain a query or fragment"
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| format!("provider `{provider_id}` base URL has no host"))?;
    let trusted = trusted_provider_hosts(provider_id)
        .ok_or_else(|| format!("provider `{provider_id}` has no trusted host policy"))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if loopback {
        if !allow_explicit_loopback || !matches!(url.scheme(), "http" | "https") {
            return Err(format!(
                "provider `{provider_id}` loopback endpoint requires an explicit deployment override"
            ));
        }
    } else {
        if url.scheme() != "https" {
            return Err(format!("provider `{provider_id}` base URL must use HTTPS"));
        }
        if !trusted
            .iter()
            .any(|allowed| host.eq_ignore_ascii_case(allowed))
        {
            return Err(format!(
                "provider `{provider_id}` host `{host}` is not trusted"
            ));
        }
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

pub(in crate::providers::adapter) fn provider_base_url(
    descriptor: &ProviderDescriptor,
) -> Result<String, String> {
    if let Some(configured) = provider_base_url_override(descriptor.id) {
        return validated_provider_base_url(descriptor.id, &configured, true);
    }
    validated_provider_base_url(descriptor.id, descriptor.base_url, false)
}

pub(in crate::providers::adapter) fn provider_base_url_for(
    descriptor: &ProviderDescriptor,
    model_id: &str,
) -> Result<String, String> {
    if descriptor.id != "local-openai" {
        return provider_base_url(descriptor);
    }
    if let Some(configured) = provider_base_url_override(descriptor.id) {
        return validated_provider_base_url(descriptor.id, &configured, true);
    }
    let path = crate::core::inference_routes::configured_path()
        .ok_or_else(|| "BRAMA_INFERENCE_ROUTES_FILE is required for local inference".to_string())?;
    crate::core::inference_routes::base_url(&path, model_id)
}
