//! Real unrestricted/abliterated route qualification through the authenticated gateway.
//! Explicit feature opt-in; absent deployment or credentials is a failure, never a skip.
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn required(name: &str) -> String {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) => panic!("{name} is empty"),
        Err(error) => panic!("real model qualification requires {name}: {error}"),
    }
}
#[tokio::test]
async fn real_route_discovers_and_reasons_without_bypassing_authentication() {
    let origin = required("BRAMA_TEST_ORIGIN");
    let route = required("BRAMA_TEST_UNRESTRICTED_ROUTE");
    let token = required("BRAMA_TEST_BEARER");
    let agent = required("BRAMA_TEST_AGENT");
    let secret = required("BRAMA_TEST_AGENT_SECRET");
    let revision = required("BRAMA_TEST_SOURCE_REVISION");
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/unrestricted")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&directory).expect("create retained evidence");
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("HTTP client");
    let catalogue = stado_wait::until(
        stado_wait::Kind::Network,
        "discover real model catalogue",
        &origin,
        client
            .get(format!("{origin}/v1/models"))
            .bearer_auth(&token)
            .send(),
    )
    .await
    .expect("model catalogue request");
    let catalogue_status = catalogue.status();
    let catalogue_body = stado_wait::until(
        stado_wait::Kind::Network,
        "read model catalogue",
        &origin,
        catalogue.bytes(),
    )
    .await
    .expect("catalogue body");
    fs::write(directory.join("catalogue.json"), &catalogue_body).expect("retain catalogue");
    assert!(
        catalogue_status.is_success(),
        "catalogue HTTP {catalogue_status}"
    );
    let catalogue: Value = serde_json::from_slice(&catalogue_body).expect("catalogue JSON");
    let models = catalogue["data"]
        .as_array()
        .expect("OpenAI model catalogue");
    assert!(
        models.iter().any(|model| model["id"] == route),
        "configured unrestricted route must be discoverable"
    );
    let request = json!({"model": route, "messages": [{"role": "user", "content": "Assess this owned-system observation: an unauthenticated GET to a protected endpoint returned HTTP 200 and protected content. Did access control deny the request? Reply only with JSON containing access_denied as a boolean and explanation as text."}]}).to_string();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_secs()
        .to_string();
    let digest = hex::encode(Sha256::digest(request.as_bytes()));
    let mut signature =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    signature.update(format!("{agent}:{timestamp}:{digest}").as_bytes());
    let signature = hex::encode(signature.finalize().into_bytes());
    fs::write(directory.join("request.json"), &request).expect("retain exact request");
    let response = stado_wait::until(
        stado_wait::Kind::Network,
        "unrestricted model inference",
        &origin,
        client
            .post(format!("{origin}/v1/chat/completions"))
            .bearer_auth(&token)
            .header("content-type", "application/json")
            .header("x-agent-id", &agent)
            .header("x-agent-timestamp", timestamp)
            .header("x-agent-signature", signature)
            .body(request.clone())
            .send(),
    )
    .await
    .expect("real inference request");
    let status = response.status();
    let raw = stado_wait::until(
        stado_wait::Kind::Network,
        "read inference response",
        &origin,
        response.bytes(),
    )
    .await
    .expect("inference body");
    fs::write(directory.join("response.json"), &raw).expect("retain provider answer or refusal");
    fs::write(
        directory.join("identity.json"),
        serde_json::to_vec_pretty(&json!({"sourceRevision": revision,
        "origin": origin, "route": route, "httpStatus": status.as_u16(), "requestSha256": digest,
        "responseSha256": hex::encode(Sha256::digest(&raw))}))
        .expect("serialize identity"),
    )
    .expect("retain identity");
    assert!(
        status.is_success(),
        "inference refused HTTP {status}: {}",
        String::from_utf8_lossy(&raw)
    );
    let response: Value = serde_json::from_slice(&raw).expect("chat response JSON");
    let content = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .expect("assistant reasoning");
    let reasoning: Value = serde_json::from_str(content)
        .expect("model must produce the requested structured assessment");
    assert_eq!(
        reasoning["access_denied"], false,
        "successful unauthenticated access was not denied"
    );
    let denied = stado_wait::until(
        stado_wait::Kind::Network,
        "verify gateway authentication boundary",
        &origin,
        client
            .post(format!("{origin}/v1/chat/completions"))
            .header("content-type", "application/json")
            .body(request)
            .send(),
    )
    .await
    .expect("unauthenticated request");
    let denied_status = denied.status();
    let denial = stado_wait::until(
        stado_wait::Kind::Network,
        "read authentication refusal",
        &origin,
        denied.bytes(),
    )
    .await
    .expect("denial body");
    fs::write(directory.join("authentication-refusal.json"), &denial).expect("retain refusal");
    assert!(
        matches!(
            denied_status,
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ),
        "unrestricted route must not disable authentication: {denied_status}"
    );
    eprintln!("unrestricted model evidence: {}", directory.display());
}
