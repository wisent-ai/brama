//! The three providers paid by an account's own OAuth grant rather than by a
//! deployment API key. Their wire is the vendor's ordinary one; what differs
//! is the credential, which Weles mints by signing in and Skarbiec holds.

use super::super::{AuthKind, ProviderDescriptor, WireProtocol};

pub(super) const CLAUDE_CODE: ProviderDescriptor = ProviderDescriptor {
    id: "claude-code",
    display_name: "Claude Code (subscription)",
    base_url: "https://api.anthropic.com",
    models_path: "/v1/models",
    chat_path: "/v1/messages",
    decision_path: "",
    image_path: "",
    video_path: "",
    video_status_path: "",
    speech_path: "",
    wire: WireProtocol::AnthropicMessages,
    auth: AuthKind::AnthropicBearer,
    static_models: &["claude-haiku-4-5", "claude-opus-4-6", "claude-sonnet-4-6"],
};

pub(super) const KIMI: ProviderDescriptor = ProviderDescriptor {
    id: "kimi",
    display_name: "Kimi (subscription)",
    base_url: "https://api.kimi.com/coding",
    models_path: "/v1/models",
    chat_path: "/v1/chat/completions",
    decision_path: "",
    image_path: "",
    video_path: "",
    video_status_path: "",
    speech_path: "",
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::Bearer,
    static_models: &["kimi-for-coding"],
};

pub(super) const CODEX: ProviderDescriptor = ProviderDescriptor {
    id: "codex",
    display_name: "Codex (ChatGPT subscription)",
    base_url: "https://chatgpt.com/backend-api/codex",
    models_path: "/models",
    chat_path: "/responses",
    decision_path: "",
    image_path: "",
    video_path: "",
    video_status_path: "",
    speech_path: "",
    wire: WireProtocol::OpenAiResponses,
    auth: AuthKind::Bearer,
    static_models: &[
        "gpt-6-astra",
        "gpt-5.6-sol",
        "gpt-5.6-luna",
        "gpt-5.6-terra",
        "gpt-5.5",
        "gpt-5.3-codex-spark",
    ],
};
