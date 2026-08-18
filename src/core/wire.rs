//! Native wire formats at the ingress: Anthropic Messages and OpenAI
//! Responses, translated once here and nowhere else.
//!
//! [`super::server`] speaks OpenAI chat completions natively; this module is
//! what lets a client that speaks the other two first-party formats reach the
//! same routing decision without a protocol shim in between. The translation
//! is deliberately total and lossy in one direction each way: inbound, only
//! what a [`ModelRequest`] can hold is kept (stop sequences, cache-control
//! hints and provider-side tool types have no home there and are dropped, not
//! invented); outbound, only what the provider actually said is emitted (a
//! stream that produced no usage chunk produces no usage numbers).
//!
//! Streaming encoders follow the same rule the OpenAI one does: a provider
//! failure after the first byte ends the stream without its terminal event --
//! `message_stop`, `response.completed` -- because the generation the caller
//! holds is incomplete and no later event may pretend otherwise.

use serde_json::{json, Value};

use crate::providers::stream::{StreamDelta, StreamItem};
use crate::types::{Message, ModelRequest, ModelResponse, Tool, ToolCall, ToolFunction};
use axum::response::sse::Event;

/// What an encoder reports once, when its stream ends: the provider's own
/// token meters and whether the generation completed.
///
/// The encoders live here, but process statistics belong to the HTTP layer
/// that owns them, so the layer hands each stream a closure instead of this
/// module reaching for a static it does not own.
pub type StreamAccounting = Box<dyn FnMut(u32, u32, bool) + Send>;

/// One parsed inbound call, format already erased.
pub struct InboundCall {
    pub model: String,
    pub request: ModelRequest,
    pub stream: bool,
}

fn default_temperature() -> f64 {
    0.7
}

fn default_max_tokens() -> u32 {
    1024
}

/// Shared inbound bounds, identical to the chat-completions contract so the
/// answer does not depend on which format the caller speaks.
fn validate(model: &str, max_tokens: u32, temperature: f64) -> Result<(), String> {
    if model.trim().is_empty() {
        return Err("missing field `model`".to_string());
    }
    if max_tokens == u32::default() || max_tokens > 32_768 {
        return Err("max_tokens must be between one and 32768".to_string());
    }
    if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
        return Err("temperature must be finite and between zero and 2".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Anthropic Messages, inbound.
// ---------------------------------------------------------------------------

/// Turn one Anthropic content block list into OpenAI-shaped message fields.
///
/// Returns `(content, tool_calls, tool_messages)`: the text-and-image content
/// of this message, the tool calls an assistant made in it, and the tool
/// results it carries -- Anthropic puts `tool_result` blocks inside a `user`
/// message, while the internal shape gives each result its own `tool` role
/// message, appended after this one.
fn anthropic_content_in(
    content: &Value,
) -> (Value, Option<Vec<Value>>, Vec<Message>) {
    let mut text = String::new();
    let mut parts: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_messages: Vec<Message> = Vec::new();
    let blocks: Vec<Value> = match content {
        Value::String(plain) => {
            return (Value::String(plain.clone()), None, Vec::new());
        }
        Value::Array(blocks) => blocks.clone(),
        _ => return (Value::String(String::new()), None, Vec::new()),
    };
    for block in &blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let value = block.get("text").and_then(Value::as_str).unwrap_or_default();
                text.push_str(value);
                parts.push(json!({ "type": "text", "text": value }));
            }
            Some("image") => {
                let source = block.get("source").cloned().unwrap_or(Value::Null);
                if source.get("type").and_then(Value::as_str) == Some("base64") {
                    let media_type = source
                        .get("media_type")
                        .and_then(Value::as_str)
                        .unwrap_or("image/png");
                    let data = source.get("data").and_then(Value::as_str).unwrap_or_default();
                    parts.push(json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{media_type};base64,{data}") },
                    }));
                }
            }
            Some("tool_use") => {
                tool_calls.push(json!({
                    "id": block.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "type": "function",
                    "function": {
                        "name": block.get("name").and_then(Value::as_str).unwrap_or_default(),
                        "arguments": block
                            .get("input")
                            .map(|input| serde_json::to_string(input).unwrap_or_default())
                            .unwrap_or_default(),
                    },
                }));
            }
            Some("tool_result") => {
                let result_text = match block.get("content") {
                    Some(Value::String(plain)) => plain.clone(),
                    Some(Value::Array(blocks)) => blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(""),
                    _ => String::new(),
                };
                tool_messages.push(Message {
                    role: "tool".to_string(),
                    content: Value::String(result_text),
                    tool_call_id: block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    name: None,
                    tool_calls: None,
                });
            }
            _ => {}
        }
    }
    let has_images = parts
        .iter()
        .any(|part| part.get("type").and_then(Value::as_str) == Some("image_url"));
    let content = if has_images {
        Value::Array(parts)
    } else {
        Value::String(text)
    };
    let tool_calls = if tool_calls.is_empty() {
        None
    } else {
        Some(tool_calls)
    };
    (content, tool_calls, tool_messages)
}

/// Parse one Anthropic Messages request into the internal call shape.
///
/// `max_tokens` is required by the format and bounded by the same contract the
/// chat endpoint enforces. `metadata`, `stop_sequences`, `cache_control` and
/// `thinking` blocks are accepted and dropped: the internal request has no
/// field that could hold them honestly.
pub fn anthropic_request(body: &[u8]) -> Result<InboundCall, String> {
    let raw: Value =
        serde_json::from_slice(body).map_err(|error| format!("invalid JSON: {error}"))?;
    let model = raw
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let max_tokens = raw
        .get("max_tokens")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_default();
    let temperature = raw
        .get("temperature")
        .and_then(Value::as_f64)
        .unwrap_or_else(default_temperature);
    validate(&model, max_tokens, temperature)?;
    let system = match raw.get("system") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(blocks)) => {
            let joined = blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        _ => None,
    };
    let mut messages = Vec::new();
    for raw_message in raw
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let role = raw_message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
            .to_string();
        let (content, tool_calls, tool_messages) =
            anthropic_content_in(raw_message.get("content").unwrap_or(&Value::Null));
        messages.push(Message {
            role,
            content,
            tool_call_id: None,
            name: None,
            tool_calls,
        });
        messages.extend(tool_messages);
    }
    if messages.is_empty() {
        return Err("messages must not be empty".to_string());
    }
    let tools = raw
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter(|tool| tool.get("name").and_then(Value::as_str).is_some())
                .map(|tool| Tool {
                    tool_type: "function".to_string(),
                    function: ToolFunction {
                        name: tool
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        description: tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        parameters: tool.get("input_schema").cloned(),
                    },
                })
                .collect::<Vec<_>>()
        })
        .filter(|tools: &Vec<Tool>| !tools.is_empty());
    let tool_choice = raw.get("tool_choice").and_then(|choice| {
        match choice.get("type").and_then(Value::as_str) {
            Some("auto") => Some(json!("auto")),
            Some("any") => Some(json!("required")),
            Some("none") => Some(json!("none")),
            Some("tool") => Some(json!({
                "type": "function",
                "function": { "name": choice.get("name").and_then(Value::as_str).unwrap_or_default() },
            })),
            _ => None,
        }
    });
    Ok(InboundCall {
        model,
        request: ModelRequest {
            messages,
            model: String::new(),
            max_tokens,
            temperature,
            system,
            tools,
            tool_choice,
            billing_target: None,
        },
        stream: raw
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------
// OpenAI Responses, inbound.
// ---------------------------------------------------------------------------

/// Parse one OpenAI Responses request into the internal call shape.
///
/// `store`, `previous_response_id`, `reasoning`, `include` and non-function
/// tool types are accepted and dropped: Brama is stateless across calls and
/// the internal request has no field for them. `instructions` maps to the
/// system slot, matching how the buffered adapter already translates it back.
pub fn responses_request(body: &[u8]) -> Result<InboundCall, String> {
    let raw: Value =
        serde_json::from_slice(body).map_err(|error| format!("invalid JSON: {error}"))?;
    let model = raw
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let max_tokens = raw
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(default_max_tokens);
    let temperature = raw
        .get("temperature")
        .and_then(Value::as_f64)
        .unwrap_or_else(default_temperature);
    validate(&model, max_tokens, temperature)?;
    let system = raw
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut messages = Vec::new();
    match raw.get("input") {
        Some(Value::String(text)) => messages.push(Message {
            role: "user".to_string(),
            content: Value::String(text.clone()),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }),
        Some(Value::Array(items)) => {
            for item in items {
                match item.get("type").and_then(Value::as_str).unwrap_or("message") {
                    "message" => {
                        let role = item
                            .get("role")
                            .and_then(Value::as_str)
                            .unwrap_or("user")
                            .to_string();
                        let content = match item.get("content") {
                            Some(Value::String(text)) => Value::String(text.clone()),
                            Some(Value::Array(parts)) => {
                                let mapped: Vec<Value> = parts
                                    .iter()
                                    .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                                        Some("input_text") | Some("output_text") => part
                                            .get("text")
                                            .and_then(Value::as_str)
                                            .map(|text| json!({ "type": "text", "text": text })),
                                        Some("input_image") => part
                                            .get("image_url")
                                            .and_then(Value::as_str)
                                            .map(|url| json!({
                                                "type": "image_url",
                                                "image_url": { "url": url },
                                            })),
                                        _ => None,
                                    })
                                    .collect();
                                if mapped
                                    .iter()
                                    .any(|part| part.get("type").and_then(Value::as_str) == Some("image_url"))
                                {
                                    Value::Array(mapped)
                                } else {
                                    Value::String(
                                        mapped
                                            .iter()
                                            .filter_map(|part| {
                                                part.get("text").and_then(Value::as_str)
                                            })
                                            .collect::<Vec<_>>()
                                            .join(""),
                                    )
                                }
                            }
                            _ => Value::String(String::new()),
                        };
                        messages.push(Message {
                            role,
                            content,
                            tool_call_id: None,
                            name: None,
                            tool_calls: None,
                        });
                    }
                    "function_call" => {
                        messages.push(Message {
                            role: "assistant".to_string(),
                            content: Value::String(String::new()),
                            tool_call_id: None,
                            name: None,
                            tool_calls: Some(vec![json!({
                                "id": item.get("call_id").or_else(|| item.get("id"))
                                    .and_then(Value::as_str).unwrap_or_default(),
                                "type": "function",
                                "function": {
                                    "name": item.get("name").and_then(Value::as_str).unwrap_or_default(),
                                    "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or_default(),
                                },
                            })]),
                        });
                    }
                    "function_call_output" => {
                        messages.push(Message {
                            role: "tool".to_string(),
                            content: Value::String(
                                item.get("output")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            ),
                            tool_call_id: item
                                .get("call_id")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            name: None,
                            tool_calls: None,
                        });
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if messages.is_empty() {
        return Err("input must not be empty".to_string());
    }
    let tools = raw
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter(|tool| tool.get("type").and_then(Value::as_str) == Some("function"))
                .map(|tool| Tool {
                    tool_type: "function".to_string(),
                    function: ToolFunction {
                        name: tool
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        description: tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        parameters: tool.get("parameters").cloned(),
                    },
                })
                .collect::<Vec<_>>()
        })
        .filter(|tools: &Vec<Tool>| !tools.is_empty());
    let tool_choice = raw.get("tool_choice").and_then(|choice| match choice {
        Value::String(value) => Some(json!(value)),
        Value::Object(_) if choice.get("type").and_then(Value::as_str) == Some("function") => {
            Some(json!({
                "type": "function",
                "function": { "name": choice.get("name").and_then(Value::as_str).unwrap_or_default() },
            }))
        }
        _ => None,
    });
    Ok(InboundCall {
        model,
        request: ModelRequest {
            messages,
            model: String::new(),
            max_tokens,
            temperature,
            system,
            tools,
            tool_choice,
            billing_target: None,
        },
        stream: raw
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------
// Buffered egress.
// ---------------------------------------------------------------------------

fn tool_calls_from_response(response: &ModelResponse) -> Vec<ToolCall> {
    response.tool_calls.clone().unwrap_or_default()
}

/// Shape one buffered internal response as an Anthropic message.
///
/// `stop_reason` is derived, not reported: the internal response does not
/// carry the provider's reason, so a tool call reads as `tool_use` and
/// anything else as `end_turn` -- the two readings a caller can act on.
pub fn anthropic_response(id: &str, model: &str, response: &ModelResponse) -> Value {
    let tool_calls = tool_calls_from_response(response);
    let mut content = Vec::new();
    if !response.content.is_empty() {
        content.push(json!({ "type": "text", "text": response.content }));
    }
    for call in &tool_calls {
        content.push(json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.function.name,
            "input": serde_json::from_str::<Value>(&call.function.arguments)
                .unwrap_or_else(|_| json!({})),
        }));
    }
    json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": if tool_calls.is_empty() { "end_turn" } else { "tool_use" },
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": response.input_tokens,
            "output_tokens": response.output_tokens,
        },
    })
}

/// Shape one buffered internal response as an OpenAI Responses object.
pub fn responses_response(id: &str, model: &str, created: u64, response: &ModelResponse) -> Value {
    let mut output = Vec::new();
    if !response.content.is_empty() {
        output.push(json!({
            "type": "message",
            "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
            "status": "completed",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": response.content, "annotations": [] }],
        }));
    }
    for call in tool_calls_from_response(response) {
        output.push(json!({
            "type": "function_call",
            "id": format!("fc_{}", uuid::Uuid::new_v4().simple()),
            "call_id": call.id,
            "name": call.function.name,
            "arguments": call.function.arguments,
            "status": "completed",
        }));
    }
    json!({
        "id": id,
        "object": "response",
        "created_at": created,
        "status": "completed",
        "model": model,
        "output": output,
        "usage": {
            "input_tokens": response.input_tokens,
            "output_tokens": response.output_tokens,
            "total_tokens": response.input_tokens + response.output_tokens,
        },
    })
}

// ---------------------------------------------------------------------------
// Streaming egress.
// ---------------------------------------------------------------------------

/// Map a neutral finish reason onto the Anthropic stop vocabulary.
fn anthropic_stop_reason(reason: Option<&str>, saw_tool_calls: bool) -> String {
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

fn sse(event: &str, data: Value) -> Event {
    Event::default()
        .event(event)
        .data(serde_json::to_string(&data).unwrap_or_default())
}

/// Which kind of content block the Anthropic encoder currently has open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AnthropicBlock {
    Text,
    ToolUse,
}

/// Encode neutral provider events as an Anthropic Messages event stream.
///
/// Block indices are assigned here, not taken from the provider: the neutral
/// event's index distinguishes concurrent tool calls, and the map below keeps
/// each provider index attached to the block it opened.
pub struct AnthropicEventStream {
    rx: tokio::sync::mpsc::Receiver<StreamItem>,
    pending: std::collections::VecDeque<Event>,
    message_id: String,
    model: String,
    started_message: bool,
    open_block: Option<(u32, AnthropicBlock)>,
    block_count: u32,
    tool_blocks: std::collections::HashMap<u32, u32>,
    saw_tool_calls: bool,
    finish_reason: Option<String>,
    output_tokens: u32,
    input_tokens: u32,
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
    fn account(&mut self, failed: bool) {
        if let Some(mut report) = self.on_end.take() {
            report(self.input_tokens, self.output_tokens, failed);
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
                    "usage": { "input_tokens": 0, "output_tokens": 0 },
                },
            }),
        )
    }

    /// Close whatever block is open, if one is, and remember the event.
    fn close_open_block(&mut self) {
        if let Some((index, _)) = self.open_block.take() {
            self.pending.push_back(sse(
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": index }),
            ));
        }
    }

    fn open_text_block(&mut self) {
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
enum ResponsesOutputItem {
    Message { id: String, text: String },
    FunctionCall { id: String, call_id: String, name: String, arguments: String },
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
