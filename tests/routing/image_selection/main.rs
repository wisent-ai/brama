//! Qualify real buffered and streaming image requests through Brama.
//!
//! Run: `cargo test --test image_selection_real -- --ignored --nocapture`
//! with the existing `BRAMA_URL`, `BRAMA_TOKEN`, `WISENT_APP_AGENT_ID` and
//! `WISENT_APP_AGENT_AUTH_SECRET` of a workload allowed to request `best`, and
//! `BRAMA_REAL_EXPECTED_REVISION` naming the serving revision being
//! qualified. The two image requests spend existing subscription quota; no
//! account, grant, route or billing configuration is changed. Missing access
//! fails, never skips. The evidence lands in
//! `.wisent-output/image-selection-real/<uuid>/report.json`.

use std::path::PathBuf;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The heading the fixture page carries, as the model must read it back.
const EXPECTED_HEADING: &str = "measuring current figma components";
/// The completion budget each image request is given.
const MAX_TOKENS: u32 = 128;

struct Gateway {
    origin: String,
    token: String,
    agent: String,
    secret: String,
    client: reqwest::blocking::Client,
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required; this test fails rather than skips"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

impl Gateway {
    fn http(&self, path: &str, payload: Option<&Value>) -> (u16, String) {
        let body = payload.map(|value| serde_json::to_vec(value).expect("payload serialises")).unwrap_or_default();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_secs()
            .to_string();
        let body_hash = hex::encode(Sha256::digest(&body));
        let mut mac = Hmac::<Sha256>::new_from_slice(self.secret.as_bytes()).expect("hmac key");
        mac.update(format!("{}:{timestamp}:{body_hash}", self.agent).as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        let url = format!("{}{path}", self.origin);
        let request = if payload.is_some() { self.client.post(url).body(body) } else { self.client.get(url) };
        let response = request
            .bearer_auth(&self.token)
            .header("Content-Type", "application/json")
            .header("X-Agent-Id", &self.agent)
            .header("X-Agent-Timestamp", timestamp)
            .header("X-Agent-Body-Sha256", body_hash)
            .header("X-Agent-Signature", signature)
            .send()
            .expect("the gateway answers");
        let status = response.status().as_u16();
        (status, response.text().unwrap_or_default())
    }

    fn completion(&self, image_url: &str, stream: bool, model: &str) -> (u16, String) {
        let payload = json!({
            "model": model,
            "stream": stream,
            "max_tokens": MAX_TOKENS,
            "messages": [{ "role": "user", "content": [
                { "type": "text", "text": "Read the large main heading in the attached image. Return only that heading." },
                { "type": "image_url", "image_url": { "url": image_url } },
            ]}],
        });
        self.http("/v1/chat/completions", Some(&payload))
    }
}

fn normalized(text: &str) -> String {
    text.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn streamed_text(raw: &str) -> Result<String, String> {
    let mut text = String::new();
    let mut finished = false;
    for line in raw.lines() {
        let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
        if data == "[DONE]" {
            finished = true;
            continue;
        }
        let event: Value = serde_json::from_str(data).map_err(|error| format!("bad event {data}: {error}"))?;
        for choice in event.get("choices").and_then(Value::as_array).into_iter().flatten() {
            if let Some(content) = choice.pointer("/delta/content").and_then(Value::as_str) {
                text.push_str(content);
            }
        }
    }
    if finished { Ok(text) } else { Err("the gateway did not complete its event stream".into()) }
}

#[test]
#[ignore = "spends real subscription quota; run with --ignored against the gateway being qualified"]
fn image_selection_real() {
    let gateway = Gateway {
        origin: required("BRAMA_URL").trim_end_matches('/').to_string(),
        token: required("BRAMA_TOKEN"),
        agent: required("WISENT_APP_AGENT_ID"),
        secret: required("WISENT_APP_AGENT_AUTH_SECRET"),
        client: reqwest::blocking::Client::new(),
    };
    let expected = required("BRAMA_REAL_EXPECTED_REVISION");
    let image = std::fs::read(root().join("tests/routing/fixtures/page.png")).expect("fixture page.png");
    let image_url = format!("data:image/png;base64,{}", STANDARD.encode(&image));
    let mut report = json!({ "input_sha256": hex::encode(Sha256::digest(&image)), "observations": {} });

    let (status, raw) = gateway.http("/health", None);
    let identity: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    report["gateway_identity"] = json!({ "status": status, "body": identity });
    let served = identity.pointer("/build/source_revision").and_then(Value::as_str);
    let mut failures: Vec<String> = Vec::new();
    if status != 200 || served != Some(expected.as_str()) {
        failures.push(format!("expected serving revision {expected}, got HTTP {status}: {raw}"));
    } else {
        let (status, raw) = gateway.completion(&image_url, false, "best");
        report["observations"]["buffered_image_answer"] = json!({ "status": status, "response": raw });
        let answer = serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|payload| payload.pointer("/choices/0/message/content").and_then(Value::as_str).map(str::to_string));
        if status != 200 || !answer.as_deref().map(normalized).unwrap_or_default().contains(EXPECTED_HEADING) {
            failures.push(format!("buffered image answer: HTTP {status}: {raw}"));
        }
        let (status, raw) = gateway.completion(&image_url, true, "best");
        report["observations"]["streamed_image_answer"] = json!({ "status": status, "response": raw });
        match streamed_text(&raw) {
            Ok(text) if status == 200 && normalized(&text).contains(EXPECTED_HEADING) => {}
            Ok(text) => failures.push(format!("streamed image answer: HTTP {status}: {text}")),
            Err(detail) => failures.push(format!("streamed image answer: {detail}")),
        }
        let (status, raw) = gateway.completion(&image_url, false, "unauthorized-image-selection-probe");
        report["observations"]["unauthorized_route_stays_refused"] = json!({ "status": status, "response": raw });
        let code = serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|payload| payload.pointer("/error/code").and_then(Value::as_str).map(str::to_string));
        if status != 403 || code.as_deref() != Some("forbidden") {
            failures.push(format!("unauthorized route: HTTP {status}: {raw}"));
        }
    }

    let revision = std::env::var("WISENT_SOURCE_COMMIT").unwrap_or_else(|_| {
        let output = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(root()).output().expect("git");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    });
    report["source_revision"] = json!(revision);
    report["failures"] = json!(failures);
    let output = root().join(".wisent-output/image-selection-real").join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&output).expect("evidence directory");
    std::fs::write(output.join("report.json"), serde_json::to_string_pretty(&report).expect("report") + "\n").expect("report written");
    println!("Image selection evidence: {}", output.join("report.json").display());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
