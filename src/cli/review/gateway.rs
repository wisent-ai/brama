//! One chat completion with tools, asked of a Brama gateway over its
//! OpenAI-compatible endpoint. A refusal is returned with the gateway's own
//! status and body, so the failed review names what the gateway said.

use serde_json::{json, Value};

pub(super) struct Gateway {
    client: reqwest::Client,
    url: String,
    token: String,
    model: String,
}

impl Gateway {
    pub(super) fn new(base: &str, token: &str, model: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: format!("{}/v1/chat/completions", base.trim_end_matches('/')),
            token: token.to_string(),
            model: model.to_string(),
        }
    }

    /// The assistant message of one completion.
    pub(super) async fn complete(
        &self,
        messages: &[Value],
        tools: &Value,
    ) -> Result<Value, String> {
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(&self.token)
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "tool_choice": "auto",
                "stream": false,
            }))
            .send()
            .await
            .map_err(|error| format!("{} could not be reached: {error}", self.url))?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            format!(
                "{} answered {status} with an unreadable body: {error}",
                self.url
            )
        })?;
        if !status.is_success() {
            return Err(format!("{} answered {status}: {}", self.url, body.trim()));
        }
        let document: Value = serde_json::from_str(&body).map_err(|error| {
            format!(
                "{} answered {status} with no JSON document: {error}",
                self.url
            )
        })?;
        document
            .pointer("/choices/0/message")
            .filter(|message| message.is_object())
            .cloned()
            .ok_or_else(|| {
                format!(
                    "{} answered with no assistant message: {}",
                    self.url,
                    body.trim()
                )
            })
    }
}
