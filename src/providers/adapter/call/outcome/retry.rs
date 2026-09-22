//! When a second send is the same request rather than a second one.

/// Send once more when the first attempt never reached the provider.
///
/// A `reqwest` error that is a connect failure, or a request-phase failure that
/// produced no response, means the provider never saw the request: nothing was
/// generated, nothing was billed, and sending it again is the same request
/// rather than a second one. On 2026-09-05 that class of failure reached a
/// person as "Assistant response failed. Please try again." while the gateway's
/// own record said `attempts=1` and `retryable=true` — the retry the envelope
/// promised had no implementation behind it.
///
/// A timeout is deliberately NOT retried. The provider may have accepted that
/// request and be generating against it, and a duplicate would bill twice and
/// could answer twice.
pub(in crate::providers::adapter) async fn send_once_more_if_unsent(
    builder: reqwest::RequestBuilder,
) -> Result<reqwest::Response, reqwest::Error> {
    let Some(retry) = builder.try_clone() else {
        return builder.send().await;
    };
    match builder.send().await {
        Err(error) if !error.is_timeout() && (error.is_connect() || error.is_request()) => {
            retry.send().await
        }
        result => result,
    }
}
