//! The providers this build can ask for an image, a video, a voice or music.
//!
//! The OpenAI-shaped vendors document `POST /v1/images/generations` for
//! images, `POST /v1/audio/speech` for voice, and for OpenAI also
//! `POST /v1/videos` with `GET /v1/videos/{id}` for the job it starts. The
//! vendors whose media API is a different contract carry a [`MediaWire`]
//! naming that contract, and only the paths that vendor documents: a path
//! nobody serves is what turns a refusal the caller could act on into
//! somebody else's 404.

use super::super::{AuthKind, MediaWire, ProviderDescriptor, WireProtocol};

/// The one provider here that generates in every OpenAI shape: text, image,
/// video and voice. Video is a job, not an answer — the create call returns
/// an id and a status, and the render is read back from the status path,
/// which is why both halves are declared.
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
    music_path: "",
    voices_path: "",
    media_wire: MediaWire::OpenAi,
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
    music_path: "",
    voices_path: "",
    media_wire: MediaWire::OpenAi,
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
    music_path: "",
    voices_path: "",
    media_wire: MediaWire::OpenAi,
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
    music_path: "",
    voices_path: "",
    media_wire: MediaWire::OpenAi,
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
    music_path: "",
    voices_path: "",
    media_wire: MediaWire::OpenAi,
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::Bearer,
    static_models: &[],
};

/// ElevenLabs speaks one text in one voice on
/// `POST /v1/text-to-speech/{voice}`, lists the account's voices on
/// `GET /v1/voices` and clones one from samples on `POST /v1/voices/add`.
pub(super) const ELEVENLABS: ProviderDescriptor = ProviderDescriptor {
    id: "elevenlabs",
    display_name: "ElevenLabs",
    base_url: "https://api.elevenlabs.io",
    models_path: "/v1/models",
    chat_path: "",
    decision_path: "",
    image_path: "",
    video_path: "",
    video_status_path: "",
    speech_path: "/v1/text-to-speech/{voice}",
    music_path: "",
    voices_path: "/v1/voices",
    media_wire: MediaWire::ElevenLabs,
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::XiApiKey,
    static_models: &[
        "eleven_multilingual_v2",
        "eleven_v3",
        "eleven_flash_v2_5",
        "eleven_turbo_v2_5",
    ],
};

/// MiniMax speaks on `POST /v1/t2a_v2` and composes a song from lyrics on
/// `POST /v1/music_generation`; both answer the audio hex-encoded in JSON.
pub(super) const MINIMAX: ProviderDescriptor = ProviderDescriptor {
    id: "minimax",
    display_name: "MiniMax",
    base_url: "https://api.minimax.io",
    models_path: "/v1/models",
    chat_path: "",
    decision_path: "",
    image_path: "",
    video_path: "",
    video_status_path: "",
    speech_path: "/v1/t2a_v2",
    music_path: "/v1/music_generation",
    voices_path: "",
    media_wire: MediaWire::MiniMax,
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::Bearer,
    static_models: &[
        "speech-2.8-hd",
        "speech-2.8-turbo",
        "speech-02-hd",
        "speech-02-turbo",
        "music-3.0",
        "music-2.6",
    ],
};

/// Gemini's image models answer `generateContent`, taking the prompt and any
/// input images as parts and answering the picture as inline data. Its chat
/// models are reached through the catalogue's `google` provider; this
/// declaration serves images only.
pub(super) const GEMINI: ProviderDescriptor = ProviderDescriptor {
    id: "gemini",
    display_name: "Gemini (images)",
    base_url: "https://generativelanguage.googleapis.com",
    models_path: "/v1beta/models",
    chat_path: "",
    decision_path: "",
    image_path: "/v1beta/models/{model}:generateContent",
    video_path: "",
    video_status_path: "",
    speech_path: "",
    music_path: "",
    voices_path: "",
    media_wire: MediaWire::Gemini,
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::GoogleApiKey,
    static_models: &["gemini-2.5-flash-image"],
};

/// BytePlus ModelArk serves Seedream on the OpenAI image path, with input
/// images carried in its own `image` field.
pub(super) const BYTEPLUS: ProviderDescriptor = ProviderDescriptor {
    id: "byteplus",
    display_name: "BytePlus ModelArk",
    base_url: "https://ark.ap-southeast.bytepluses.com/api/v3",
    models_path: "/models",
    chat_path: "",
    decision_path: "",
    image_path: "/images/generations",
    video_path: "",
    video_status_path: "",
    speech_path: "",
    music_path: "",
    voices_path: "",
    media_wire: MediaWire::OpenAi,
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::Bearer,
    static_models: &["seedream-4-0-250828"],
};
