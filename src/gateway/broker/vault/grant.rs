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

/// The vault coordinate a resource stands for, as the operator wrote it.
///
/// The same table the authority consults, read here so nothing in this process
/// ever decides for itself which credential a purpose means.
pub(in crate::gateway::broker) fn capability_route(
    resource: &str,
) -> Result<(String, String), Failure> {
    let path = std::env::var_os("SKARBIEC_CAPABILITY_ROUTES_FILE").ok_or_else(|| {
        credential_failure(
            "SKARBIEC_CAPABILITY_ROUTES_FILE is not configured",
            resource,
            Code::Config,
        )
    })?;
    let raw = std::fs::read_to_string(&path).map_err(|error| {
        credential_failure(
            format!(
                "read capability routes file `{}`: {error}",
                path.to_string_lossy()
            ),
            resource,
            Code::Config,
        )
    })?;
    let document: Value = serde_json::from_str(&raw).map_err(|error| {
        credential_failure(
            format!(
                "capability routes file `{}` contains malformed JSON: {error}",
                path.to_string_lossy()
            ),
            resource,
            Code::Config,
        )
    })?;
    let table = document.get("routes").unwrap_or(&document);
    let entry = table.get(resource).ok_or_else(|| {
        credential_failure(
            format!("no capability route maps resource `{resource}`"),
            resource,
            Code::Config,
        )
    })?;
    let item = entry
        .get("item")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            credential_failure(
                format!("capability route for `{resource}` is missing non-empty field `item`"),
                resource,
                Code::Config,
            )
        })?
        .to_owned();
    let field = entry
        .get("field")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            credential_failure(
                format!("capability route for `{resource}` is missing non-empty field `field`"),
                resource,
                Code::Config,
            )
        })?
        .to_owned();
    Ok((item, field))
}

/// Read one provider credential through the grant the vault already carries.
///
/// Redeeming a capability is the stronger path and stays first. It is not the
/// only one the fleet provisions: some providers are granted as a plain
/// per-field read to a named consumer.
pub(in crate::gateway::broker) async fn credential_by_grant(
    resource: &str,
) -> Result<Secret, Failure> {
    let (item, field) = capability_route(resource)?;
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
