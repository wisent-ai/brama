//! The selected gateway owns both stored pool reports and provider usage refreshes.
use serde_json::Value;

pub(crate) async fn report(
    gateway: &str,
    bearer: &str,
    refresh_usage: bool,
) -> Result<Value, String> {
    let (method, path) = if refresh_usage {
        (reqwest::Method::POST, "/v1/plan-usage")
    } else {
        (reqwest::Method::GET, "/v1/subscription-pool")
    };
    let endpoint = format!("{}{path}", gateway.trim_end_matches('/'));
    let operation = format!("{method} {endpoint}");
    // The proven identity selects scope; plan usage explicitly requires an empty body.
    let request = super::client()?
        .request(method, &endpoint)
        .bearer_auth(bearer);
    let response = stado_wait::until(
        stado_wait::Kind::Network,
        &operation,
        &endpoint,
        request.send(),
    )
    .await
    .map_err(|error| format!("{operation}: {error}"))?;
    let status = response.status();
    let body = stado_wait::until(
        stado_wait::Kind::Network,
        "read gateway pool response",
        &endpoint,
        response.text(),
    )
    .await
    .map_err(|error| format!("{operation}: HTTP {status}: reading response: {error}"))?;
    if !status.is_success() {
        return Err(format!("{operation}: HTTP {status}: {body}"));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("{operation}: HTTP {status}: invalid pool JSON: {error}"))
}
