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
        if let Ok(response) = request.send().await {
            if response.status().is_success() {
                if let Ok(body) = response.json::<Value>().await {
                    let dynamic = body
                        .get("data")
                        .or_else(|| body.get("models"))
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    models.extend(
                        dynamic
                            .iter()
                            .filter_map(|row| catalog_model_from_value(provider_id, row)),
                    );
                }
            }
        }
        models.sort_by(|left, right| left.route_id.cmp(&right.route_id));
        models.dedup_by(|left, right| left.route_id == right.route_id);
        if models.is_empty() {
            return Err(format!("provider `{provider_id}` has no catalog models"));
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
    let dynamic = match request.send().await {
        Ok(response) if response.status().is_success() => response
            .json::<Value>()
            .await
            .ok()
            .and_then(|body| {
                body.get("data")
                    .or_else(|| body.get("models"))
                    .and_then(Value::as_array)
                    .cloned()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
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
        return Err(format!(
            "provider `{provider_id}` returned no models and this build carries no substitute \
             model list for it"
        ));
    }
    Ok(models)
}
