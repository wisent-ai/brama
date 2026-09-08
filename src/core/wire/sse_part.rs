//! Part of `wire`, split out to keep every file inside the line limit.
#![allow(unused_imports)]

use super::*;
use serde_json::{json, Value};
use crate::providers::stream::{StreamDelta, StreamItem};
use crate::types::{Message, ModelRequest, ModelResponse, Tool, ToolCall, ToolFunction};
use axum::response::sse::Event;

// ---------------------------------------------------------------------------
// Streaming egress.
// ---------------------------------------------------------------------------

/// Map a neutral finish reason onto the Anthropic stop vocabulary.
pub(crate) fn anthropic_stop_reason(reason: Option<&str>, saw_tool_calls: bool) -> String {
    match reason {
        Some("end_turn") | Some("stop") | Some("stop_sequence") => "end_turn",
        Some("max_tokens") | Some("length") => "max_tokens",
        Some("tool_use") | Some("tool_calls") => "tool_use",
        Some(other) if other.starts_with("incomplete") => {
            if other.contains("max_output_tokens") {
                "max_tokens"
            } else {
                "end_turn"
            }
        }
        _ => {
            if saw_tool_calls {
                "tool_use"
            } else {
                "end_turn"
            }
        }
    }
    .to_string()
}

pub(crate) fn sse(event: &str, data: Value) -> Event {
    Event::default()
        .event(event)
        .data(serde_json::to_string(&data).unwrap_or_default())
}

/// Which kind of content block the Anthropic encoder currently has open.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnthropicBlock {
    Text,
    ToolUse,
}

/// Encode neutral provider events as an Anthropic Messages event stream.
///
/// Block indices are assigned here, not taken from the provider: the neutral
/// event's index distinguishes concurrent tool calls, and the map below keeps
/// each provider index attached to the block it opened.
pub struct AnthropicEventStream {
    pub(crate) rx: tokio::sync::mpsc::Receiver<StreamItem>,
    pub(crate) pending: std::collections::VecDeque<Event>,
    pub(crate) message_id: String,
    pub(crate) model: String,
    pub(crate) started_message: bool,
    pub(crate) open_block: Option<(u32, AnthropicBlock)>,
    pub(crate) block_count: u32,
    pub(crate) tool_blocks: std::collections::HashMap<u32, u32>,
    pub(crate) saw_tool_calls: bool,
    pub(crate) finish_reason: Option<String>,
    pub(crate) output_tokens: u32,
    pub(crate) input_tokens: u32,
    pub(crate) terminated: bool,
    pub(crate) failed: bool,
    pub(crate) on_end: Option<StreamAccounting>,
}

impl AnthropicEventStream {
    pub fn new(
        rx: tokio::sync::mpsc::Receiver<StreamItem>,
        message_id: String,
        model: String,
        on_end: StreamAccounting,
    ) -> Self {
        Self {
            rx,
            pending: std::collections::VecDeque::new(),
            message_id,
            model,
            started_message: false,
            open_block: None,
            block_count: 0,
            tool_blocks: std::collections::HashMap::new(),
            saw_tool_calls: false,
            finish_reason: None,
            output_tokens: 0,
            input_tokens: 0,
            terminated: false,
            failed: false,
            on_end: Some(on_end),
        }
    }

    /// Report this stream's end to the layer that owns the statistics, once.
    pub(crate) fn account(&mut self, failed: bool) {
        if let Some(mut report) = self.on_end.take() {
            report(self.input_tokens, self.output_tokens, failed);
        }
    }

    pub(crate) fn message_start(&self) -> Event {
        sse(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": { "input_tokens": 0, "output_tokens": 0 },
                },
            }),
        )
    }

    /// Close whatever block is open, if one is, and remember the event.
    pub(crate) fn close_open_block(&mut self) {
        if let Some((index, _)) = self.open_block.take() {
            self.pending.push_back(sse(
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": index }),
            ));
        }
    }

    pub(crate) fn open_text_block(&mut self) {
        if self.open_block == Some((self.block_count, AnthropicBlock::Text)) {
            return;
        }
        self.close_open_block();
        let index = self.block_count;
        self.block_count += 1;
        self.open_block = Some((index, AnthropicBlock::Text));
        self.pending.push_back(sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": { "type": "text", "text": "" },
            }),
        ));
    }
}

impl futures_core::Stream for AnthropicEventStream {
    type Item = Result<Event, std::convert::Infallible>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use std::task::Poll;
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }
            if self.terminated {
                let failed = self.failed;
                self.account(failed);
                return Poll::Ready(None);
            }
            let item = match self.rx.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    self.terminated = true;
                    self.failed = true;
                    continue;
                }
                Poll::Ready(Some(item)) => item,
            };
            if !self.started_message {
                self.started_message = true;
                let start = self.message_start();
                self.pending.push_back(start);
            }
            match item {
                StreamItem::Delta(StreamDelta::Text(text)) => {
                    self.open_text_block();
                    let index = self.open_block.map(|(index, _)| index).unwrap_or_default();
                    self.pending.push_back(sse(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": { "type": "text_delta", "text": text },
                        }),
                    ));
                }
                StreamItem::Delta(StreamDelta::ToolCallStart { index, id, name }) => {
                    self.saw_tool_calls = true;
                    self.close_open_block();
                    let block_index = self.block_count;
                    self.block_count += 1;
                    self.open_block = Some((block_index, AnthropicBlock::ToolUse));
                    self.tool_blocks.insert(index, block_index);
                    self.pending.push_back(sse(
                        "content_block_start",
                        json!({
                            "type": "content_block_start",
                            "index": block_index,
                            "content_block": { "type": "tool_use", "id": id, "name": name, "input": {} },
                        }),
                    ));
                }
                StreamItem::Delta(StreamDelta::ToolCallArguments { index, delta }) => {
                    let block_index = self
                        .tool_blocks
                        .get(&index)
                        .copied()
                        .or(self.open_block.map(|(index, _)| index))
                        .unwrap_or_default();
                    self.pending.push_back(sse(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": block_index,
                            "delta": { "type": "input_json_delta", "partial_json": delta },
                        }),
                    ));
                }
                StreamItem::Delta(StreamDelta::Finish { reason }) => {
                    self.finish_reason = Some(anthropic_stop_reason(
                        reason.as_deref(),
                        self.saw_tool_calls,
                    ));
                }
                StreamItem::Delta(StreamDelta::Usage {
                    input_tokens,
                    output_tokens,
                }) => {
                    self.input_tokens = input_tokens;
                    self.output_tokens = output_tokens;
                }
                StreamItem::Done => {
                    self.close_open_block();
                    let reason = self
                        .finish_reason
                        .clone()
                        .unwrap_or_else(|| anthropic_stop_reason(None, self.saw_tool_calls));
                    let (input_tokens, output_tokens) = (self.input_tokens, self.output_tokens);
                    self.pending.push_back(sse(
                        "message_delta",
                        json!({
                            "type": "message_delta",
                            "delta": { "stop_reason": reason, "stop_sequence": Value::Null },
                            "usage": { "input_tokens": input_tokens, "output_tokens": output_tokens },
                        }),
                    ));
                    self.pending
                        .push_back(sse("message_stop", json!({ "type": "message_stop" })));
                    self.terminated = true;
                }
                StreamItem::Failed(message) => {
                    tracing::warn!(
                        event = "stream_failed_mid_flight",
                        format = "anthropic-messages",
                        error = %message,
                        "provider stream failed after commit; ending without message_stop"
                    );
                    self.terminated = true;
                    self.failed = true;
                }
            }
        }
    }
}

/// One output item the Responses encoder is accumulating for its terminal
/// `response.completed`, which must carry the whole generation again.
#[derive(Clone)]
pub(crate) enum ResponsesOutputItem {
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
