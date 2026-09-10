//! The requests a story makes of its gateway, with exactly the headers each
//! audience presents: a bearer, the HMAC trio a signed agent adds, or both.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::{Gateway, AGENT, AGENT_BEARER, AGENT_SIGNING_SECRET, CONSOLE_BEARER};

impl Gateway {
    /// One request, with exactly the headers the named audience presents.
    pub fn request(
        &self,
        path: &str,
        method: reqwest::Method,
        bearer: Option<&str>,
        body: Option<&Value>,
        signed_as: Option<(&str, &str)>,
    ) -> (u16, Value) {
        let raw = body.map(|body| serde_json::to_vec(body).expect("request body"));
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.origin));
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        if let Some((agent, secret)) = signed_as {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_secs() as i64;
            let signature = brama::crypto::hmac_auth::compute_signature(
                agent,
                timestamp,
                raw.as_deref().unwrap_or_default(),
                secret.as_bytes(),
            )
            .expect("sign the request as the agent would");
            request = request
                .header("x-agent-id", agent)
                .header("x-agent-timestamp", timestamp.to_string())
                .header("x-agent-signature", signature);
        }
        if let Some(raw) = raw {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(raw);
        }
        let response = request.send().expect("Brama response");
        let status = response.status().as_u16();
        let body = response.json().expect("Brama JSON response");
        (status, body)
    }

    pub fn console(
        &self,
        path: &str,
        method: reqwest::Method,
        body: Option<&Value>,
    ) -> (u16, Value) {
        self.request(path, method, Some(CONSOLE_BEARER), body, None)
    }

    pub fn agent(&self, path: &str, method: reqwest::Method, body: Option<&Value>) -> (u16, Value) {
        self.request(
            path,
            method,
            Some(AGENT_BEARER),
            body,
            Some((AGENT, AGENT_SIGNING_SECRET)),
        )
    }
}
