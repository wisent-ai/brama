//! Reading a credential through the plain grant a vault item already carries.
//!
//! The weaker of the two credential paths, and the one the operator's routes
//! table describes: which item and field a resource stands for, and the field
//! read itself. It is separate from capability issuance because a route is a
//! statement an operator wrote and this process only obeys it, while an
//! issuance is a decision the authority makes for one call.

use serde_json::Value;

use super::super::credential_failure;
use super::router::{router_output, router_refusal};
use crate::capability::Secret;
use crate::core::failure;
use wisent_errors::{Code, Failure};

/// The vault coordinate a resource stands for, as Skarbiec resolves it.
///
/// Asked of the router binary through `route resolve`, the same answer the
/// authority gives, so nothing in this process ever decides for itself
/// which credential a purpose means. Until 2026-09-17 this read the
/// operator's routes table file directly, and a subscription whose route
/// Skarbiec declares from the item's own tags - every one an import or a
/// sign-in creates - was answered `no capability route maps resource`
/// here while `stado route capability brama` listed it: the second reader
/// of one table had drifted from the first.
pub(in crate::gateway::broker) async fn capability_route(
    resource: &str,
) -> Result<(String, String), Failure> {
    let output = router_output("resolve capability route", |command| {
        command.arg("route").arg("resolve").arg(resource);
    })
    .await
    .map_err(|error| credential_failure(error, resource, Code::Config))?;
    let document: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        credential_failure(
            format!(
                "route resolve for `{resource}` answered malformed JSON: {error}; stderr: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            resource,
            Code::Config,
        )
    })?;
    let route = document
        .get("routes")
        .and_then(Value::as_array)
        .and_then(|routes| {
            routes
                .iter()
                .find(|route| route.get("resource").and_then(Value::as_str) == Some(resource))
        })
        .ok_or_else(|| {
            credential_failure(
                format!("route resolve answered nothing for resource `{resource}`"),
                resource,
                Code::Config,
            )
        })?;
    if let Some(problem) = route.get("problem").and_then(Value::as_str) {
        return Err(credential_failure(
            format!("no capability route maps resource `{resource}`: {problem}"),
            resource,
            Code::Config,
        ));
    }
    let coordinate = |name: &str| {
        route
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                credential_failure(
                    format!(
                        "capability route for `{resource}` is missing non-empty field `{name}`"
                    ),
                    resource,
                    Code::Config,
                )
            })
    };
    Ok((coordinate("item")?, coordinate("field")?))
}

/// Read one provider credential through the grant the vault already carries.
///
/// Redeeming a capability is the stronger path and stays first. It is not the
/// only one the fleet provisions: some providers are granted as a plain
/// per-field read to a named consumer.
pub(in crate::gateway::broker) async fn credential_by_grant(
    resource: &str,
) -> Result<Secret, Failure> {
    let (item, field) = capability_route(resource).await?;
    let output = router_output("read credential through grant", |command| {
        command.arg("get").arg(&item);
    })
    .await
    .map_err(|error| {
        credential_failure(error, resource, Code::Unknown)
            .with_context("item", item.as_str())
            .with_context("field", field.as_str())
    })?;
    if !output.status.success() {
        return Err(credential_failure(
            router_refusal("read credential through grant", &output),
            resource,
            failure::code_for("credential_unauthorized"),
        )
        .with_context("item", item.as_str())
        .with_context("field", field.as_str()));
    }
    let payload: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        credential_failure(
            format!("credential read for item `{item}` returned malformed JSON: {error}"),
            resource,
            Code::Unknown,
        )
        .with_context("item", item.as_str())
        .with_context("field", field.as_str())
    })?;
    let fields = payload
        .get("fields")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            credential_failure(
                format!("credential document for item `{item}` is missing object field `fields`"),
                resource,
                Code::Unknown,
            )
            .with_context("item", item.as_str())
            .with_context("field", field.as_str())
        })?;
    let value = fields.get(&field).ok_or_else(|| {
        credential_failure(
            format!("credential document for item `{item}` is missing field `{field}`"),
            resource,
            Code::Unknown,
        )
        .with_context("item", item.as_str())
        .with_context("field", field.as_str())
    })?;
    let bytes = match value {
        Value::String(value) => value.as_bytes().to_vec(),
        Value::Null => {
            return Err(credential_failure(
                format!("credential document for item `{item}` has null field `{field}`"),
                resource,
                Code::Unknown,
            )
            .with_context("item", item.as_str())
            .with_context("field", field.as_str()));
        }
        value => serde_json::to_vec(value).map_err(|error| {
            credential_failure(
                format!(
                    "credential field `{field}` for item `{item}` could not be preserved as JSON: {error}"
                ),
                resource,
                Code::Unknown,
            )
            .with_context("item", item.as_str())
            .with_context("field", field.as_str())
        })?,
    };
    Ok(Secret::from_bytes(bytes))
}
