//! What the two media endpoints accept.
//!
//! Both name one model — an alias or a canonical route — and one prompt, and
//! carry the OpenAI-shaped options the vendors agree on. `deny_unknown_fields`
//! is what makes a mistyped option a refusal rather than a dropped intention:
//! a render is billed whether or not the option the caller thought it sent
//! arrived, so being told is worth more than being served.

use serde::Deserialize;
use serde_json::{Map, Value};

/// A prompt is bounded because a provider bills the request either way and a
/// caller that pastes a file into one is going to be told, not charged.
pub(super) const MAX_PROMPT_BYTES: usize = 32_000;
/// One request may ask for a handful of images; a hundred is a mistake with
/// an invoice attached.
pub(super) const MAX_IMAGES: u32 = 10;
/// The longest clip any declared provider renders in one job.
pub(super) const MAX_VIDEO_SECONDS: u32 = 60;
/// Every declared voice provider caps one spoken request at a few thousand
/// characters; a caller sending a chapter is told here rather than billed for
/// the vendor's refusal.
pub(super) const MAX_SPOKEN_BYTES: usize = 4_096;
/// The playback speeds the OpenAI-shaped speech contract accepts.
pub(super) const MIN_SPEED: f32 = 0.25;
pub(super) const MAX_SPEED: f32 = 4.0;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct ImageRequest {
    pub(super) model: String,
    pub(super) prompt: String,
    #[serde(default)]
    pub(super) n: Option<u32>,
    #[serde(default)]
    pub(super) size: Option<String>,
    #[serde(default)]
    pub(super) quality: Option<String>,
    #[serde(default)]
    pub(super) background: Option<String>,
    #[serde(default)]
    pub(super) output_format: Option<String>,
    #[serde(default)]
    pub(super) response_format: Option<String>,
    #[serde(default)]
    pub(super) user: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct VideoRequest {
    pub(super) model: String,
    pub(super) prompt: String,
    #[serde(default)]
    pub(super) size: Option<String>,
    #[serde(default)]
    pub(super) seconds: Option<u32>,
    #[serde(default)]
    pub(super) user: Option<String>,
}

/// A spoken request. `voice` is required by every declared provider — a
/// speech model with no voice named is a 400 from the vendor — so it is
/// required here too, where the caller can still read why.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct SpeechRequest {
    pub(super) model: String,
    pub(super) input: String,
    pub(super) voice: String,
    #[serde(default)]
    pub(super) response_format: Option<String>,
    #[serde(default)]
    pub(super) speed: Option<f32>,
    #[serde(default)]
    pub(super) instructions: Option<String>,
}

impl SpeechRequest {
    pub(super) fn invalid(&self) -> Option<&'static str> {
        if self.input.trim().is_empty() {
            return Some("input must not be empty");
        }
        if self.input.len() > MAX_SPOKEN_BYTES {
            return Some("input is longer than this gateway speaks in one request");
        }
        if self.voice.trim().is_empty() {
            return Some("voice must name one of the provider's voices");
        }
        if self
            .speed
            .is_some_and(|speed| !(MIN_SPEED..=MAX_SPEED).contains(&speed))
        {
            return Some("speed must be between 0.25 and 4.0");
        }
        None
    }

    pub(super) fn payload(self) -> Map<String, Value> {
        let mut payload = Map::new();
        payload.insert("input".to_string(), Value::String(self.input));
        payload.insert("voice".to_string(), Value::String(self.voice));
        if let Some(speed) = self.speed {
            payload.insert(
                "speed".to_string(),
                serde_json::Number::from_f64(f64::from(speed)).map_or(Value::Null, Value::Number),
            );
        }
        stated(&mut payload, "response_format", self.response_format);
        stated(&mut payload, "instructions", self.instructions);
        payload
    }
}

/// One option as the provider takes it, dropped when the caller said nothing.
fn stated(payload: &mut Map<String, Value>, key: &str, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        payload.insert(key.to_string(), Value::String(value));
    }
}

impl ImageRequest {
    pub(super) fn invalid(&self) -> Option<&'static str> {
        if self.prompt.trim().is_empty() {
            return Some("prompt must not be empty");
        }
        if self.prompt.len() > MAX_PROMPT_BYTES {
            return Some("prompt is longer than this gateway accepts");
        }
        if self
            .n
            .is_some_and(|count| count == u32::MIN || count > MAX_IMAGES)
        {
            return Some("n must be between 1 and 10");
        }
        None
    }

    pub(super) fn payload(self) -> Map<String, Value> {
        let mut payload = Map::new();
        payload.insert("prompt".to_string(), Value::String(self.prompt));
        if let Some(count) = self.n {
            payload.insert("n".to_string(), Value::from(count));
        }
        stated(&mut payload, "size", self.size);
        stated(&mut payload, "quality", self.quality);
        stated(&mut payload, "background", self.background);
        stated(&mut payload, "output_format", self.output_format);
        stated(&mut payload, "response_format", self.response_format);
        stated(&mut payload, "user", self.user);
        payload
    }
}

impl VideoRequest {
    pub(super) fn invalid(&self) -> Option<&'static str> {
        if self.prompt.trim().is_empty() {
            return Some("prompt must not be empty");
        }
        if self.prompt.len() > MAX_PROMPT_BYTES {
            return Some("prompt is longer than this gateway accepts");
        }
        if self
            .seconds
            .is_some_and(|seconds| seconds == u32::MIN || seconds > MAX_VIDEO_SECONDS)
        {
            return Some("seconds must be between 1 and 60");
        }
        None
    }

    pub(super) fn payload(self) -> Map<String, Value> {
        let mut payload = Map::new();
        payload.insert("prompt".to_string(), Value::String(self.prompt));
        if let Some(seconds) = self.seconds {
            payload.insert("seconds".to_string(), Value::String(seconds.to_string()));
        }
        stated(&mut payload, "size", self.size);
        stated(&mut payload, "user", self.user);
        payload
    }
}

/// Brama keeps no job state, so a status read that names no model has no
/// provider to ask and the caller is told exactly that.
pub(super) const MODEL_REQUIRED: &str =
    "a video status read names the model that started the job: GET /v1/videos/{id}?model=…";

/// Which route a video status read belongs to.
///
/// The query is read as raw pairs rather than deserialized, because a
/// deserializer's own rejection is plain text: a caller that named nothing
/// would get a body its error handling cannot parse from the endpoint whose
/// whole answer is a job document.
pub(super) fn video_status_model(
    query: std::collections::HashMap<String, String>,
) -> Result<String, &'static str> {
    let mut model = None;
    for (member, value) in query {
        if member != "model" {
            return Err(MODEL_REQUIRED);
        }
        model = Some(value);
    }
    model
        .filter(|value| !value.trim().is_empty())
        .ok_or(MODEL_REQUIRED)
}
