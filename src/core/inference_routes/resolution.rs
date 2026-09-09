//! Turning what an operator wrote into the one destination a request is sent
//! to: which endpoints are allowed to answer, which deployment a model name
//! names, and what a single alias resolves to.

use std::collections::HashMap;
use std::path::Path;

use super::document::{read, Deployment, Registry};

pub(super) fn safe_inference_host(value: &str) -> bool {
    let Ok(address) = value.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    if address.is_loopback() {
        return true;
    }
    let octets = address.octets();
    let first = "100".parse::<u8>().expect("static Tailscale prefix");
    let lower = "64".parse::<u8>().expect("static Tailscale range");
    let upper = "128".parse::<u8>().expect("static Tailscale range");
    octets[usize::MIN] == first && (lower..upper).contains(&octets[usize::from(true)])
}

fn deployment_for_model<'a>(registry: &'a Registry, model: &str) -> Result<&'a Deployment, String> {
    let mut matches = registry.deployments.iter().filter(|deployment| {
        deployment.name == model
            || deployment
                .adapters
                .iter()
                .any(|adapter| adapter.name == model)
    });
    let deployment = matches
        .next()
        .ok_or_else(|| format!("unknown local inference model '{model}'"))?;
    if matches.next().is_some() {
        return Err(format!("ambiguous local inference model '{model}'"));
    }
    Ok(deployment)
}

/// A destination naming `best` is delegation, not a local deployment: the
/// operator is saying "whatever the subscription route picks", so it passes
/// through untouched and subscription dispatch resolves it per caller identity.
///
/// Without this, `best` carries no slash, reaches the deployment lookup below
/// and is rejected as an unknown local model. That forced every route to name
/// one fixed provider and model, and left no way to point an alias at the
/// subscription that pays.
pub(super) fn resolved_destination(registry: &Registry, destination: &str) -> Result<String, String> {
    if destination == crate::core::server::BEST_ALIAS {
        return Ok(destination.to_string());
    }
    if destination.contains('/') {
        return Ok(destination.to_string());
    }
    let deployment = deployment_for_model(registry, destination)?;
    if !safe_inference_host(&deployment.endpoint.host) || deployment.endpoint.port == u16::MIN {
        return Err(format!(
            "inference deployment '{destination}' has no safe local or Tailscale endpoint"
        ));
    }
    Ok(format!("local-openai/{destination}"))
}

pub fn resolved(path: &Path) -> Result<HashMap<String, String>, String> {
    let registry = read(path)?;
    let mut routes = HashMap::new();
    for (alias, destination) in &registry.routes {
        routes.insert(alias.clone(), resolved_destination(&registry, destination)?);
    }
    Ok(routes)
}

pub fn base_url(path: &Path, model_name: &str) -> Result<String, String> {
    let registry = read(path)?;
    let deployment = deployment_for_model(&registry, model_name)?;
    if !safe_inference_host(&deployment.endpoint.host) || deployment.endpoint.port == u16::MIN {
        return Err(format!(
            "inference model '{model_name}' has no safe local or Tailscale endpoint"
        ));
    }
    Ok(format!(
        "http://{}:{}",
        deployment.endpoint.host, deployment.endpoint.port
    ))
}

pub fn validate(path: &Path) -> Result<(), String> {
    resolved(path).map(|_| ())
}

pub fn resolve(path: &Path, alias: &str) -> Result<Option<String>, String> {
    Ok(resolved(path)?.remove(alias))
}
