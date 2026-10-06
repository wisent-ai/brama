//! What a Weles sign-in reports while it runs, read as it arrives.
//!
//! An admitted `POST /reauth` answers one JSON object per line: `admitted`,
//! `started`, one `stage` per step of the browser trajectory, an
//! `operator_request` when the run waits for a person, and last `result`.
//! A browser sign-in has no clock that ends it, so the verdict alone left
//! every caller -- the CLI, Desktop, the renewal sweep -- waiting in silence
//! with nothing to say about what it waited for. Each event goes to the
//! caller's [`Progress`] as it arrives and is kept for the verdict, and an
//! answer that ends without `result` is reported with the run and the last
//! stage it reached.

use std::sync::Arc;

use serde_json::{json, Value};

/// The content type of an admitted sign-in's answer.
pub(crate) const PROGRESS_CONTENT_TYPE: &str = "application/x-ndjson";

/// Receives every event Weles reports for a running sign-in, as Weles sent
/// it. The CLI prints them, the gateway streams them to Desktop, the sweep
/// logs them.
pub type Progress = Arc<dyn Fn(&Value) + Send + Sync>;

/// What the answer said before it ended: the run, its stages, the operator
/// request it waits on, and the `result` when it arrived.
#[derive(Default)]
pub(crate) struct Observed {
    pub run_id: Option<String>,
    pub host: Option<String>,
    pub stages: Vec<Value>,
    pub operator_request: Option<Value>,
    pub result: Option<Value>,
}

impl Observed {
    /// Where the run was when it stopped reporting, in one clause.
    pub fn whereabouts(&self) -> String {
        let run = self.run_id.as_deref().unwrap_or("not yet started");
        let host = self
            .host
            .as_deref()
            .map(|host| format!(" on {host}"))
            .unwrap_or_default();
        let stage = self
            .stages
            .last()
            .and_then(|stage| {
                Some(format!(
                    "last stage {} (reached {})",
                    stage.get("stage")?.as_str()?,
                    stage.get("at")?.as_str()?
                ))
            })
            .unwrap_or_else(|| "no stage reported".to_string());
        let waiting = self
            .operator_request
            .as_ref()
            .and_then(|request| request.get("instruction").and_then(Value::as_str))
            .map(|instruction| format!("; it was waiting for the operator: {instruction}"))
            .unwrap_or_default();
        format!("run {run}{host}, {stage}{waiting}")
    }

    /// The last stage reached, for a failure's `stage`.
    pub fn last_stage(&self) -> Option<&str> {
        self.stages
            .last()
            .and_then(|stage| stage.get("stage"))
            .and_then(Value::as_str)
    }

    /// Write what the run reported into the identity the verdict records.
    pub fn record(&self, identity: &mut Value) {
        identity["stages"] = json!(self.stages);
        identity["operator_request"] = self.operator_request.clone().unwrap_or(Value::Null);
        if let Some(run_id) = &self.run_id {
            identity["run_id"] = json!(run_id);
        }
    }

    fn take(&mut self, event: &Value) {
        let text = |field: &str| event.get(field).and_then(Value::as_str).map(str::to_owned);
        match event.get("event").and_then(Value::as_str) {
            Some("started") => {
                self.run_id = text("run_id");
                self.host = text("host");
            }
            Some("stage") => self.stages.push(json!({
                "stage": event.get("stage"),
                "at": event.get("at"),
            })),
            Some("operator_request") => {
                self.operator_request = event.get("request").cloned();
            }
            _ => {}
        }
    }
}

/// Read an admitted sign-in's answer to its end, handing every event to
/// `progress` as it arrives. `Err` carries what was observed and why the
/// answer stopped before `result`.
pub(crate) async fn read(
    mut response: reqwest::Response,
    progress: Option<&Progress>,
) -> Result<Observed, Box<(Observed, String)>> {
    let mut observed = Observed::default();
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                let detail = format!("Weles stopped answering: {error:?}");
                return Err(Box::new((observed, detail)));
            }
        };
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let event: Value = match serde_json::from_str(line) {
                Ok(event) => event,
                Err(error) => {
                    let detail =
                        format!("Weles sent a progress line that is not JSON ({error}): {line}");
                    return Err(Box::new((observed, detail)));
                }
            };
            if event.get("event").and_then(Value::as_str) == Some("result") {
                observed.result = Some(event);
                return Ok(observed);
            }
            observed.take(&event);
            if let Some(progress) = progress {
                progress(&event);
            }
        }
    }
    let unfinished = String::from_utf8_lossy(&pending).trim().to_string();
    let detail = if unfinished.is_empty() {
        "Weles ended the answer without a result".to_string()
    } else {
        format!("Weles ended the answer inside a line, without a result: {unfinished}")
    };
    Err(Box::new((observed, detail)))
}

/// One event as the line a person reads, or `None` for an event that tells a
/// person nothing. The CLI prints these and the sweep logs them.
pub fn sentence(event: &Value) -> Option<String> {
    let text = |field: &str| {
        event
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or("unreported")
    };
    match event.get("event").and_then(Value::as_str)? {
        "admitted" => Some(format!(
            "Weles admitted the sign-in of {} with login {}{}",
            text("account_ref"),
            text("login_item"),
            if event.get("coalesced").and_then(Value::as_bool) == Some(true) {
                "; it joined a sign-in of this account already running"
            } else {
                ""
            }
        )),
        "started" => Some(format!(
            "Weles run {} started on {} at {}",
            text("run_id"),
            text("host"),
            text("started_at")
        )),
        "stage" => Some(format!("{} stage {}", text("at"), text("stage"))),
        "operator_request" => {
            let request = event.get("request").cloned().unwrap_or(Value::Null);
            let field = |name: &str| {
                request
                    .get(name)
                    .and_then(Value::as_str)
                    .unwrap_or("unreported")
                    .to_owned()
            };
            Some(format!(
                "WAITING FOR THE OPERATOR since {}: {} (account {}, request {}, {}; `weles operator-requests show {}`)",
                field("opened_at"),
                field("instruction"),
                field("account"),
                field("id"),
                if request.get("paged").and_then(Value::as_bool) == Some(true) {
                    "paged"
                } else {
                    "NOT paged: no alert channel took it"
                },
                field("id")
            ))
        }
        _ => None,
    }
}
