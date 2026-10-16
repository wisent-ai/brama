//! What a caller of a committed stream sees, whichever provider answered.
//!
//! Every wire module in this directory produces these three types and nothing
//! else, which is the whole point of the streaming boundary: a dispatcher
//! reading a generation never learns whether the bytes came from a chat
//! completion, an Anthropic message or a Responses call. The vocabulary lives
//! apart from the dialects that speak it so that promise stays checkable -- a
//! wire that wants to say something new has to come here and name it.

use serde_json::Value;
use tokio::sync::mpsc;

use crate::types::LimitReading;

/// One incremental piece of a generation, provider-neutral.
///
/// `index` on the tool-call variants is the provider's own content or output
/// index, so two interleaved tool calls stay two calls. `Usage` is the
/// provider's meter reading and may arrive more than once; the last one wins.
#[derive(Clone, Debug)]
pub enum StreamDelta {
    Text(String),
    ToolCallStart {
        index: u32,
        id: String,
        name: String,
    },
    ToolCallArguments {
        index: u32,
        delta: String,
    },
    Finish {
        reason: Option<String>,
    },
    Usage {
        input_tokens: u32,
        output_tokens: u32,
    },
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

/// Read a wire number as the width this vocabulary counts in. An absent
/// meter stays absent; a present malformed or oversized one ends the stream
/// with its field and value rather than disappearing from the reading.
pub(super) fn json_u32(value: Option<&Value>, field: &str) -> Result<Option<u32>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let number = value.as_u64().ok_or_else(|| {
        format!("provider stream field `{field}` is not a nonnegative integer: {value}")
    })?;
    u32::try_from(number).map(Some).map_err(|error| {
        format!(
            "provider stream field `{field}` does not fit a token count or index ({number}): {error}"
        )
    })
}

/// A tool call's position, id and name as one stream event states them, or
/// the failure that ends the stream naming the event and what it lacked.
/// Each wire's own field names are given by its reader. Read as 0 and empty,
/// two calls without an index merged into one, and a call without an id or
/// name reached the caller as a call it could neither answer nor run.
pub(super) fn tool_call_start(
    event: &str,
    index: Option<&Value>,
    id: Option<&Value>,
    name: Option<&Value>,
) -> Result<StreamDelta, String> {
    let index = json_u32(index, "index")?.ok_or_else(|| {
        format!("provider stream event `{event}` starts a tool call with no index")
    })?;
    let text = |value: Option<&Value>, field: &str| {
        value
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                format!("provider stream event `{event}` starts tool call {index} with no {field}")
            })
    };
    Ok(StreamDelta::ToolCallStart {
        index,
        id: text(id, "id")?,
        name: text(name, "name")?,
    })
}

/// The position a tool call's argument fragment belongs to, or the failure
/// that ends the stream: a fragment without one was appended to call 0.
pub(super) fn tool_call_index(event: &str, index: Option<&Value>) -> Result<u32, String> {
    json_u32(index, "index")?.ok_or_else(|| {
        format!("provider stream event `{event}` carries tool call arguments with no index")
    })
}

/// What a provider's stream error says: the message it states, or, when it
/// states none, the event as sent. A fixed sentence in its place dropped
/// whatever the provider did say, so the failure could not be traced.
pub(super) fn stream_error(message: Option<&Value>, event: &Value) -> String {
    match message.and_then(Value::as_str) {
        Some(message) => message.to_string(),
        None => format!("provider reported a stream error without a message: {event}"),
    }
}
