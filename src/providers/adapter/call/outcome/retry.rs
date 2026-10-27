//! When a second send is the same request rather than a second one.

/// Send once more when the first attempt never reached the provider.
///
/// A `reqwest` error that is a connect failure, or a request-phase failure that
/// produced no response, means the provider never saw the request: nothing was
/// generated, nothing was billed, and sending it again is the same request
/// rather than a second one. Without this, that class of failure reaches a
/// person as "Assistant response failed. Please try again." while the
/// gateway's own record says `attempts=1` and `retryable=true` — a retry the
/// envelope promises with no implementation behind it. Brama's client sets no
/// deadline, so no send ends for want of time; a failure after the provider
/// answered is a response, not an error here, and is never sent again. Each
/// send is announced while it waits.
pub(in crate::providers::adapter) async fn send_once_more_if_unsent(
    builder: reqwest::RequestBuilder,
) -> Result<reqwest::Response, reqwest::Error> {
    let Some(retry) = builder.try_clone() else {
        return stado_wait::request(builder).await;
    };
    match stado_wait::request(builder).await {
        Err(error) if error.is_connect() || error.is_request() => stado_wait::request(retry).await,
        result => result,
    }
}
