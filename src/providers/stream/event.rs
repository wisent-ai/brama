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

/// Read a wire number as the width this vocabulary counts in.
///
/// Token meters and content indices are `u32` here while JSON carries them as
/// `u64`, so every dialect narrows the same way and a count that would not fit
/// is reported absent rather than wrapped.
pub(super) fn json_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
}
