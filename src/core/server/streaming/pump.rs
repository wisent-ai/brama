//! Driving the chat-chunk encoder: one provider event in, zero or more caller
//! frames out, and the two ways a stream ends — `[DONE]` after a finish, or
//! silence after a failure the caller can no longer be told about in-band.

use serde_json::json;
use tracing::warn;

use crate::providers::stream::{StreamDelta, StreamItem};

use super::chunks::ChatChunkStream;
use super::openai_finish_reason;

impl futures_core::Stream for ChatChunkStream {
    type Item = Result<axum::response::sse::Event, std::convert::Infallible>;

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
                self.account();
                return Poll::Ready(None);
            }
            let item = match self.rx.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                // The recorder closed the channel without a terminal item: an
                // abnormal end, signalled the same way as a mid-stream failure
                // -- the stream simply stops, without `data: [DONE]`.
                Poll::Ready(None) => {
                    self.terminated = true;
                    self.failed = true;
                    continue;
                }
                Poll::Ready(Some(item)) => item,
            };
            match item {
                StreamItem::Delta(StreamDelta::Text(text)) => {
                    // Queued rather than returned: the role chunk may be
                    // waiting in front of it, and a caller that receives
                    // content before the role it belongs to is reading a
                    // different conversation than the one being sent.
                    if !self.sent_role {
                        self.sent_role = true;
                        let role = self.role_chunk();
                        self.pending.push_back(role);
                    }
                    let content = self.chunk(json!({ "content": text }), None);
                    self.pending.push_back(content);
                }
                StreamItem::Delta(StreamDelta::ToolCallStart { index, id, name }) => {
                    self.saw_tool_calls = true;
                    if !self.sent_role {
                        self.sent_role = true;
                        let role = self.role_chunk();
                        self.pending.push_back(role);
                    }
                    let delta = json!({
                        "tool_calls": [{
                            "index": index,
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": "" },
                        }],
                    });
                    let chunk = self.chunk(delta, None);
                    self.pending.push_back(chunk);
                }
                StreamItem::Delta(StreamDelta::ToolCallArguments { index, delta }) => {
                    let delta = json!({
                        "tool_calls": [{
                            "index": index,
                            "function": { "arguments": delta },
                        }],
                    });
                    let chunk = self.chunk(delta, None);
                    self.pending.push_back(chunk);
                }
                StreamItem::Delta(StreamDelta::Finish { reason }) => {
                    self.finish_reason =
                        Some(openai_finish_reason(reason.as_deref(), self.saw_tool_calls));
                }
                StreamItem::Delta(StreamDelta::Usage {
                    input_tokens,
                    output_tokens,
                }) => {
                    self.usage = Some((input_tokens, output_tokens));
                    self.input_tokens = input_tokens;
                    self.output_tokens = output_tokens;
                }
                StreamItem::Done => {
                    let reason = self
                        .finish_reason
                        .clone()
                        .unwrap_or_else(|| openai_finish_reason(None, self.saw_tool_calls));
                    let finish = self.chunk(json!({}), Some(&reason));
                    self.pending.push_back(finish);
                    if let Some((input_tokens, output_tokens)) = self.usage {
                        let usage = self.usage_chunk(input_tokens, output_tokens);
                        self.pending.push_back(usage);
                    }
                    self.pending
                        .push_back(axum::response::sse::Event::default().data("[DONE]"));
                    self.terminated = true;
                }
                StreamItem::Failed(message) => {
                    warn!(
                        event = "stream_failed_mid_flight",
                        request_id = %self.request_id,
                        model = %self.model,
                        error = %message,
                        "provider stream failed after commit; ending without [DONE]"
                    );
                    self.terminated = true;
                    self.failed = true;
                }
            }
        }
    }
}
