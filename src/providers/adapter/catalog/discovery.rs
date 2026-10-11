//! Which models one provider says it serves, asked of the provider itself and
//! read into this gateway-s own shape.

use serde_json::{json, Value};

use crate::subscription_dispatch::model_catalog;

use super::super::call::credential::{
    authorize_catalog, authorize_provider, provider_credential_key,
};
use super::super::registry::{
    apply_omp_model_metadata, endpoint, model_from_value, provider_base_url,
};
use super::super::{control_client, credential_key, provider, RegistryModel};
use super::endpoint::{catalog_endpoint, catalog_provider_base_url};
use super::model_row::catalog_model_from_value;

pub async fn discover_models(
    provider_id: &str,
    item: &str,
    secret: &str,
) -> Result<Vec<RegistryModel>, String> {
    let catalog = model_catalog::snapshot().await?;
    if let Some(descriptor) = catalog.providers.get(provider_id) {
        if !descriptor.executable() {
            return Err(format!(
                "provider `{provider_id}` uses a protocol not implemented by Brama"
            ));
        }
        let mut models = catalog
            .models
            .iter()
            .filter(|model| model.provider_id == provider_id)
            .cloned()
            .collect::<Vec<_>>();
        let key = credential_key(item, secret)?;
        let base_url = catalog_provider_base_url(descriptor)?;
        let client = control_client()?;
        let request = authorize_catalog(
            client.get(catalog_endpoint(&base_url, "/models")),
            descriptor,
            &key,
        );
        let listing = listed_rows(provider_id, request).await;
        if let Ok(dynamic) = &listing {
            models.extend(
                dynamic
                    .iter()
                    .filter_map(|row| catalog_model_from_value(provider_id, row)),
            );
        }
        models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
        models.dedup_by(|left, right| left.route_id == right.route_id);
        if models.is_empty() {
            return Err(match listing {
                Err(cause) => format!("provider `{provider_id}` has no catalog models: {cause}"),
                Ok(_) => format!("provider `{provider_id}` has no catalog models"),
            });
        }
        return Ok(models);
    }

    let descriptor = provider(provider_id)
        .ok_or_else(|| format!("provider `{provider_id}` is not in the Wisent registry"))?;
    let key = provider_credential_key(descriptor, item, secret)?;
    let base_url = provider_base_url(descriptor)?;
    let client = control_client()?;
    let request = authorize_provider(
        client.get(endpoint(&base_url, descriptor.models_path)),
        descriptor,
        &key,
        secret,
    );
    let listing = listed_rows(provider_id, request).await;
    let dynamic = match &listing {
        Ok(rows) => rows.as_slice(),
        Err(_) => &[],
    };
    let mut models = dynamic
        .iter()
        .filter_map(|row| model_from_value(descriptor, row))
        .collect::<Vec<_>>();
    models.extend(
        descriptor
            .static_models
            .iter()
            .filter_map(|id| model_from_value(descriptor, &json!({"id": id}))),
    );
    models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    models.dedup_by(|left, right| left.route_id == right.route_id);
    apply_omp_model_metadata(provider_id, &mut models);
    if models.is_empty() {
        let cause = match listing {
            Err(cause) => format!(": {cause}"),
            Ok(_) => String::new(),
        };
        return Err(format!(
            "provider `{provider_id}` returned no models and this build carries no substitute \
             model list for it{cause}"
        ));
    }
    Ok(models)
}

/// The model rows a provider's listing endpoint answered, or why it gave
/// none: unreachable, refused with its body, not JSON, or carrying neither
/// `data` nor `models`. A listing failure used to be dropped silently, so a
/// provider left with no models was reported without the reason.
async fn listed_rows(
    provider_id: &str,
    request: reqwest::RequestBuilder,
) -> Result<Vec<Value>, String> {
    let response = stado_wait::until(
        stado_wait::Kind::Network,
        "list provider models",
        provider_id,
        request.send(),
    )
    .await
    .map_err(|error| {
        format!(
            "the model listing could not be reached: {}",
            error.without_url()
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        let body = match response.text().await {
            Ok(text) => text,
            Err(error) => format!("(the body could not be read: {})", error.without_url()),
        };
        return Err(format!("the model listing answered {status}: {body}"));
    }
    let body: Value = response.json().await.map_err(|error| {
        format!(
            "the model listing is not readable JSON: {}",
            error.without_url()
        )
    })?;
    match body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array)
    {
        Some(rows) => Ok(rows.clone()),
        None => Err("the model listing carries neither `data` nor `models`".to_string()),
    }
}
