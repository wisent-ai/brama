//! One routed answer: the generation, or the one refusal that stands in its
//! place. A failure holds its class and its sentence together, so no reader
//! meets a failure without either and invents one.
//!
//! It is an in-process value: every wire shape a caller reads is built from
//! it by the encoders under `core::wire`, so it carries no serialized form.

use super::{GatewayRefusal, LimitReading, ProviderRefusal, Refusal, ToolCall};

/// The tokens a provider stated it read and wrote for one answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl TokenUsage {
    /// Both counts together, wide enough that the sum cannot overflow.
    pub fn total(self) -> u64 {
        u64::from(self.input_tokens) + u64::from(self.output_tokens)
    }
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub content: String,
    pub model: String,
    /// The provider's stated token usage; `None` when it stated none, which
    /// is no measurement: readers that total tokens skip such an answer and
    /// never add a zero for it.
    pub usage: Option<TokenUsage>,
    pub latency_ms: f64,
    /// Number of provider HTTP calls consumed by this routed request.
    pub attempts: u32,
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Plan windows the provider reported on this exact response, when it
    /// reports any. Present on rate-limited responses too, which is when it
    /// matters most.
    pub limits: Vec<LimitReading>,
    /// Why this answer failed: its class, stated where the refusal was built
    /// (the provider's HTTP status where one answered, Brama's own reason
    /// where Brama refused), and the sentence the caller reads. `None` is a
    /// success and nothing else, so no failure exists without both, and no
    /// reader has a missing class or reason to stand in for. Rotation and
    /// the HTTP edge decide from the class, never from the sentence.
    pub refusal: Option<Refusal>,
}

impl ModelResponse {
    /// The provider's answer to one HTTP call; `usage` is the provider's
    /// stated reading.
    pub fn answered(
        model: &str,
        content: String,
        usage: Option<TokenUsage>,
        latency_ms: f64,
        tool_calls: Option<Vec<ToolCall>>,
    ) -> Self {
        Self {
            content,
            model: model.to_string(),
            usage,
            latency_ms,
            // The one call that produced this answer.
            attempts: std::num::NonZeroU32::MIN.get(),
            tool_calls,
            limits: Vec::new(),
            refusal: None,
        }
    }

    /// A failed answer and its class. There is no failure without one: the
    /// edge answers from the class, and a refusal whose class nobody stated
    /// would have to be guessed from its sentence.
    pub fn failure(model: &str, class: ProviderRefusal, error: String) -> Self {
        Self::from_refusal(model, Refusal::new(class, error))
    }

    /// A refusal Brama builds itself, with its class stated.
    pub fn refused(model: &str, class: GatewayRefusal, error: String) -> Self {
        Self::failure(model, ProviderRefusal::Gateway(class), error)
    }

    /// A failed answer carrying one stated refusal.
    pub fn from_refusal(model: &str, refusal: Refusal) -> Self {
        Self {
            content: String::new(),
            model: model.to_string(),
            // A failed answer carries no usage.
            usage: None,
            latency_ms: Default::default(),
            attempts: Default::default(),
            tool_calls: None,
            limits: Vec::new(),
            refusal: Some(refusal),
        }
    }

    /// Whether the provider answered: true exactly when no refusal is held.
    pub fn success(&self) -> bool {
        self.refusal.is_none()
    }

    /// The class of this answer's refusal, `None` on a success.
    pub fn failure_class(&self) -> Option<ProviderRefusal> {
        self.refusal.as_ref().map(|refusal| refusal.class)
    }

    /// The sentence of this answer's refusal, `None` on a success.
    pub fn reason(&self) -> Option<&str> {
        self.refusal.as_ref().map(|refusal| refusal.message.as_str())
    }
}
