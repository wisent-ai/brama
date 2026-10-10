use super::super::remote;
use serde_json::Value;
/// Run one provider's refresh on the gateway that holds its credentials.
pub(super) async fn on_gateway(
    destination: remote::Destination,
    provider: &str,
    reason: &str,
) -> Result<Value, String> {
    let (origin, bearer) = destination.resolve_reading_stdin().await?;
    let origin = origin.ok_or_else(|| {
        String::from("name --gateway or --gateway-consumer to refresh another gateway's pool")
    })?;
    remote::refresh(&origin, bearer.trim(), provider, reason).await
}
