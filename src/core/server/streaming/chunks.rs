//! The OpenAI chat-chunk encoder: what one caller-facing SSE frame looks like,
//! and the stream's end reported once through the layer's accounting.

use serde_json::{json, Value};

use crate::core::wire::StreamAccounting;
use crate::providers::stream::StreamItem;

/// The index of the one choice a Brama stream carries: a request asks for one
/// generation, and the chunk object numbers its choices from zero.
const ONLY_CHOICE: u32 = 0; // https://platform.openai.com/docs/api-reference/chat-streaming/streaming choices[].index

/// One caller-facing SSE stream built from neutral provider events.
///
/// Process statistics are reported at the stream's end, because that is when
/// the numbers exist; the subscription ledger is accounted one layer down,
/// where the credential that paid is known.
pub(in crate::core::server) struct ChatChunkStream {
    pub(super) rx: tokio::sync::mpsc::Receiver<StreamItem>,
    pub(super) pending: std::collections::VecDeque<axum::response::sse::Event>,
    pub(super) request_id: String,
    pub(super) model: String,
    pub(super) created: u64,
    pub(super) terminated: bool,
    pub(super) sent_role: bool,
    pub(super) finish_reason: Option<String>,
    /// The provider's (input, output) meter, once it stated one.
    pub(super) usage: Option<(u32, u32)>,
    pub(super) failed: bool,
    pub(super) on_end: Option<StreamAccounting>,
}

impl ChatChunkStream {
    /// One stream, before any event has arrived: everything else about it is
    /// derived from what the provider sends next.
    pub(in crate::core::server) fn new(
        rx: tokio::sync::mpsc::Receiver<StreamItem>,
        request_id: String,
        model: String,
        created: u64,
        on_end: StreamAccounting,
    ) -> Self {
        Self {
            rx,
            pending: std::collections::VecDeque::new(),
            request_id,
            model,
            created,
            terminated: false,
            sent_role: false,
            finish_reason: None,
            usage: None,
            failed: false,
            on_end: Some(on_end),
        }
    }

    pub(super) fn chunk(
        &self,
        delta: Value,
        finish_reason: Option<&str>,
    ) -> axum::response::sse::Event {
        // A finish the provider did not state is null, as the chunk object
        // writes every unfinished choice.
        let choice = json!({ "index": ONLY_CHOICE, "delta": delta, "finish_reason": finish_reason });
        // A JSON value always writes; its Display is that writing, with no
        // failure to stand an empty frame in for.
        axum::response::sse::Event::default().data(
            json!({
                "id": self.request_id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [choice],
            })
            .to_string(),
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
            json!({
                "id": self.request_id,
                "object": "chat.completion.chunk",
                "created": self.created,
                "model": self.model,
                "choices": [],
                "usage": {
                    "prompt_tokens": input_tokens,
                    "completion_tokens": output_tokens,
                    "total_tokens": u64::from(input_tokens) + u64::from(output_tokens),
                },
            })
            .to_string(),
        )
    }

    /// Report this stream's end to the layer that owns the statistics, once.
    pub(super) fn account(&mut self) {
        if let Some(mut report) = self.on_end.take() {
            report(self.usage, self.failed);
        }
    }
}
