//! The two providers that are neither an ordinary API-key chat vendor nor a
//! subscription: the one that answers typed decisions and generates no text,
//! and this deployment's own model server.

use super::super::{AuthKind, ProviderDescriptor, WireProtocol};

/// The one provider here that generates no text. TypeSafe AI's System One
/// model answers a state and a set of typed questions with typed answers, so
/// it serves `POST /v1/decisions` and nothing else: `chat_path` is empty
/// because a chat completion is not a shape this provider has, and a caller
/// that names one of its routes on a chat endpoint is refused rather than
/// sent to an endpoint that does not exist.
pub(super) const TYPESAFE: ProviderDescriptor = ProviderDescriptor {
    id: "typesafe",
    display_name: "TypeSafe AI",
    base_url: "https://api.typesafe.ai",
    models_path: "/v1/models",
    chat_path: "",
    decision_path: "/v1/systemone",
    image_path: "",
    video_path: "",
    video_status_path: "",
    speech_path: "",
    wire: WireProtocol::TypeSafeSystemOne,
    auth: AuthKind::Bearer,
    static_models: &["jev-latest", "jev-preview"],
};

/// The local model server is authenticated. `AuthKind::None` was true when
/// this meant an unguarded loopback process; Stado now deploys vLLM with
/// `--api-key` and publishes the endpoint on the tailnet, and it mints the
/// bearer for exactly this caller (`provider:local-openai#token`, the item
/// the operator's capability route already names). With `None` the broker
/// returned an empty secret without reading anything and no Authorization
/// header was sent, so on 2026-09-05 every product chat took HTTP 401 from
/// the model host while the same request carrying that token answered in
/// 0.3 s — and nothing in the gateway logged a credential step at all,
/// because none happened.
pub(super) const LOCAL_OPENAI: ProviderDescriptor = ProviderDescriptor {
    id: "local-openai",
    display_name: "Local OpenAI",
    base_url: "http://127.0.0.1",
    models_path: "/v1/models",
    chat_path: "/v1/chat/completions",
    decision_path: "",
    image_path: "",
    video_path: "",
    video_status_path: "",
    speech_path: "",
    wire: WireProtocol::OpenAiChat,
    auth: AuthKind::Bearer,
    static_models: &[],
};
