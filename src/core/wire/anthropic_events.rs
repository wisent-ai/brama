//! What a live generation looks like as an Anthropic Messages event stream.
//!
//! Anthropic streams a message as numbered content blocks that are opened,
//! filled and stopped, and it reports its stop reason and its meter on one
//! `message_delta` before `message_stop`. None of that ordering is present in
//! the neutral events this encoder is fed, so the whole choreography -- which
//! block is open, which index a tool call was given, and which of the format's
//! stop reasons a provider's stop means -- is decided here, in the one place a
//! change to Anthropic's stream has to be read.

use serde_json::{json, Value};

use crate::providers::stream::{StopReason, StreamDelta, StreamItem};
use axum::response::sse::Event;

use super::{sse, StreamAccounting};

/// The meter a `message_start` carries: this stream has counted nothing yet
/// when it opens, and the provider's stated meter follows on `message_delta`.
const NOTHING_COUNTED_YET: u32 = 0; // https://docs.anthropic.com/en/docs/build-with-claude/streaming message_start usage

/// A provider stop reason in the Anthropic stop vocabulary
/// (https://docs.anthropic.com/en/api/messages#response-stop-reason); a reason
/// it has no word for is passed on as the provider stated it.
fn anthropic_stop_reason(reason: &StopReason) -> String {
    match reason {
        StopReason::EndTurn => "end_turn".to_string(),
        StopReason::StopSequence => "stop_sequence".to_string(),
        StopReason::MaxTokens => "max_tokens".to_string(),
        StopReason::ToolUse => "tool_use".to_string(),
        StopReason::ContentFilter => "refusal".to_string(),
        StopReason::Incomplete(Some(stated)) | StopReason::Other(stated) => stated.clone(),
        StopReason::Incomplete(None) => "incomplete".to_string(),
    }
}

/// Which kind of content block the Anthropic encoder currently has open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AnthropicBlock {
    Text,
    ToolUse,
}

/// Encode neutral provider events as an Anthropic Messages event stream.
///
/// A block's index is its position among the blocks this stream opened, in
/// order, as the format numbers them; the map keeps each provider tool-call
/// index attached to the block it opened, so two interleaved calls stay two.
pub struct AnthropicEventStream {
    rx: tokio::sync::mpsc::Receiver<StreamItem>,
    pending: std::collections::VecDeque<Event>,
    message_id: String,
    model: String,
    started_message: bool,
    /// Every block opened so far, by index.
    blocks: Vec<AnthropicBlock>,
    /// The index of the block still open for deltas.
    open_block: Option<usize>,
    tool_blocks: std::collections::HashMap<u32, usize>,
    stop_reason: Option<String>,
    /// The provider's (input, output) meter, once it stated one.
    usage: Option<(u32, u32)>,
    terminated: bool,
    failed: bool,
    on_end: Option<StreamAccounting>,
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
            blocks: Vec::new(),
            open_block: None,
            tool_blocks: std::collections::HashMap::new(),
            stop_reason: None,
            usage: None,
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

    fn message_start(&self) -> Event {
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
                    "usage": { "input_tokens": NOTHING_COUNTED_YET, "output_tokens": NOTHING_COUNTED_YET },
                },
            }),
        )
    }

    /// Close whatever block is open, if one is, and remember the event.
    fn close_open_block(&mut self) {
        if let Some(index) = self.open_block.take() {
            self.pending.push_back(sse(
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": index }),
            ));
        }
    }

    /// Open a new block of `kind` and return its index.
    fn open_block_of(&mut self, kind: AnthropicBlock) -> usize {
        self.close_open_block();
        let index = self.blocks.len();
        self.blocks.push(kind);
        self.open_block = Some(index);
        index
    }

    /// The open text block, opened now when the open block is not one, and
    /// its index.
    fn open_text_block(&mut self) -> usize {
        if let Some(index) = self.open_block {
            if self.blocks.get(index) == Some(&AnthropicBlock::Text) {
                return index;
            }
        }
        let index = self.open_block_of(AnthropicBlock::Text);
        self.pending.push_back(sse(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": { "type": "text", "text": "" },
            }),
        ));
        index
    }

    /// End the stream as failed: the caller's transport closes without
    /// `message_stop`, which is how this format says the message is cut.
    fn fail(&mut self, cause: &str) {
        tracing::warn!(
            event = "stream_failed_mid_flight",
            format = "anthropic-messages",
            error = %cause,
            "provider stream failed after commit; ending without message_stop"
        );
        self.terminated = true;
        self.failed = true;
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
                    let index = self.open_text_block();
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
                    let block_index = self.open_block_of(AnthropicBlock::ToolUse);
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
                    // Arguments belong to the block their call opened; a
                    // fragment for a call never started ends the stream
                    // instead of landing in whichever block is open.
                    let Some(block_index) = self.tool_blocks.get(&index).copied() else {
                        self.fail(&format!(
                            "the provider sent arguments for tool call {index}, which it never started"
                        ));
                        continue;
                    };
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
                    self.stop_reason = Some(anthropic_stop_reason(&reason));
                }
                StreamItem::Delta(StreamDelta::Usage {
                    input_tokens,
                    output_tokens,
                }) => {
                    self.usage = Some((input_tokens, output_tokens));
                }
                StreamItem::Done => {
                    self.close_open_block();
                    // The provider's own reason, or null when it stated none;
                    // the meter only when it stated one.
                    let mut delta = json!({
                        "type": "message_delta",
                        "delta": { "stop_reason": self.stop_reason, "stop_sequence": Value::Null },
                    });
                    if let Some((input_tokens, output_tokens)) = self.usage {
                        delta["usage"] =
                            json!({ "input_tokens": input_tokens, "output_tokens": output_tokens });
                    }
                    self.pending.push_back(sse("message_delta", delta));
                    self.pending
                        .push_back(sse("message_stop", json!({ "type": "message_stop" })));
                    self.terminated = true;
                }
                StreamItem::Failed(message) => self.fail(&message),
            }
        }
    }
}
