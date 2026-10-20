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

use crate::providers::stream::{StopReason, StreamItem};
use axum::response::sse::Event;

use super::{sse, StreamAccounting};

/// The position of the one text part a Brama message item carries: the
/// Responses wire numbers an item's content parts from zero.
const ONLY_PART: u32 = 0; // https://platform.openai.com/docs/api-reference/responses-streaming content_index

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

impl ResponsesOutputItem {
    /// The item as the terminal response lists it.
    fn completed(&self) -> Value {
        match self {
            Self::Message { id, text } => json!({
                "type": "message",
                "id": id,
                "status": "completed",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": text, "annotations": [] }],
            }),
            Self::FunctionCall {
                id,
                call_id,
                name,
                arguments,
            } => json!({
                "type": "function_call",
                "id": id,
                "call_id": call_id,
                "name": name,
                "arguments": arguments,
                "status": "completed",
            }),
        }
    }
}

/// Encode neutral provider events as an OpenAI Responses event stream.
///
/// An item's `output_index` is its position in `items`, the order it was
/// added in, so no counter can drift from the list it numbers.
pub struct ResponsesEventStream {
    rx: tokio::sync::mpsc::Receiver<StreamItem>,
    pending: std::collections::VecDeque<Event>,
    response_id: String,
    model: String,
    created: u64,
    started: bool,
    /// The position of the item still open for deltas.
    open_item: Option<usize>,
    items: Vec<ResponsesOutputItem>,
    /// Each provider tool-call index's item position.
    tool_items: std::collections::HashMap<u32, usize>,
    /// The provider's (input, output) meter, once it stated one.
    usage: Option<(u32, u32)>,
    /// Why the provider stopped, once it stated it.
    stop: Option<StopReason>,
    terminated: bool,
    failed: bool,
    on_end: Option<StreamAccounting>,
}

impl ResponsesEventStream {
    pub fn new(
        rx: tokio::sync::mpsc::Receiver<StreamItem>,
        response_id: String,
        model: String,
        created: u64,
        on_end: StreamAccounting,
    ) -> Self {
        Self {
            rx,
            pending: std::collections::VecDeque::new(),
            response_id,
            model,
            created,
            started: false,
            open_item: None,
            items: Vec::new(),
            tool_items: std::collections::HashMap::new(),
            usage: None,
            stop: None,
            terminated: false,
            failed: false,
            on_end: Some(on_end),
        }
    }

    /// Report this stream's end to the layer that owns the statistics, once.
    fn account(&mut self, failed: bool) {
        if let Some(mut report) = self.on_end.take() {
            report(self.usage, failed);
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

    /// The terminal response: `completed`, or `incomplete` with the reason
    /// the provider stated when it stopped short, carrying every item and the
    /// usage only when the provider stated one.
    fn terminal_response(&self) -> (&'static str, Value) {
        let incomplete = match &self.stop {
            Some(StopReason::MaxTokens) => Some(Some("max_output_tokens".to_string())),
            Some(StopReason::ContentFilter) => Some(Some("content_filter".to_string())),
            Some(StopReason::Incomplete(reason)) => Some(reason.clone()),
            _ => None,
        };
        let status = match incomplete {
            Some(_) => "incomplete",
            None => "completed",
        };
        let mut response = self.response_shell(status);
        response["output"] = Value::Array(self.items.iter().map(ResponsesOutputItem::completed).collect());
        if let Some(reason) = incomplete {
            response["incomplete_details"] = json!({ "reason": reason });
        }
        if let Some((input_tokens, output_tokens)) = self.usage {
            response["usage"] = json!({
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "total_tokens": u64::from(input_tokens) + u64::from(output_tokens),
            });
        }
        let event = match status {
            "incomplete" => "response.incomplete",
            _ => "response.completed",
        };
        (event, response)
    }

    /// Close the open output item with its own `done` events, per format.
    fn close_open_item(&mut self) {
        let Some(index) = self.open_item.take() else {
            return;
        };
        match self.items.get(index).cloned() {
            Some(ResponsesOutputItem::Message { id, text }) => {
                self.pending.push_back(sse(
                    "response.output_text.done",
                    json!({
                        "type": "response.output_text.done",
                        "item_id": id,
                        "output_index": index,
                        "content_index": ONLY_PART,
                        "text": text,
                    }),
                ));
                self.pending.push_back(sse(
                    "response.content_part.done",
                    json!({
                        "type": "response.content_part.done",
                        "item_id": id,
                        "output_index": index,
                        "content_index": ONLY_PART,
                        "part": { "type": "output_text", "text": text, "annotations": [] },
                    }),
                ));
                self.pending.push_back(sse(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": index,
                        "item": ResponsesOutputItem::Message { id, text }.completed(),
                    }),
                ));
            }
            Some(call @ ResponsesOutputItem::FunctionCall { .. }) => {
                if let ResponsesOutputItem::FunctionCall { id, arguments, .. } = &call {
                    self.pending.push_back(sse(
                        "response.function_call_arguments.done",
                        json!({
                            "type": "response.function_call_arguments.done",
                            "item_id": id,
                            "output_index": index,
                            "arguments": arguments,
                        }),
                    ));
                }
                self.pending.push_back(sse(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": index,
                        "item": call.completed(),
                    }),
                ));
            }
            None => {}
        }
    }
}
