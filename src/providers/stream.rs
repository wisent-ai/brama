//! Incremental reads of provider `text/event-stream` bodies.
//!
//! The buffered path reads a whole provider answer into memory before anyone
//! learns a word of it, which is the right shape for a control call and the
//! wrong one for a generation a caller is waiting on. This module is the
//! streaming half of that boundary: it parses each provider's SSE wire once,
//! here, and hands dispatchers a single event vocabulary so no caller ever
//! branches on which provider answered.
//!
//! One rule decides everything downstream: a stream is either committed or it
//! is not, and the boundary is the first event. [`crate::providers::adapter`]
//! returns a stream only after the provider answered with success status, so a
//! failure that still permits rotation surfaces as an ordinary
//! [`crate::types::ModelResponse`] failure and never reaches here. Everything
//! after that is committed -- bytes may already be with the caller -- and a
//! mid-stream failure is reported as [`StreamItem::Failed`], never retried by
//! this process.
//!
//! What stays in this file is the pump: reading one committed body to its end
//! and owing the caller exactly one terminal item. `event` holds the
//! vocabulary the pump delivers, `frame` turns arriving bytes into whole SSE
//! events, and `openai_chat`, `anthropic_messages` and `openai_responses` each
//! hold one provider's own rules for what a framed event means -- so a wire
//! that renames its events is one file, and this loop never learns which
//! provider it is reading beyond the single match below.

mod anthropic_messages;
mod event;
mod frame;
mod openai_chat;
mod openai_responses;

use tokio::sync::mpsc;

use super::adapter::WireProtocol;

use anthropic_messages::{anthropic_items, AnthropicState};
use frame::SseFramer;
use openai_chat::openai_chat_items;
use openai_responses::responses_items;

pub use event::{ProviderStream, StreamDelta, StreamItem};

/// How long the pump waits for the next provider byte before calling the
/// stream stalled.
///
/// This is the per-attempt limit the buffered contract already states, applied
/// between reads rather than across the whole body, because a generation is
/// legitimately silent for minutes while it thinks and a total budget cannot
/// tell that from a dead socket.
const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(255);

/// Read one committed provider SSE body to its end and deliver neutral items.
///
/// The task owns the response, so dropping the receiver -- the caller hung up
/// -- fails the next send, exits the task, and drops the in-flight provider
/// future with it. A provider that goes silent longer than
/// [`STREAM_IDLE_TIMEOUT`] between bytes is a stalled stream, reported as
/// `Failed` like any other mid-stream cut.
pub(crate) fn spawn(wire: WireProtocol, response: reqwest::Response) -> mpsc::Receiver<StreamItem> {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(async move {
        use futures_util::StreamExt as _;
        let mut framer = SseFramer::new();
        let mut anthropic = AnthropicState::default();
        let mut saw_tool_call = std::collections::HashSet::new();
        let mut terminal = false;
        let mut bytes = std::pin::pin!(response.bytes_stream());
        loop {
            let next = tokio::time::timeout(STREAM_IDLE_TIMEOUT, bytes.next()).await;
            let chunk = match next {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(error))) => {
                    let _ = tx
                        .send(StreamItem::Failed(format!(
                            "provider stream read failed: {error}"
                        )))
                        .await;
                    return;
                }
                Ok(None) => break,
                Err(_) => {
                    let _ = tx
                        .send(StreamItem::Failed(
                            "provider stream stalled between events".to_string(),
                        ))
                        .await;
                    return;
                }
            };
            let events = match framer.feed(&chunk) {
                Ok(events) => events,
                Err(message) => {
                    let _ = tx.send(StreamItem::Failed(message)).await;
                    return;
                }
            };
            for (event, data) in events {
                let (items, done) = match wire {
                    WireProtocol::OpenAiChat => openai_chat_items(&data, &mut saw_tool_call),
                    WireProtocol::AnthropicMessages => {
                        anthropic_items(event.as_deref(), &data, &mut anthropic)
                    }
                    WireProtocol::OpenAiResponses => responses_items(event.as_deref(), &data),
                };
                for item in items {
                    let failed = matches!(item, StreamItem::Failed(_));
                    if tx.send(item).await.is_err() {
                        return;
                    }
                    if failed {
                        return;
                    }
                }
                if done {
                    terminal = true;
                }
            }
        }
        if !terminal {
            let _ = tx
                .send(StreamItem::Failed(
                    "provider stream ended before a terminal event".to_string(),
                ))
                .await;
            return;
        }
        let _ = tx.send(StreamItem::Done).await;
    });
    rx
}
