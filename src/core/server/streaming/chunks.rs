//! The OpenAI chat-chunk encoder: what one caller-facing SSE frame looks like,
//! and the single point at which a finished stream is accounted for.

use std::sync::atomic::Ordering;
use std::time::Instant;

use serde_json::{json, Value};

use crate::core::server::telemetry::{TOTAL_FAILURES, TOTAL_INPUT_TOKENS, TOTAL_OUTPUT_TOKENS};
use crate::providers::stream::StreamItem;

use super::epoch_seconds;

/// One caller-facing SSE stream built from neutral provider events.
///
/// Process statistics are accounted here, at the stream's end, because that
/// is when the numbers exist; the subscription ledger is accounted one layer
/// down, where the credential that paid is known.
pub(in crate::core::server) struct ChatChunkStream {
    pub(super) rx: tokio::sync::mpsc::Receiver<StreamItem>,
    pub(super) pending: std::collections::VecDeque<axum::response::sse::Event>,
    pub(super) request_id: String,
    pub(super) model: String,
    pub(super) requested_model: String,
    pub(super) created: u64,
    pub(super) started: Instant,
    pub(super) terminated: bool,
    pub(super) accounted: bool,
    pub(super) sent_role: bool,
    pub(super) saw_tool_calls: bool,
    pub(super) finish_reason: Option<String>,
    pub(super) usage: Option<(u32, u32)>,
    pub(super) input_tokens: u32,
    pub(super) output_tokens: u32,
    pub(super) failed: bool,
}

impl ChatChunkStream {
    /// One stream, before any event has arrived: everything else about it is
    /// derived from what the provider sends next.
    pub(in crate::core::server) fn new(
        rx: tokio::sync::mpsc::Receiver<StreamItem>,
        request_id: String,
        model: String,
        requested_model: String,
        started: Instant,
    ) -> Self {
        Self {
            rx,
            pending: std::collections::VecDeque::new(),
            request_id,
            model,
            requested_model,
            created: epoch_seconds(),
            started,
            terminated: false,
            accounted: false,
            sent_role: false,
            saw_tool_calls: false,
            finish_reason: None,
            usage: None,
            input_tokens: 0,
            output_tokens: 0,
            failed: false,
        }
    }

    pub(super) fn chunk(
        &self,
        delta: Value,
        finish_reason: Option<&str>,
    ) -> axum::response::sse::Event {
        let mut choice = json!({ "index": 0, "delta": delta });
        match finish_reason {
            Some(reason) => choice["finish_reason"] = json!(reason),
            None => choice["finish_reason"] = Value::Null,
        }
        axum::response::sse::Event::default().data(
            serde_json::to_string(&json!({
                "id": self.request_id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [choice],
            }))
            .unwrap_or_default(),
        )
    }

    pub(super) fn role_chunk(&self) -> axum::response::sse::Event {
        self.chunk(json!({ "role": "assistant" }), None)
    }

    pub(super) fn usage_chunk(
        &self,
        input_tokens: u32,
        output_tokens: u32,
    ) -> axum::response::sse::Event {
        axum::response::sse::Event::default().data(
            serde_json::to_string(&json!({
                "id": self.request_id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [],
                "usage": {
                    "prompt_tokens": input_tokens,
                    "completion_tokens": output_tokens,
                    "total_tokens": input_tokens + output_tokens,
                },
            }))
            .unwrap_or_default(),
        )
    }

    /// Fold this stream's end into the process statistics, exactly once.
    pub(super) fn account(&mut self) {
        if self.accounted {
            return;
        }
        self.accounted = true;
        TOTAL_INPUT_TOKENS.fetch_add(u64::from(self.input_tokens), Ordering::Relaxed);
        TOTAL_OUTPUT_TOKENS.fetch_add(u64::from(self.output_tokens), Ordering::Relaxed);
        if self.failed {
            TOTAL_FAILURES.fetch_add(u64::from(true), Ordering::Relaxed);
        } else {
            crate::core::perf::record(
                &self.requested_model,
                self.started.elapsed().as_secs_f64() * 1_000.0,
                self.output_tokens,
            );
        }
    }
}
