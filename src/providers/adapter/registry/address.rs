//! Which URL a request is allowed to leave for.
//!
//! A declaration carries an origin, a deployment may point a provider at its
//! own proxy, and both are checked against the same trusted-host policy before
//! anything is sent.

use super::ProviderDescriptor;

pub(in crate::providers::adapter) fn endpoint(base_url: &str, path: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), path)
}

/// The scheme of a provider address, spelled as the URL spells it: a
/// provider is reached over HTTPS, and a loopback deployment override may
/// also use plain HTTP.
#[derive(serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum WebScheme {
    Http,
    Https,
}

/// The scheme `url` is reached by, refused with the scheme it names when it
/// is neither HTTP nor HTTPS.
fn web_scheme(provider_id: &str, url: &reqwest::Url) -> Result<WebScheme, String> {
    serde_json::from_value(serde_json::Value::String(url.scheme().to_owned())).map_err(|_unknown_scheme| {
        format!(
            "provider `{provider_id}` base URL uses the scheme `{}`; a provider is reached over https, or http on a loopback override",
            url.scheme()
        )
    })
}

/// The address a deployment points `provider_id` at instead of its declared
/// one, `BRAMA_PROVIDER_<ID>_BASE_URL`: `None` when unset or blank, refused
/// by name when it cannot be read, since reading it as unset would send the
/// request past the proxy the deployment named. Surrounding whitespace is
/// kept, so the validation below refuses it.
pub(in crate::providers::adapter) fn provider_base_url_override(
    provider_id: &str,
) -> Result<Option<String>, String> {
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
    Ok(
        crate::environment::declared_text(&format!("BRAMA_PROVIDER_{suffix}_BASE_URL"))?
            .filter(|value| !value.trim().is_empty()),
    )
}

/// `candidate` as the base URL `provider_id` may be sent to. The trusted
/// host is the host of the address the provider is declared with
/// (`declared`, its roster or catalogue entry), so a deployment override may
/// move the path but never the host, except to loopback when
/// `allow_explicit_loopback` says the override is the deployment's own.
pub(in crate::providers::adapter) fn validated_provider_base_url(
    provider_id: &str,
    candidate: &str,
    declared: &str,
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
    let trusted = reqwest::Url::parse(declared)
        .map_err(|error| {
            format!(
                "provider `{provider_id}` is declared with {declared}, which is not a URL: {error}"
            )
        })?
        .host_str()
        .map(str::to_owned)
        .ok_or_else(|| {
            format!("provider `{provider_id}` is declared with {declared}, which names no host")
        })?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    let scheme = web_scheme(provider_id, &url)?;
    if loopback {
        if !allow_explicit_loopback {
            return Err(format!(
                "provider `{provider_id}` loopback endpoint requires an explicit deployment override"
            ));
        }
    } else {
        if scheme != WebScheme::Https {
            return Err(format!("provider `{provider_id}` base URL must use HTTPS"));
        }
        if !host.eq_ignore_ascii_case(&trusted) {
            return Err(format!(
                "provider `{provider_id}` host `{host}` is not trusted: it is declared at `{trusted}`"
            ));
        }
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

pub(in crate::providers::adapter) fn provider_base_url(
    descriptor: &ProviderDescriptor,
) -> Result<String, String> {
    if let Some(configured) = provider_base_url_override(descriptor.id)? {
        return validated_provider_base_url(descriptor.id, &configured, descriptor.base_url, true);
    }
    validated_provider_base_url(
        descriptor.id,
        descriptor.base_url,
        descriptor.base_url,
        false,
    )
}

pub(in crate::providers::adapter) fn provider_base_url_for(
    descriptor: &ProviderDescriptor,
    model_id: &str,
) -> Result<String, String> {
    if descriptor.id != "local-openai" {
        return provider_base_url(descriptor);
    }
    if let Some(configured) = provider_base_url_override(descriptor.id)? {
        return validated_provider_base_url(descriptor.id, &configured, descriptor.base_url, true);
    }
    let path = crate::core::inference_routes::configured_path()
        .ok_or_else(|| "BRAMA_INFERENCE_ROUTES_FILE is required for local inference".to_string())?;
    crate::core::inference_routes::base_url(&path, model_id)
}
