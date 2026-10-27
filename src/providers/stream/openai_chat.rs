//! What the OpenAI chat-completions event stream says.
//!
//! This wire has rules no other provider shares: one `choices/0/delta` object
//! per chunk, a tool call's `index` repeated on every fragment while `id` and
//! `name` come only with the first, a meter that may ride along on any chunk,
//! and a plain `[DONE]` sentinel instead of a typed terminal event. They are
//! read here alone, so correcting Brama's reading of this wire cannot disturb
//! another provider's.

use serde_json::Value;

use super::event::{
    json_u32, stream_error, tool_call_index, tool_call_start, StopReason, StreamDelta, StreamItem,
};

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
    // A chunk that is not JSON ends the stream with the chunk named, so the
    // text or tool call it carries never vanishes from an answer that still
    // finishes as complete.
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
        items.push(StreamItem::Failed(stream_error(
            error.get("message"),
            &body,
        )));
        return (items, true);
    }
    // Usage is passed on only when the provider stated both counts, as on
    // the Responses wire; a missing count is no measurement of zero, and a
    // chunk carrying `usage: null` states none.
    match (
        json_u32(body.pointer("/usage/prompt_tokens"), "usage.prompt_tokens"),
        json_u32(
            body.pointer("/usage/completion_tokens"),
            "usage.completion_tokens",
        ),
    ) {
        (Ok(Some(input_tokens)), Ok(Some(output_tokens))) => {
            items.push(StreamItem::Delta(StreamDelta::Usage {
                input_tokens,
                output_tokens,
            }));
        }
        (Err(cause), _) | (_, Err(cause)) => {
            items.push(StreamItem::Failed(cause));
            return (items, true);
        }
        _ => {}
    }
    let Some(choice) = body.pointer("/choices/0") else {
        return (items, false);
    };
    let Some(delta) = choice.get("delta") else {
        items.push(StreamItem::Failed(format!(
            "provider chat stream choice carries no delta: {choice}"
        )));
        return (items, true);
    };
    if let Some(text) = delta.get("content").and_then(Value::as_str) {
        if !text.is_empty() {
            items.push(StreamItem::Delta(StreamDelta::Text(text.to_string())));
        }
    }
    if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            // A fragment without an index, or a first fragment without an id
            // or name, ends the stream naming what it lacked, so no call is
            // appended to the wrong one or started that nobody could run.
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
            reason: chat_stop_reason(reason),
        }));
    }
    (items, false)
}

/// The chat wire's `finish_reason` as a stop reason: the values its chunk
/// object documents (https://platform.openai.com/docs/api-reference/chat-streaming/streaming),
/// and any other exactly as the provider stated it.
fn chat_stop_reason(stated: &str) -> StopReason {
    match stated {
        "stop" => StopReason::EndTurn,
        "length" => StopReason::MaxTokens,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "content_filter" => StopReason::ContentFilter,
        other => StopReason::Other(other.to_string()),
    }
}
