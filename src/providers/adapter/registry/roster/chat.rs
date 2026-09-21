//! The API-key providers that generate text and nothing else. Each is paid by
//! the deployment's own long-lived key, redeemed at final use, and each speaks
//! either the OpenAI chat wire or Anthropic's messages wire.

use super::super::{AuthKind, ProviderDescriptor, WireProtocol};

/// Every entry below differs from its neighbours in four places at most — id,
/// display name, base URL and the paths — so the shared shape is written once
/// here and each declaration fills only what is its own.
const fn openai_chat(
    id: &'static str,
    display_name: &'static str,
    base_url: &'static str,
    models_path: &'static str,
    chat_path: &'static str,
    static_models: &'static [&'static str],
) -> ProviderDescriptor {
    ProviderDescriptor {
        id,
        display_name,
        base_url,
        models_path,
        chat_path,
        decision_path: "",
        image_path: "",
        video_path: "",
        video_status_path: "",
        speech_path: "",
        wire: WireProtocol::OpenAiChat,
        auth: AuthKind::Bearer,
        static_models,
    }
}

pub(super) const ANTHROPIC: ProviderDescriptor = ProviderDescriptor {
    id: "anthropic",
    display_name: "Anthropic",
    base_url: "https://api.anthropic.com",
    models_path: "/v1/models",
    chat_path: "/v1/messages",
    decision_path: "",
    image_path: "",
    video_path: "",
    video_status_path: "",
    speech_path: "",
    wire: WireProtocol::AnthropicMessages,
    auth: AuthKind::XApiKey,
    static_models: &["claude-haiku-4-5", "claude-opus-4-6", "claude-sonnet-4-6"],
};

pub(super) const OPENROUTER: ProviderDescriptor = openai_chat(
    "openrouter",
    "OpenRouter",
    "https://openrouter.ai/api",
    "/v1/models",
    "/v1/chat/completions",
    &[],
);

pub(super) const MISTRAL: ProviderDescriptor = openai_chat(
    "mistral",
    "Mistral",
    "https://api.mistral.ai",
    "/v1/models",
    "/v1/chat/completions",
    &[],
);

pub(super) const DEEPSEEK: ProviderDescriptor = openai_chat(
    "deepseek",
    "DeepSeek",
    "https://api.deepseek.com",
    "/models",
    "/chat/completions",
    &["deepseek-chat", "deepseek-reasoner"],
);

pub(super) const CEREBRAS: ProviderDescriptor = openai_chat(
    "cerebras",
    "Cerebras",
    "https://api.cerebras.ai",
    "/v1/models",
    "/v1/chat/completions",
    &[],
);

pub(super) const FIREWORKS: ProviderDescriptor = openai_chat(
    "fireworks",
    "Fireworks",
    "https://api.fireworks.ai/inference",
    "/v1/models",
    "/v1/chat/completions",
    &[],
);

pub(super) const NVIDIA: ProviderDescriptor = openai_chat(
    "nvidia",
    "NVIDIA NIM",
    "https://integrate.api.nvidia.com",
    "/v1/models",
    "/v1/chat/completions",
    &[],
);

pub(super) const MOONSHOT: ProviderDescriptor = openai_chat(
    "moonshot",
    "Moonshot",
    "https://api.moonshot.ai",
    "/v1/models",
    "/v1/chat/completions",
    &[],
);

pub(super) const ZAI: ProviderDescriptor = openai_chat(
    "zai",
    "Z.AI",
    "https://api.z.ai/api/paas",
    "/v4/models",
    "/v4/chat/completions",
    &[],
);

pub(super) const QWEN: ProviderDescriptor = openai_chat(
    "qwen",
    "Qwen",
    "https://dashscope-intl.aliyuncs.com/compatible-mode",
    "/v1/models",
    "/v1/chat/completions",
    &[],
);

pub(super) const HUGGINGFACE: ProviderDescriptor = openai_chat(
    "huggingface",
    "Hugging Face Inference",
    "https://router.huggingface.co",
    "/v1/models",
    "/v1/chat/completions",
    &[],
);

pub(super) const FEATHERLESS: ProviderDescriptor = openai_chat(
    "featherless",
    "Featherless",
    "https://api.featherless.ai",
    "/v1/models",
    "/v1/chat/completions",
    &["TheDrummer/Cydonia-24B-v4.3"],
);

pub(super) const NOVITA: ProviderDescriptor = openai_chat(
    "novita",
    "Novita",
    "https://api.novita.ai/openai",
    "/v1/models",
    "/v1/chat/completions",
    &[],
);

pub(super) const SYNTHETIC: ProviderDescriptor = openai_chat(
    "synthetic",
    "Synthetic",
    "https://api.synthetic.new",
    "/v1/models",
    "/v1/chat/completions",
    &[],
);
