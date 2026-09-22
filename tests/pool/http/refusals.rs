//! What the pool refuses, and to whom: a caller that proved no identity at
//! all, one that names an owner it did not prove, and a write naming a member
//! or an action the deployment does not have.

use reqwest::Method;
use serde_json::json;

use crate::gateway::{refusal, Gateway, AGENT, OTHER_AGENT, POOL};
use crate::VAULT;

/// A caller that proved nothing is told nothing, and the sentence it is told
/// is the one the gateway's own envelope carries.
#[test]
fn the_pool_refuses_a_caller_that_proved_no_identity() {
    let gateway = Gateway::start("pool-unproven", VAULT);
    let (status, body) = gateway.request(POOL, Method::GET, None, None, None);
    assert_eq!(status, 401, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "unauthenticated".to_owned(),
            "authentication_error".to_owned(),
            "unauthorized".to_owned(),
        ),
    );

    let (status, body) = gateway.request(POOL, Method::GET, Some(STRANGER_BEARER), None, None);
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "forbidden".to_owned(),
            "authorization_error".to_owned(),
            "forbidden".to_owned(),
        ),
    );
}

/// Ownership is read off the proof. An agent naming an owner is refused
/// rather than obeyed, and an account it does not own is not found for it even
/// though the deployment holds it.
#[test]
fn the_pool_refuses_a_caller_that_names_an_owner_it_did_not_prove() {
    let gateway = Gateway::start("pool-wrong-owner", VAULT);
    let (status, body) = gateway.agent(
        POOL,
        Method::POST,
        Some(&json!({
            "action": "retire",
            "agent_id": OTHER_AGENT,
            "subscription_id": "pool-other-openai",
        })),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "invalid_request".to_owned(),
            "request_error".to_owned(),
            "agent_id is derived from the proven identity and must not be sent".to_owned(),
        ),
    );

    let (status, body) = gateway.agent(
        POOL,
        Method::POST,
        Some(&json!({"action": "retire", "subscription_id": "pool-other-openai"})),
    );
    assert_eq!(status, 404, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "subscription_not_found".to_owned(),
            "state_error".to_owned(),
            "subscription not found".to_owned(),
        ),
    );

    // A signature by another agent than the bearer is bound to contradicts
    // itself, and a contradiction between two proofs is never resolved in
    // favour of either.
    let (status, body) = gateway.request(
        POOL,
        Method::GET,
        Some(gateway::AGENT_BEARER),
        None,
        Some((OTHER_AGENT, AGENT_SIGNING_SECRET)),
    );
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "forbidden".to_owned(),
            "authorization_error".to_owned(),
            "forbidden".to_owned(),
        ),
    );
}

/// A subscription nothing owns cannot be retired, and a write that names no
/// recognised action is refused before anything is written.
#[test]
fn the_pool_refuses_an_unknown_subscription_and_an_unknown_action() {
    let gateway = Gateway::start("pool-unknown", VAULT);
    let (status, body) = gateway.agent(
        POOL,
        Method::POST,
        Some(&json!({"action": "retire", "subscription_id": "pool-nothing-owns-this"})),
    );
    assert_eq!(status, 404, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "subscription_not_found".to_owned(),
            "state_error".to_owned(),
            "subscription not found".to_owned(),
        ),
    );

    let (status, body) = gateway.agent(POOL, Method::POST, Some(&json!({"action": "borrow"})));
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        refusal(&body).2,
        "action must be \"bank\" or \"retire\"".to_owned(),
    );

    // The console is the only caller that may name an owner, and it must.
    let (status, body) = gateway.console(POOL, Method::POST, Some(&json!({"action": "retire"})));
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        refusal(&body).2,
        "agent_id names the agent whose pool is written and is required for a \
         deployment-scoped write"
            .to_owned(),
    );

    assert!(
        !gateway.root().join("donated.json").exists(),
        "a refused write must not create the donated-subscription overlay"
    );
}
