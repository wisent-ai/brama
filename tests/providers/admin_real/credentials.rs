//! The provider credential panel, proved by real OpenRouter dispatch.
//!
//! Installing, replacing and deleting the credential is observed through the
//! canonical route, so a deleted credential is read as the refusal a caller
//! would receive, never as a quietly served answer from somewhere else.

use serde_json::json;

use super::gateway::{real_provider_credential, Gateway, ROUTE};

#[test]
fn key_add_replace_and_delete_changes_real_openrouter_dispatch() {
    let credential = real_provider_credential("openrouter");
    let gateway = Gateway::start();
    let (status, created) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/credentials",
        Some(json!({"provider":"openrouter","credential":credential})),
    );
    assert_eq!(status, 200, "{created}");
    let (status, answer) = gateway.completion(ROUTE);
    assert_eq!(status, 200, "{answer}");

    let credential = real_provider_credential("openrouter");
    let (status, replaced) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/credentials",
        Some(json!({"provider":"openrouter","credential":credential})),
    );
    assert_eq!(status, 200, "{replaced}");
    let (status, answer) = gateway.completion(ROUTE);
    assert_eq!(status, 200, "{answer}");

    let (status, deleted) = gateway.admin(
        reqwest::Method::DELETE,
        "/v1/admin/credentials",
        Some(json!({"provider":"openrouter"})),
    );
    assert_eq!(status, 200, "{deleted}");
    let (status, _) = gateway.completion(ROUTE);
    assert_ne!(status, 200);
}
