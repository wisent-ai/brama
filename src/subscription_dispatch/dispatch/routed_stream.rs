//! One committed generation stream, and the ledger record its ending writes.

use std::time::Instant;
use tokio::sync::mpsc;

use crate::providers::stream::{ProviderStream, StreamDelta, StreamItem};
use crate::subscription_dispatch::usage;
use crate::types::ModelResponse;

/// One committed, routed generation stream.
///
/// `model` and `attempts` are the facts the HTTP layer needs to finish its own
/// accounting; `events` is already past the rotation boundary -- every item on
/// it belongs to a provider answer that committed, and no item on it will ever
/// be followed by a silent retry on another credential.
pub struct RoutedStream {
    pub model: String,
    pub attempts: u32,
    pub events: mpsc::Receiver<StreamItem>,
}

/// Forward one committed provider stream to the caller's channel, recording
/// the subscription's spend when the stream ends, however it ends.
///
/// The buffered path records from a whole [`ModelResponse`]; a stream has no
/// whole response until it is over, so this task accumulates one. Three
/// endings all write exactly one record: `Done` is the measured generation,
/// `Failed` is a partial generation with the provider's own sentence, and a
/// closed caller channel is a cut the caller asked for -- still spend the
/// subscription paid for, so it is recorded as a failure with what was
/// measured rather than dropped from the ledger entirely.
pub(in crate::subscription_dispatch::dispatch) fn spawn_stream_recorder(
    subscription_id: &str,
    provider: &str,
    model: &str,
    attempts: u32,
    started: Instant,
    stream: ProviderStream,
) -> mpsc::Receiver<StreamItem> {
    let (forward_tx, forward_rx) = mpsc::channel::<StreamItem>(64);
    let subscription_id = subscription_id.to_string();
    let provider = provider.to_string();
    let model = model.to_string();
    let limits = stream.limits;
    let mut events = stream.events;
    tokio::spawn(async move {
        let mut content = String::new();
        let mut input_tokens = u32::default();
        let mut output_tokens = u32::default();
        let mut error: Option<String> = None;
        let mut finished = false;
        loop {
            let item = match events.recv().await {
                Some(item) => item,
                None => break,
            };
            match &item {
                StreamItem::Delta(StreamDelta::Text(text)) => content.push_str(text),
                StreamItem::Delta(StreamDelta::Usage {
                    input_tokens: input,
                    output_tokens: output,
                }) => {
                    input_tokens = *input;
                    output_tokens = *output;
                }
                StreamItem::Failed(message) => error = Some(message.clone()),
                _ => {}
            }
            let terminal = matches!(item, StreamItem::Failed(_) | StreamItem::Done);
            finished = finished || matches!(item, StreamItem::Done);
            if forward_tx.send(item).await.is_err() {
                error.get_or_insert_with(|| "caller disconnected mid-stream".to_string());
                break;
            }
            if terminal {
                break;
            }
        }
        let success = finished && error.is_none();
        let mut response = ModelResponse::failure(
            &model,
            error.unwrap_or_else(|| "provider stream ended without a verdict".to_string()),
        );
        response.success = success;
        if success {
            response.error = None;
        }
        response.content = content;
        response.input_tokens = input_tokens;
        response.output_tokens = output_tokens;
        response.latency_ms = started.elapsed().as_secs_f64() * 1_000.0;
        response.attempts = attempts;
        response.limits = limits;
        usage::record_call(&subscription_id, &provider, &response);
    });
    forward_rx
}
