//! The route administration panel: an alias carries exactly one route.
//!
//! Creating, replacing and deleting the alias is proved by real OpenRouter
//! dispatch through the real released binary, and the registry document the
//! panel answers with is read back field by field.

use serde_json::{json, Value};

use super::gateway::{real_provider_credential, Gateway, ALIAS, ROUTE};

/// The second real OpenRouter route the alias is moved to. Replacing an alias
/// rewrites its one route; it does not add another one beside it.
pub(crate) const REPLACEMENT_ROUTE: &str = "openrouter/google/gemini-2.0-flash-001";

fn install_real_credential(gateway: &Gateway) {
    let credential = real_provider_credential("openrouter");
    let (status, installed) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/credentials",
        Some(json!({"provider":"openrouter","credential":credential})),
    );
    assert_eq!(status, 200, "{installed}");
}

#[test]
fn alias_add_edit_and_delete_changes_real_openrouter_dispatch() {
    let gateway = Gateway::start();
    install_real_credential(&gateway);

    let (status, created) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/routes",
        Some(json!({"alias":ALIAS,"primary":ROUTE})),
    );
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["routes"]["routes"][ALIAS], json!(ROUTE));
    let (status, answer) = gateway.completion(ALIAS);
    assert_eq!(status, 200, "{answer}");
    assert!(answer
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty()));

    let (status, edited) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/routes",
        Some(json!({"alias":ALIAS,"primary":REPLACEMENT_ROUTE})),
    );
    assert_eq!(status, 200, "{edited}");
    assert_eq!(edited["routes"]["routes"][ALIAS], json!(REPLACEMENT_ROUTE));
    let (status, answer) = gateway.completion(ALIAS);
    assert_eq!(status, 200, "{answer}");

    let (status, deleted) = gateway.admin(
        reqwest::Method::DELETE,
        "/v1/admin/routes",
        Some(json!({"alias":ALIAS})),
    );
    assert_eq!(status, 200, "{deleted}");
    assert!(
        deleted["routes"]["routes"].get(ALIAS).is_none(),
        "{deleted}"
    );
    let (status, _) = gateway.completion(ALIAS);
    assert_ne!(status, 200);
}

/// The panel accepts an alias and its one route, and nothing else: a body
/// carrying a field the registry no longer has is refused whole, and the
/// alias keeps the route it already had.
#[test]
fn a_route_update_carrying_an_unknown_field_is_refused_whole() {
    let gateway = Gateway::start();
    install_real_credential(&gateway);

    let (status, created) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/routes",
        Some(json!({"alias":ALIAS,"primary":ROUTE})),
    );
    assert_eq!(status, 200, "{created}");

    let (status, refused) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/routes",
        Some(json!({"alias":ALIAS,"primary":REPLACEMENT_ROUTE,"spare_routes":[ROUTE]})),
    );
    assert!(
        !(200..300).contains(&status),
        "an unknown route field was accepted: {status} {refused}"
    );

    let (status, snapshot) = gateway.admin(reqwest::Method::GET, "/v1/admin/snapshot", None);
    assert_eq!(status, 200, "{snapshot}");
    assert_eq!(
        snapshot["routes"]["routes"][ALIAS],
        json!(ROUTE),
        "the refused update changed the registry: {snapshot}"
    );
}
