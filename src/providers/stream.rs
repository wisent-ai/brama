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

use serde_json::Value;
use tokio::sync::mpsc;

use super::adapter::WireProtocol;
use crate::types::LimitReading;

/// One incremental piece of a generation, provider-neutral.
///
/// `index` on the tool-call variants is the provider's own content or output
/// index, so two interleaved tool calls stay two calls. `Usage` is the
/// provider's meter reading and may arrive more than once; the last one wins.
#[derive(Clone, Debug)]
pub enum StreamDelta {
    Text(String),
    ToolCallStart { index: u32, id: String, name: String },
    ToolCallArguments { index: u32, delta: String },
    Finish { reason: Option<String> },
    Usage { input_tokens: u32, output_tokens: u32 },
}

/// What the pump delivers.
///
/// `Done` is sent exactly once, after the provider's own terminal event, and
/// nothing follows it. `Failed` is terminal too, and means the stream was cut
/// or refused after the first byte: the generation the caller holds is
/// incomplete and this process will not add to it.
#[derive(Clone, Debug)]
pub enum StreamItem {
    Delta(StreamDelta),
    Failed(String),
    Done,
}

/// One committed provider generation: the plan windows its headers carried,
/// and the events as they arrive.
pub struct ProviderStream {
    pub limits: Vec<LimitReading>,
    pub events: mpsc::Receiver<StreamItem>,
}

/// How long the pump waits for the next provider byte before calling the
/// stream stalled.
///
/// This is the per-attempt timeout the buffered contract already states,
/// applied between reads rather than across the whole body, because a
/// generation is legitimately silent for minutes while it thinks and a total
/// budget cannot tell that from a dead socket.
const STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(255);

/// The parser's buffered ceiling.
///
/// A provider event is at most a content block; a buffer that will not drain
/// past this is a peer sending an unterminated line, which is a failure, not a
/// large generation.
const MAX_EVENT_BUFFER_BYTES: usize = 1024 * 1024;

/// Line-based SSE framing: events in, complete `(event, data)` pairs out.
///
/// The wire is `event:` and `data:` fields joined by blank lines, with `:`
/// comments interleaved. Data accumulates across repeated `data:` fields of
/// one event, joined by newlines, per the SSE grammar.
struct SseFramer {
    buffer: String,
    event: Option<String>,
    data: Vec<String>,
}

impl SseFramer {
    fn new() -> Self {
        Self {
            buffer: String::new(),
            event: None,
            data: Vec::new(),
        }
    }

    /// Append one received chunk and yield every event it completed.
    fn feed(&mut self, chunk: &[u8]) -> Result<Vec<(Option<String>, String)>, String> {
        if self.buffer.len().saturating_add(chunk.len()) > MAX_EVENT_BUFFER_BYTES {
            return Err("provider stream carried an unterminated event".to_string());
        }
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut events = Vec::new();
        while let Some(line_end) = self.buffer.find('\n') {
            let line = self.buffer[..line_end].trim_end_matches('\r').to_string();
            self.buffer.drain(..=line_end);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push((self.event.take(), self.data.join("\n")));
                    self.data.clear();
                } else {
                    self.event = None;
                }
                continue;
            }
            if let Some(comment) = line.strip_prefix(':') {
                let _ = comment;
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (line.as_str(), ""),
            };
            match field {
                "event" => self.event = Some(value.to_string()),
                "data" => self.data.push(value.to_string()),
                _ => {}
            }
        }
        Ok(events)
    }
}

fn json_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
}

/// The terminal `data: [DONE]` sentinel of the OpenAI chat wire.
const OPENAI_DONE: &str = "[DONE]";

/// Translate one OpenAI chat-completions chunk into neutral items.
///
/// `saw_tool_call` records which streaming indices have already produced a
/// `ToolCallStart`, because this wire repeats `index` on every fragment and
/// emits `id`/`name` only on the first.
fn openai_chat_items(
    data: &str,
    saw_tool_call: &mut std::collections::HashSet<u32>,
) -> (Vec<StreamItem>, bool) {
    let mut items = Vec::new();
    if data.trim() == OPENAI_DONE {
        return (items, true);
    }
    let body: Value = match serde_json::from_str(data) {
        Ok(body) => body,
        Err(_) => return (items, false),
    };
    if let Some(error) = body.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("provider reported a stream error");
        items.push(StreamItem::Failed(message.chars().take(200).collect()));
        return (items, true);
    }
    if let Some(usage) = body.get("usage") {
        let input = json_u32(usage.get("prompt_tokens")).unwrap_or_default();
        let output = json_u32(usage.get("completion_tokens")).unwrap_or_default();
        if input > 0 || output > 0 {
            items.push(StreamItem::Delta(StreamDelta::Usage {
                input_tokens: input,
                output_tokens: output,
            }));
        }
    }
    let Some(choice) = body.pointer("/choices/0") else {
        return (items, false);
    };
    let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
    if let Some(text) = delta.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            items.push(StreamItem::Delta(StreamDelta::Text(text.to_string())));
        }
    }
    if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let index = json_u32(call.get("index")).unwrap_or_default();
            if saw_tool_call.insert(index) {
                items.push(StreamItem::Delta(StreamDelta::ToolCallStart {
                    index,
                    id: call
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: call
                        .pointer("/function/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                }));
            }
            if let Some(arguments) = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
            {
                if !arguments.is_empty() {
                    items.push(StreamItem::Delta(StreamDelta::ToolCallArguments {
                        index,
                        delta: arguments.to_string(),
                    }));
                }
            }
        }
    }
    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        items.push(StreamItem::Delta(StreamDelta::Finish {
            reason: Some(reason.to_string()),
        }));
    }
    (items, false)
}

/// Mutable state for the Anthropic messages wire: the input meter arrives at
/// the start of the stream and the output meter only at its end.
struct AnthropicState {
    input_tokens: u32,
}

/// Translate one Anthropic messages event into neutral items.
fn anthropic_items(
    event: Option<&str>,
    data: &str,
    state: &mut AnthropicState,
) -> (Vec<StreamItem>, bool) {
    let mut items = Vec::new();
    let body: Value = match serde_json::from_str(data) {
        Ok(body) => body,
        Err(_) => return (items, false),
    };
    match event.unwrap_or_else(|| body.get("type").and_then(Value::as_str).unwrap_or("")) {
        "message_start" => {
            state.input_tokens = body
                .pointer("/message/usage/input_tokens")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or_default();
        }
        "content_block_start" => {
            let index = json_u32(body.get("index")).unwrap_or_default();
            let block = body.get("content_block").cloned().unwrap_or(Value::Null);
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                items.push(StreamItem::Delta(StreamDelta::ToolCallStart {
                    index,
                    id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                }));
            }
        }
        "content_block_delta" => {
            let index = json_u32(body.get("index")).unwrap_or_default();
            let delta = body.get("delta").cloned().unwrap_or(Value::Null);
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => {
                    if let Some(text) = delta.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            items.push(StreamItem::Delta(StreamDelta::Text(text.to_string())));
                        }
                    }
                }
                Some("input_json_delta") => {
                    if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                        if !partial.is_empty() {
                            items.push(StreamItem::Delta(StreamDelta::ToolCallArguments {
                                index,
                                delta: partial.to_string(),
                            }));
                        }
                    }
                }
                _ => {}
            }
        }
        "message_delta" => {
            let output = body
                .pointer("/usage/output_tokens")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or_default();
            if state.input_tokens > 0 || output > 0 {
                items.push(StreamItem::Delta(StreamDelta::Usage {
                    input_tokens: state.input_tokens,
                    output_tokens: output,
                }));
            }
            if let Some(reason) = body.pointer("/delta/stop_reason").and_then(Value::as_str) {
                items.push(StreamItem::Delta(StreamDelta::Finish {
                    reason: Some(reason.to_string()),
                }));
            }
        }
        "message_stop" => return (items, true),
        "error" => {
            let message = body
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("provider reported a stream error");
            items.push(StreamItem::Failed(message.chars().take(200).collect()));
            return (items, true);
        }
        _ => {}
    }
    (items, false)
}

/// Translate one OpenAI Responses event into neutral items.
fn responses_items(event: Option<&str>, data: &str) -> (Vec<StreamItem>, bool) {
    let mut items = Vec::new();
    let kind = event
        .map(str::to_string)
        .or_else(|| {
            serde_json::from_str::<Value>(data)
                .ok()
                .and_then(|body| body.get("type").and_then(Value::as_str).map(str::to_string))
        })
        .unwrap_or_default();
    let body: Value = match serde_json::from_str(data) {
        Ok(body) => body,
        Err(_) => return (items, false),
    };
    match kind.as_str() {
        "response.output_text.delta" => {
            if let Some(text) = body.get("delta").and_then(Value::as_str) {
                if !text.is_empty() {
                    items.push(StreamItem::Delta(StreamDelta::Text(text.to_string())));
                }
            }
        }
        "response.output_item.added" => {
            let index = json_u32(body.get("output_index")).unwrap_or_default();
            let item = body.get("item").cloned().unwrap_or(Value::Null);
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                items.push(StreamItem::Delta(StreamDelta::ToolCallStart {
                    index,
                    id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                }));
            }
        }
        "response.function_call_arguments.delta" => {
            let index = json_u32(body.get("output_index")).unwrap_or_default();
            if let Some(delta) = body.get("delta").and_then(Value::as_str) {
                if !delta.is_empty() {
                    items.push(StreamItem::Delta(StreamDelta::ToolCallArguments {
                        index,
                        delta: delta.to_string(),
                    }));
                }
            }
        }
        "response.completed" | "response.incomplete" => {
            let response = body.get("response").cloned().unwrap_or(Value::Null);
            let usage = response.get("usage").cloned().unwrap_or(Value::Null);
            items.push(StreamItem::Delta(StreamDelta::Usage {
                input_tokens: json_u32(usage.get("input_tokens")).unwrap_or_default(),
                output_tokens: json_u32(usage.get("output_tokens")).unwrap_or_default(),
            }));
            let reason = if kind == "response.incomplete" {
                Some(
                    response
                        .pointer("/incomplete_details/reason")
                        .and_then(Value::as_str)
                        .map(|reason| format!("incomplete:{reason}"))
                        .unwrap_or_else(|| "incomplete".to_string()),
                )
            } else {
                Some("stop".to_string())
            };
            items.push(StreamItem::Delta(StreamDelta::Finish { reason }));
            return (items, true);
        }
        "response.failed" | "error" => {
            let message = body
                .pointer("/response/error/message")
                .or_else(|| body.pointer("/error/message"))
                .or_else(|| body.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("provider reported a stream error");
            items.push(StreamItem::Failed(message.chars().take(200).collect()));
            return (items, true);
        }
        _ => {}
    }
    (items, false)
}

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
        let mut anthropic = AnthropicState { input_tokens: 0 };
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
