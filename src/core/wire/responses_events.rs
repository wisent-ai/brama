//! What an OpenAI Responses stream is accumulating, and what each output item
//! owes the caller when it closes.
//!
//! This format streams a response as numbered output items and then repeats
//! the entire generation in its terminal `response.completed`, so unlike the
//! other two encoders in this crate it cannot forget what it has already sent:
//! every message and function call stays here until the stream ends. Holding
//! that memory, and the `done` events an item is closed with, apart from the
//! event-by-event choreography in `pump` keeps the two questions separable --
//! what the stream remembers changes when the format's item shapes change,
//! while `pump` changes when its event order does.

mod pump;

use serde_json::{json, Value};

use crate::providers::stream::StreamItem;
use axum::response::sse::Event;

use super::{sse, StreamAccounting};

/// One output item the Responses encoder is accumulating for its terminal
/// `response.completed`, which must carry the whole generation again.
#[derive(Clone)]
enum ResponsesOutputItem {
    Message {
        id: String,
        text: String,
    },
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
}

/// Encode neutral provider events as an OpenAI Responses event stream.
pub struct ResponsesEventStream {
    rx: tokio::sync::mpsc::Receiver<StreamItem>,
    pending: std::collections::VecDeque<Event>,
    response_id: String,
    model: String,
    created: u64,
    started: bool,
    output_index: u32,
    open_item: Option<u32>,
    items: Vec<ResponsesOutputItem>,
    tool_items: std::collections::HashMap<u32, u32>,
    input_tokens: u32,
    output_tokens: u32,
    terminated: bool,
    failed: bool,
    on_end: Option<StreamAccounting>,
}

impl ResponsesEventStream {
    pub fn new(
        rx: tokio::sync::mpsc::Receiver<StreamItem>,
        response_id: String,
        model: String,
        on_end: StreamAccounting,
    ) -> Self {
        Self {
            rx,
            pending: std::collections::VecDeque::new(),
            response_id,
            model,
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default(),
            started: false,
            output_index: 0,
            open_item: None,
            items: Vec::new(),
            tool_items: std::collections::HashMap::new(),
            input_tokens: 0,
            output_tokens: 0,
            terminated: false,
            failed: false,
            on_end: Some(on_end),
        }
    }

    /// Report this stream's end to the layer that owns the statistics, once.
    fn account(&mut self, failed: bool) {
        if let Some(mut report) = self.on_end.take() {
            report(self.input_tokens, self.output_tokens, failed);
        }
    }

    fn response_shell(&self, status: &str) -> Value {
        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created,
            "status": status,
            "model": self.model,
            "output": [],
        })
    }

    fn current_item_id(&self) -> String {
        match self.items.last() {
            Some(ResponsesOutputItem::Message { id, .. }) => id.clone(),
            Some(ResponsesOutputItem::FunctionCall { id, .. }) => id.clone(),
            None => String::new(),
        }
    }

    /// Close the open output item with its own `done` events, per format.
    fn close_open_item(&mut self) {
        if self.open_item.take().is_none() {
            return;
        }
        let index = self.output_index.saturating_sub(1);
        match self.items.last().cloned() {
            Some(ResponsesOutputItem::Message { id, text }) => {
                self.pending.push_back(sse(
                    "response.output_text.done",
                    json!({
                        "type": "response.output_text.done",
                        "item_id": id,
                        "output_index": index,
                        "content_index": 0,
                        "text": text,
                    }),
                ));
                self.pending.push_back(sse(
                    "response.content_part.done",
                    json!({
                        "type": "response.content_part.done",
                        "item_id": id,
                        "output_index": index,
                        "content_index": 0,
                        "part": { "type": "output_text", "text": text, "annotations": [] },
                    }),
                ));
                self.pending.push_back(sse(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": index,
                        "item": {
                            "type": "message",
                            "id": id,
                            "status": "completed",
                            "role": "assistant",
                            "content": [{ "type": "output_text", "text": text, "annotations": [] }],
                        },
                    }),
                ));
            }
            Some(ResponsesOutputItem::FunctionCall {
                id,
                call_id,
                name,
                arguments,
            }) => {
                self.pending.push_back(sse(
                    "response.function_call_arguments.done",
                    json!({
                        "type": "response.function_call_arguments.done",
                        "item_id": id,
                        "output_index": index,
                        "arguments": arguments,
                    }),
                ));
                self.pending.push_back(sse(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": index,
                        "item": {
                            "type": "function_call",
                            "id": id,
                            "call_id": call_id,
                            "name": name,
                            "arguments": arguments,
                            "status": "completed",
                        },
                    }),
                ));
            }
            None => {}
        }
    }
}
