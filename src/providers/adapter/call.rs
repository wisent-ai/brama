//! The shared HTTP clients one provider call is made through.

pub(in crate::providers::adapter) mod credential;
pub(in crate::providers::adapter) mod media;
pub(in crate::providers::adapter) mod outcome;

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
/// None of these clients states how long a pooled connection may sit unused,
/// and none of them carries a deadline: what ends a call is the provider
/// answering or the connection failing. `reqwest`'s asynchronous client sets
/// none of its own, so saying nothing here is saying it.
static CONTROL_CLIENT: std::sync::LazyLock<Result<Client, String>> =
    std::sync::LazyLock::new(build_shared_client);
static DISPATCH_CLIENT: std::sync::LazyLock<Result<Client, String>> =
    std::sync::LazyLock::new(build_shared_client);

fn build_shared_client() -> Result<Client, String> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())
}

/// Catalogue and provider control calls.
pub fn control_client() -> Result<Client, String> {
    CONTROL_CLIENT.clone()
}

/// Model dispatch, which waits as long as a generation legitimately takes.
pub(in crate::providers::adapter) fn dispatch_client() -> Result<Client, String> {
    DISPATCH_CLIENT.clone()
}

/// Streaming dispatch. Like the two above it, the client carries no deadline:
/// `reqwest` would otherwise apply its own to the body read and cut a
/// generation that is legitimately still producing.
static STREAM_CLIENT: std::sync::LazyLock<Result<Client, String>> =
    std::sync::LazyLock::new(build_shared_client);

pub(in crate::providers::adapter) fn stream_client() -> Result<Client, String> {
    STREAM_CLIENT.clone()
}
