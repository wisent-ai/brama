//! The providers this build can ask for an image, a video or a voice.
//!
//! Each media path below is the one its vendor documents as OpenAI-shaped:
//! `POST /v1/images/generations` for images, `POST /v1/audio/speech` for
//! voice, and for OpenAI also `POST /v1/videos` with `GET /v1/videos/{id}`
//! for the job it starts. A provider whose media API is a different contract
//! is not declared here at all, because half a declaration is what makes a
//! caller's request land on a URL nobody serves.

use super::super::{AuthKind, ProviderDescriptor, WireProtocol};

/// The one provider here that generates in every shape: text, image, video
/// and voice. Video is a job, not an answer — the create call returns an id
/// and a status, and the render is read back from the status path, which is
/// why both halves are declared.
pub(super) const OPENAI: ProviderDescriptor = ProviderDescriptor {
    id: "openai",
    display_name: "OpenAI",
    base_url: "https://api.openai.com",
    models_path: "/v1/models",
    chat_path: "/v1/chat/completions",
    decision_path: "",
    image_path: "/v1/images/generations",
    video_path: "/v1/videos",
    video_status_path: "/v1/videos/{id}",
    speech_path: "/v1/audio/speech",
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::Bearer,
    static_models: &[],
};

pub(super) const XAI: ProviderDescriptor = ProviderDescriptor {
    id: "xai",
    display_name: "xAI",
    base_url: "https://api.x.ai",
    models_path: "/v1/models",
    chat_path: "/v1/chat/completions",
    decision_path: "",
    image_path: "/v1/images/generations",
    video_path: "",
    video_status_path: "",
    speech_path: "",
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::Bearer,
    static_models: &[],
};

pub(super) const TOGETHER: ProviderDescriptor = ProviderDescriptor {
    id: "together",
    display_name: "Together",
    base_url: "https://api.together.xyz",
    models_path: "/v1/models",
    chat_path: "/v1/chat/completions",
    decision_path: "",
    image_path: "/v1/images/generations",
    video_path: "",
    video_status_path: "",
    speech_path: "",
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::Bearer,
    static_models: &[],
};

/// Venice publishes OpenAI-compatible image and speech endpoints beside its
/// chat one, under the same `/api` prefix this base URL already carries.
pub(super) const VENICE: ProviderDescriptor = ProviderDescriptor {
    id: "venice",
    display_name: "Venice",
    base_url: "https://api.venice.ai/api",
    models_path: "/v1/models",
    chat_path: "/v1/chat/completions",
    decision_path: "",
    image_path: "/v1/images/generations",
    video_path: "",
    video_status_path: "",
    speech_path: "/v1/audio/speech",
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::Bearer,
    static_models: &[],
};

/// Groq generates no pictures and no video; its media shape is voice, on the
/// same OpenAI-compatible path under the `/openai` prefix this base URL
/// already carries.
pub(super) const GROQ: ProviderDescriptor = ProviderDescriptor {
    id: "groq",
    display_name: "Groq",
    base_url: "https://api.groq.com/openai",
    models_path: "/v1/models",
    chat_path: "/v1/chat/completions",
    decision_path: "",
    image_path: "",
    video_path: "",
    video_status_path: "",
    speech_path: "/v1/audio/speech",
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::Bearer,
    static_models: &[],
};
