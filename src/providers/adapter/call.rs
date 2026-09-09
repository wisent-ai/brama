//! The shared HTTP clients one provider call is made through.

pub(in crate::providers::adapter) mod credential;
pub(in crate::providers::adapter) mod refusal;
pub(in crate::providers::adapter) mod response_body;
pub(in crate::providers::adapter) mod retry;
pub(in crate::providers::adapter) mod typed_capability;

use reqwest::Client;

/// Two clients for this process, not one per request.
///
/// Every `reqwest::Client` owns its own connection pool, so building one per
/// call opens a fresh socket for each request and holds that pool until the
/// client is dropped. Under ordinary traffic the descriptors then accumulate
/// faster than they are reclaimed, and the listener eventually stops being able
/// to accept at all -- `Too many open files` -- while the process stays up and
/// keeps looking healthy. reqwest is built to have one client reused; it is
/// reference-counted inside, so handing out clones costs nothing.
///
/// None of these clients states how long a pooled connection may sit unused;
/// whatever the HTTP client library does by default is what applies.
static CONTROL_CLIENT: std::sync::LazyLock<Result<Client, String>> =
    std::sync::LazyLock::new(|| build_shared_client("20"));
static DISPATCH_CLIENT: std::sync::LazyLock<Result<Client, String>> =
    std::sync::LazyLock::new(|| build_shared_client("255"));

fn build_shared_client(seconds: &str) -> Result<Client, String> {
    Client::builder()
        .timeout(std::time::Duration::from_secs(
            seconds.parse().expect("static number"),
        ))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())
}

/// Catalogue and provider control calls, which are expected to answer quickly.
pub fn control_client() -> Result<Client, String> {
    CONTROL_CLIENT.clone()
}

/// Model dispatch, which waits as long as a generation legitimately takes.
pub(in crate::providers::adapter) fn dispatch_client() -> Result<Client, String> {
    DISPATCH_CLIENT.clone()
}

/// Streaming dispatch, which cannot carry a whole-body deadline.
///
/// `Client::builder().timeout` bounds the response *body* read too, which
/// would cut every generation that runs longer than the budget while
/// legitimately producing. The stream therefore gets no total budget; the
/// pump enforces the same 255 seconds between reads instead, which is where
/// "the provider stopped answering" is actually measurable.
static STREAM_CLIENT: std::sync::LazyLock<Result<Client, String>> =
    std::sync::LazyLock::new(|| {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|error| error.to_string())
    });

pub(in crate::providers::adapter) fn stream_client() -> Result<Client, String> {
    STREAM_CLIENT.clone()
}
