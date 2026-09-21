//! What `GET /v1/models` answers now, and what `GET /v1/categories` reports.
//!
//! One real `brama serve` process over one real isolated Skarbiec vault. No
//! story here reaches a provider: every answer is read out of the public
//! catalogue and the operator's own registry, which is where these facets
//! come from.

use reqwest::Method;
use serde_json::{json, Value};

use crate::gateway::{Gateway, AGENT, CONSOLE_BEARER};

/// The pool this gateway routes over. These stories never spend it; the
/// gateway needs a vault to start over.
pub(crate) const ACCOUNTS: &[(&str, &str, &str)] = &[(AGENT, "codex", "catalogue-codex")];

pub(crate) fn read(gateway: &Gateway, path: &str) -> (u16, Value) {
    gateway.request(path, Method::GET, Some(CONSOLE_BEARER), None, None)
}

pub(crate) fn post(gateway: &Gateway, path: &str, body: &Value) -> (u16, Value) {
    gateway.request(path, Method::POST, Some(CONSOLE_BEARER), Some(body), None)
}

/// The catalogue answers what a model produces and how its weights were
/// published, and it narrows on both.
#[test]
fn the_model_list_answers_and_narrows_on_the_new_facets() {
    let gateway = Gateway::start("catalogue-facets", ACCOUNTS);
    let (status, body) = read(&gateway, "/v1/models?limit=5");
    assert_eq!(status, 400, "an unknown query member is refused: {body}");

    let (status, body) = read(&gateway, "/v1/models?kind=image");
    assert_eq!(status, 200, "{body}");
    let models = body["data"].as_array().expect("an OpenAI-shaped list");
    assert!(!models.is_empty(), "image models are listed: {body}");
    for model in models {
        assert_eq!(model["kind"], "image", "{model}");
    }

    let (status, body) = read(&gateway, "/v1/models?weights=open&kind=text");
    assert_eq!(status, 200, "{body}");
    for model in body["data"].as_array().expect("a list") {
        assert_eq!(model["open_weights"], true, "{model}");
        assert_eq!(model["kind"], "text", "{model}");
    }

    for (query, sentence) in [
        ("kind=hologram", "kind must be text, image, video or audio"),
        ("weights=maybe", "weights must be open or closed"),
        (
            "category=Uncensored",
            "category must be lowercase letters, digits and hyphens",
        ),
    ] {
        let (status, body) = read(&gateway, &format!("/v1/models?{query}"));
        assert_eq!(status, 400, "{query} must be refused: {body}");
        assert_eq!(body["error"]["message"], sentence, "{body}");
    }
}

/// A declared category reaches both the category listing and the model list,
/// and it is the registry that decides membership.
#[test]
fn a_declared_category_reaches_the_catalogue() {
    let gateway = Gateway::start("catalogue-categories", ACCOUNTS);
    let (status, body) = read(&gateway, "/v1/categories");
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["categories"].as_array().map(Vec::len),
        Some(0),
        "a gateway declares no categories until the operator does: {body}"
    );

    let (status, body) = gateway.console(
        "/v1/admin/categories",
        Method::PUT,
        Some(&json!({"category": "uncensored", "terms": ["uncensored"]})),
    );
    assert_eq!(status, 200, "the console declares a category: {body}");

    let (status, body) = read(&gateway, "/v1/categories");
    assert_eq!(status, 200, "{body}");
    let declared = body["categories"].as_array().expect("a category array");
    let uncensored = declared
        .iter()
        .find(|entry| entry["category"] == "uncensored")
        .expect("the declared category is reported");
    assert!(
        uncensored["models"].as_u64().unwrap_or_default() > 0,
        "the declaration holds catalogue models: {uncensored}"
    );

    let (status, body) = read(&gateway, "/v1/models?category=uncensored");
    assert_eq!(status, 200, "{body}");
    let models = body["data"].as_array().expect("a list");
    assert!(!models.is_empty(), "the category narrows the list: {body}");
    for model in models {
        assert!(
            model["categories"]
                .as_array()
                .is_some_and(|names| names.iter().any(|name| name == "uncensored")),
            "{model}"
        );
    }

    let (status, body) = gateway.console(
        "/v1/admin/categories",
        Method::DELETE,
        Some(&json!({"category": "nobody-declared-this"})),
    );
    assert_eq!(status, 404, "{body}");
    assert_eq!(
        body["error"]["message"], "no model category 'nobody-declared-this' is declared",
        "{body}"
    );
}
