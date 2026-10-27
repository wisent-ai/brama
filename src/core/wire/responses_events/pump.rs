//! Which Responses events one arriving neutral item makes the stream emit.
//!
//! The Responses wire is the most talkative of the three: a first delta has to
//! be preceded by `response.created`, an item being added, and a content part
//! being added, and every terminal state -- completed, incomplete or failed --
//! carries a whole response object again. That ordering is this module's only
//! subject, kept apart from the state it advances so that a change to the event
//! order does not have to be read as a change to what the stream remembers.

use serde_json::json;

use crate::core::wire::sse;
use crate::providers::stream::{StreamDelta, StreamItem};
use axum::response::sse::Event;

use super::{ResponsesEventStream, ResponsesOutputItem, ONLY_PART};

impl ResponsesEventStream {
    /// Open a message item for text, unless the open item already is one,
    /// and return its position.
    fn open_message(&mut self) -> usize {
        if let Some(index) = self.open_item {
            if matches!(
                self.items.get(index),
                Some(ResponsesOutputItem::Message { .. })
            ) {
                return index;
            }
        }
        self.close_open_item();
        let id = format!("msg_{}", uuid::Uuid::new_v4().simple());
        let index = self.items.len();
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
                "content_index": ONLY_PART,
                "part": { "type": "output_text", "text": "", "annotations": [] },
            }),
        ));
        index
    }

    /// End the stream as failed, telling the caller in-band why.
    fn fail(&mut self, message: &str) {
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
                    let index = self.open_message();
                    let item_id = match self.items.get_mut(index) {
                        Some(ResponsesOutputItem::Message { id, text: full }) => {
                            full.push_str(&text);
                            id.clone()
                        }
                        _ => {
                            self.fail("a text delta found no open message item to belong to");
                            continue;
                        }
                    };
                    self.pending.push_back(sse(
                        "response.output_text.delta",
                        json!({
                            "type": "response.output_text.delta",
                            "item_id": item_id,
                            "output_index": index,
                            "content_index": ONLY_PART,
                            "delta": text,
                        }),
                    ));
                }
                StreamItem::Delta(StreamDelta::ToolCallStart { index, id, name }) => {
                    self.close_open_item();
                    let item_id = format!("fc_{}", uuid::Uuid::new_v4().simple());
                    let out_index = self.items.len();
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
                    // Arguments belong to the call the provider index started;
                    // a fragment for a call never started ends the stream
                    // instead of being appended to whichever call is last.
                    let Some(out_index) = self.tool_items.get(&index).copied() else {
                        self.fail(&format!(
                            "the provider sent arguments for tool call {index}, which it never started"
                        ));
                        continue;
                    };
                    let item_id = match self.items.get_mut(out_index) {
                        Some(ResponsesOutputItem::FunctionCall { id, arguments, .. }) => {
                            arguments.push_str(&delta);
                            id.clone()
                        }
                        _ => {
                            self.fail(&format!(
                                "tool call {index} names output item {out_index}, which is not a function call"
                            ));
                            continue;
                        }
                    };
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
                StreamItem::Delta(StreamDelta::Finish { reason }) => {
                    self.stop = Some(reason);
                }
                StreamItem::Delta(StreamDelta::Usage {
                    input_tokens,
                    output_tokens,
                }) => {
                    self.usage = Some((input_tokens, output_tokens));
                }
                StreamItem::Done => {
                    self.close_open_item();
                    let (event, response) = self.terminal_response();
                    self.pending
                        .push_back(sse(event, json!({ "type": event, "response": response })));
                    self.terminated = true;
                }
                StreamItem::Failed(message) => {
                    tracing::warn!(
                        event = "stream_failed_mid_flight",
                        format = "openai-responses",
                        error = %message,
                        "provider stream failed after commit; ending with response.failed"
                    );
                    self.fail(&message);
                }
            }
        }
    }
}
