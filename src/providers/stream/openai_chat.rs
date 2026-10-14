//! What the OpenAI chat-completions event stream says.
//!
//! This wire has rules no other provider shares: one `choices/0/delta` object
//! per chunk, a tool call's `index` repeated on every fragment while `id` and
//! `name` come only with the first, a meter that may ride along on any chunk,
//! and a plain `[DONE]` sentinel instead of a typed terminal event. They are
//! read here alone, so correcting Brama's reading of this wire cannot disturb
//! another provider's.

use serde_json::Value;

use super::event::{json_u32, tool_call_index, tool_call_start, StreamDelta, StreamItem};

/// The terminal `data: [DONE]` sentinel of the OpenAI chat wire.
const OPENAI_DONE: &str = "[DONE]";

/// The object type every chunk of this wire declares, named in a refusal.
const CHUNK_EVENT: &str = "chat.completion.chunk";

/// Translate one OpenAI chat-completions chunk into neutral items.
///
/// `saw_tool_call` records which streaming indices have already produced a
/// `ToolCallStart`, because this wire repeats `index` on every fragment and
/// emits `id`/`name` only on the first.
pub(super) fn openai_chat_items(
    data: &str,
    saw_tool_call: &mut std::collections::HashSet<u32>,
) -> (Vec<StreamItem>, bool) {
    let mut items = Vec::new();
    if data.trim() == OPENAI_DONE {
        return (items, true);
    }
    // A chunk that is not JSON ends the stream with the chunk named: dropped,
    // the text or tool call it carried vanished from an answer that still
    // finished as complete.
    let body: Value = match serde_json::from_str(data) {
        Ok(body) => body,
        Err(error) => {
            items.push(StreamItem::Failed(format!(
                "provider stream chunk is not JSON ({error}): {data}"
            )));
            return (items, true);
        }
    };
    if let Some(error) = body.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("provider reported a stream error");
        items.push(StreamItem::Failed(message.to_string()));
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
            // A fragment without an index was appended to call 0, and a first
            // fragment without an id or name started a call nobody could
            // answer or run; either ends the stream naming what it lacked.
            let index = match tool_call_index(CHUNK_EVENT, call.get("index")) {
                Ok(index) => index,
                Err(cause) => {
                    items.push(StreamItem::Failed(cause));
                    return (items, true);
                }
            };
            if saw_tool_call.insert(index) {
                match tool_call_start(
                    CHUNK_EVENT,
                    call.get("index"),
                    call.get("id"),
                    call.pointer("/function/name"),
                ) {
                    Ok(start) => items.push(StreamItem::Delta(start)),
                    Err(cause) => {
                        items.push(StreamItem::Failed(cause));
                        return (items, true);
                    }
                }
            }
            if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str) {
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
