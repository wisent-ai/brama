//! The provider declarations this build ships with, in one list.
//!
//! Each declaration is a named constant beside its neighbours of the same
//! kind: [`media`] for the providers that generate images or video, [`chat`]
//! for the API-key providers that generate text, [`subscription`] for the
//! three paid by an account's own OAuth grant, and [`special`] for the two
//! that are neither — the decision-only provider and this deployment's own
//! model server.
//!
//! `image_path`, `video_path` and `video_status_path` are written only where
//! the vendor documents that exact endpoint. A provider whose media API is
//! not OpenAI-shaped names its contract in `media_wire`, so the one adapter
//! that speaks it translates the request instead of posting the OpenAI shape
//! to a server that has no route for it.

mod chat;
mod media;
mod special;
mod subscription;

use super::ProviderDescriptor;

pub(super) const PROVIDERS: &[ProviderDescriptor] = &[
    chat::ANTHROPIC,
    subscription::CLAUDE_CODE,
    subscription::KIMI,
    media::OPENAI,
    subscription::CODEX,
    chat::OPENROUTER,
    media::GROQ,
    chat::MISTRAL,
    media::XAI,
    chat::DEEPSEEK,
    chat::CEREBRAS,
    chat::FIREWORKS,
    media::TOGETHER,
    chat::NVIDIA,
    chat::MOONSHOT,
    chat::ZAI,
    chat::QWEN,
    chat::HUGGINGFACE,
    chat::FEATHERLESS,
    media::VENICE,
    chat::NOVITA,
    chat::SYNTHETIC,
    special::TYPESAFE,
    special::LOCAL_OPENAI,
    media::ELEVENLABS,
    media::MINIMAX,
    media::GEMINI,
    media::BYTEPLUS,
];
