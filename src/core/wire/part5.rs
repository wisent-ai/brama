//! Part of `wire`, split out to keep every file inside the line limit.
#![allow(unused_imports)]

use super::*;
use serde_json::{json, Value};
use crate::providers::stream::{StreamDelta, StreamItem};
use crate::types::{Message, ModelRequest, ModelResponse, Tool, ToolCall, ToolFunction};
use axum::response::sse::Event;

impl futures_core::Stream for ResponsesEventStream {
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
            if !self.started {
                self.started = true;
                let shell = self.response_shell("in_progress");
                self.pending.push_back(sse(
                    "response.created",
                    json!({ "type": "response.created", "response": shell }),
                ));
            }
            match item {
                StreamItem::Delta(StreamDelta::Text(text)) => {
                    if self.open_item.is_none() {
                        let id = format!("msg_{}", uuid::Uuid::new_v4().simple());
                        let index = self.output_index;
                        self.output_index += 1;
                        self.open_item = Some(index);
                        self.items.push(ResponsesOutputItem::Message {
                            id: id.clone(),
                            text: String::new(),
                        });
                        self.pending.push_back(sse(
                            "response.output_item.added",
                            json!({
                                "type": "response.output_item.added",
                                "output_index": index,
                                "item": {
                                    "type": "message",
                                    "id": id,
                                    "status": "in_progress",
                                    "role": "assistant",
                                    "content": [],
                                },
                            }),
                        ));
                        self.pending.push_back(sse(
                            "response.content_part.added",
                            json!({
                                "type": "response.content_part.added",
                                "item_id": id,
                                "output_index": index,
                                "content_index": 0,
                                "part": { "type": "output_text", "text": "", "annotations": [] },
                            }),
                        ));
                    }
                    if let Some(ResponsesOutputItem::Message { text: full, .. }) =
                        self.items.last_mut()
                    {
                        full.push_str(&text);
                    }
                    let item_id = self.current_item_id();
                    let index = self.output_index.saturating_sub(1);
                    self.pending.push_back(sse(
                        "response.output_text.delta",
                        json!({
                            "type": "response.output_text.delta",
                            "item_id": item_id,
                            "output_index": index,
                            "content_index": 0,
                            "delta": text,
                        }),
                    ));
                }
                StreamItem::Delta(StreamDelta::ToolCallStart { index, id, name }) => {
                    self.close_open_item();
                    let item_id = format!("fc_{}", uuid::Uuid::new_v4().simple());
                    let out_index = self.output_index;
                    self.output_index += 1;
                    self.open_item = Some(out_index);
                    self.tool_items.insert(index, out_index);
                    self.items.push(ResponsesOutputItem::FunctionCall {
                        id: item_id.clone(),
                        call_id: id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                    });
                    self.pending.push_back(sse(
                        "response.output_item.added",
                        json!({
                            "type": "response.output_item.added",
                            "output_index": out_index,
                            "item": {
                                "type": "function_call",
                                "id": item_id,
                                "call_id": id,
                                "name": name,
                                "arguments": "",
                                "status": "in_progress",
                            },
                        }),
                    ));
                }
                StreamItem::Delta(StreamDelta::ToolCallArguments { index, delta }) => {
                    let out_index = self
                        .tool_items
                        .get(&index)
                        .copied()
                        .unwrap_or_else(|| self.output_index.saturating_sub(1));
                    let item_position = self
                        .items
                        .iter()
                        .rposition(|item| matches!(item, ResponsesOutputItem::FunctionCall { .. }));
                    if let Some(position) = item_position {
                        if let ResponsesOutputItem::FunctionCall { arguments, .. } =
                            &mut self.items[position]
                        {
                            arguments.push_str(&delta);
                        }
                    }
                    let item_id = self.current_item_id();
                    self.pending.push_back(sse(
                        "response.function_call_arguments.delta",
                        json!({
                            "type": "response.function_call_arguments.delta",
                            "item_id": item_id,
                            "output_index": out_index,
                            "delta": delta,
                        }),
                    ));
                }
                StreamItem::Delta(StreamDelta::Finish { .. }) => {}
                StreamItem::Delta(StreamDelta::Usage {
                    input_tokens,
                    output_tokens,
                }) => {
                    self.input_tokens = input_tokens;
                    self.output_tokens = output_tokens;
                }
                StreamItem::Done => {
                    self.close_open_item();
                    let output: Vec<Value> = self
                        .items
                        .iter()
                        .map(|item| match item {
                            ResponsesOutputItem::Message { id, text } => json!({
                                "type": "message",
                                "id": id,
                                "status": "completed",
                                "role": "assistant",
                                "content": [{ "type": "output_text", "text": text, "annotations": [] }],
                            }),
                            ResponsesOutputItem::FunctionCall {
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
                        })
                        .collect();
                    let mut response = self.response_shell("completed");
                    response["output"] = Value::Array(output);
                    response["usage"] = json!({
                        "input_tokens": self.input_tokens,
                        "output_tokens": self.output_tokens,
                        "total_tokens": self.input_tokens + self.output_tokens,
                    });
                    self.pending.push_back(sse(
                        "response.completed",
                        json!({ "type": "response.completed", "response": response }),
                    ));
                    self.terminated = true;
                }
                StreamItem::Failed(message) => {
                    tracing::warn!(
                        event = "stream_failed_mid_flight",
                        format = "openai-responses",
                        error = %message,
                        "provider stream failed after commit; ending with response.failed"
                    );
                    let mut response = self.response_shell("failed");
                    response["error"] = json!({ "message": message });
                    self.pending.push_back(sse(
                        "response.failed",
                        json!({ "type": "response.failed", "response": response }),
                    ));
                    self.terminated = true;
                    self.failed = true;
                }
            }
        }
    }
}
