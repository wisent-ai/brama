//! One routed answer: the generation, or the one refusal that stands in its
//! place. A failure holds its class and its sentence together, so no reader
//! meets a failure without either and invents one.
//!
//! It is an in-process value: every wire shape a caller reads is built from
//! it by the encoders under `core::wire`, so it carries no serialized form.

use super::{GatewayRefusal, LimitReading, ProviderRefusal, Refusal, ToolCall};

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub content: String,
    pub model: String,
    /// Zero unless `usage_stated`: a count the provider did not state is no
    /// measurement, and readers that total tokens skip such an answer and
    /// count it apart instead of adding its zeros.
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// Whether the provider stated this answer's token usage.
    pub usage_stated: bool,
    pub latency_ms: f64,
    pub cost: f64,
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
    /// The provider's answer to one HTTP call. A subscription call states no
    /// price, so its cost is none; `usage` is the provider's stated
    /// (input, output, stated) token reading.
    pub fn answered(
        model: &str,
        content: String,
        usage: (u32, u32, bool),
        latency_ms: f64,
        tool_calls: Option<Vec<ToolCall>>,
    ) -> Self {
        let (input_tokens, output_tokens, usage_stated) = usage;
        Self {
            content,
            model: model.to_string(),
            input_tokens,
            output_tokens,
            usage_stated,
            latency_ms,
            cost: Default::default(),
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
            // A failed answer carries no usage: usage_stated says none was stated.
            input_tokens: Default::default(),
            output_tokens: Default::default(),
            usage_stated: false,
            latency_ms: Default::default(),
            cost: Default::default(),
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
