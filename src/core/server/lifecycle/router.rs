//! Every path this gateway answers, in one table.
//!
//! Two layers wrap it and the order is the point: the transport guard is
//! outermost, so an unprotected hop is refused before any credential is read,
//! and the bearer guard sits inside it around everything except `/health` and
//! `/readyz` — the two paths a deployment probe reaches with nothing at all.

use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{delete, get, post, put};
use axum::{Extension, Router};

use crate::core::server::administration::credentials::{
    delete_admin_credential, list_admin_credentials, put_admin_credential,
};
use crate::core::server::administration::registry::adoption::{
    apply_admin_adoption, preview_admin_adoption,
};
use crate::core::server::administration::registry::categories::{
    delete_admin_category, update_admin_category,
};
use crate::core::server::administration::registry::routes::{
    delete_admin_route, update_admin_route,
};
use crate::core::server::administration::snapshot::admin_snapshot;
use crate::core::server::admission::ingress::ModelIngressAuth;
use crate::core::server::admission::{require_model_bearer, transport::require_secure_transport};
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::catalog::models::list_models;
use crate::core::server::catalog::{list_aliases, list_categories};
use crate::core::server::chat::chat_completions;
use crate::core::server::chat::dialects::{anthropic_messages, openai_responses};
use crate::core::server::decisions::decisions;
use crate::core::server::media::{
    audio_music, audio_speech, audio_voice_clone, audio_voice_delete, audio_voices, image_edits,
    image_generations, video_generations, video_status,
};
use crate::core::server::readiness::{health, readyz};
use crate::core::server::subscriptions::leases::{
    list_leases, release_lease, release_leases, take_lease,
};
use crate::core::server::subscriptions::probe::{
    acquire_admin_subscription, attribute_admin_subscription_pool,
    discover_admin_subscription_pool, maintain_admin, probe_admin_subscription,
    refresh_admin_subscription_pool, reset_admin_subscription,
};
use crate::core::server::subscriptions::sign_in::manual::{
    complete_admin_manual_sign_in, disown_admin_grant, reinstate_admin_grant,
};
use crate::core::server::subscriptions::sign_in::{
    sign_in_account_subscription, sign_in_admin_pool_subscription, sign_in_admin_subscription,
};
use crate::core::server::subscriptions::{
    read_plan_usage, read_subscription_pool, write_subscription_pool,
};
use crate::core::server::telemetry::stats::get_stats;
use crate::core::server::typed::{embeddings, moderations};

pub(super) fn app(aliases: ModelAliases, ingress_auth: ModelIngressAuth) -> Router {
    // No route that carries a prompt keeps axum's built-in 2 MB body limit, a
    // size nobody stated: a long prompt is the routed model's to accept or to
    // refuse as context_length_exceeded, which reaches the caller classed and
    // naming the model. Kept, the limit answered a long judge request 413
    // 'Failed to buffer the request body' from the gateway itself.
    let protected = Router::new()
        .route(
            "/v1/chat/completions",
            post(chat_completions).layer(DefaultBodyLimit::disable()),
        )
        // The two first-party formats callers already speak. Same routing
        // decision, same identities, same bounded attempts as the line above.
        .route(
            "/v1/messages",
            post(anthropic_messages).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/v1/responses",
            post(openai_responses).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/v1/embeddings",
            post(embeddings).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/v1/moderations",
            post(moderations).layer(DefaultBodyLimit::disable()),
        )
        // The two generation shapes that are not text. An image is one
        // answer; a video is a job, read back from the third route with the
        // same model name that started it.
        .route(
            "/v1/images/generations",
            post(image_generations).layer(DefaultBodyLimit::disable()),
        )
        // The OpenAI edit contract: the same picture request, as a form
        // whose files are the input images.
        .route(
            "/v1/images/edits",
            post(image_edits).layer(DefaultBodyLimit::disable()),
        )
        .route("/v1/videos", post(video_generations))
        .route("/v1/audio/speech", post(audio_speech))
        .route("/v1/audio/music", post(audio_music))
        .route(
            "/v1/audio/voices",
            get(audio_voices)
                .post(audio_voice_clone)
                .layer(DefaultBodyLimit::disable()),
        )
        .route("/v1/audio/voices/:voice_id", delete(audio_voice_delete))
        .route("/v1/videos/:video_id", get(video_status))
        // The one endpoint that answers instead of generating: typed
        // questions in, one typed answer each out.
        .route(
            "/v1/decisions",
            post(decisions).layer(DefaultBodyLimit::disable()),
        )
        .route("/v1/models", get(list_models))
        .route("/v1/aliases", get(list_aliases))
        .route("/v1/categories", get(list_categories))
        // The subscription pool: one read and one write, reached by the
        // console, an account holder and a signed agent alike. Which accounts
        // an answer carries follows from the identity the caller proved, so
        // there is nothing per-audience left to register.
        .route(
            "/v1/subscription-pool",
            get(read_subscription_pool).post(write_subscription_pool),
        )
        // Subscription plan usage, on the same terms: one invocation whose
        // answer is narrowed by the same proof, in place of the four
        // per-audience refreshes that all read this one ledger.
        .route("/v1/plan-usage", post(read_plan_usage))
        // Leases: which live session runs on which subscription. Taken by
        // the program that starts a session, released when its terminal
        // ends; read by the same proof as the pool.
        .route(
            "/v1/subscription-pool/leases",
            get(list_leases).post(take_lease).delete(release_leases),
        )
        .route(
            "/v1/subscription-pool/leases/:lease_id",
            delete(release_lease),
        )
        .route(
            "/v1/account/subscription-sign-in/:subscription_id",
            post(sign_in_account_subscription),
        )
        .route("/stats", get(get_stats))
        .route("/v1/admin/snapshot", get(admin_snapshot))
        .route(
            "/v1/admin/categories",
            put(update_admin_category).delete(delete_admin_category),
        )
        .route(
            "/v1/admin/routes",
            put(update_admin_route).delete(delete_admin_route),
        )
        .route(
            "/v1/admin/configuration-adoption/preview",
            post(preview_admin_adoption),
        )
        .route(
            "/v1/admin/configuration-adoption/apply",
            post(apply_admin_adoption),
        )
        .route(
            "/v1/admin/credentials",
            get(list_admin_credentials)
                .put(put_admin_credential)
                .delete(delete_admin_credential),
        )
        .route(
            "/v1/admin/subscription-sign-in/:agent_id/:subscription_id",
            post(sign_in_admin_subscription),
        )
        .route(
            "/v1/admin/subscriptions/:agent_id/:subscription_id/probe",
            post(probe_admin_subscription),
        )
        .route(
            "/v1/admin/subscription-pool/sign-in",
            post(sign_in_admin_pool_subscription),
        )
        .route(
            "/v1/admin/subscription-pool/sign-in/:sign_in_id",
            post(complete_admin_manual_sign_in),
        )
        .route(
            "/v1/admin/subscription-pool/disown",
            post(disown_admin_grant),
        )
        .route(
            "/v1/admin/subscription-pool/reinstate",
            post(reinstate_admin_grant),
        )
        .route(
            "/v1/admin/subscription-pool/refresh",
            post(refresh_admin_subscription_pool),
        )
        .route(
            "/v1/admin/subscription-pool/attribute",
            post(attribute_admin_subscription_pool),
        )
        .route(
            "/v1/admin/subscription-pool/discover",
            post(discover_admin_subscription_pool),
        )
        .route(
            "/v1/admin/subscription-pool/reset",
            post(reset_admin_subscription),
        )
        .route(
            "/v1/admin/subscription-pool/acquire",
            post(acquire_admin_subscription),
        )
        .route("/v1/admin/maintain", post(maintain_admin))
        .layer(Extension(aliases))
        .layer(middleware::from_fn_with_state(
            ingress_auth,
            require_model_bearer,
        ));
    Router::new()
        .route("/health", get(health))
        .route("/healthz", get(health))
        .route("/readyz", get(readyz))
        .merge(protected)
        .layer(middleware::from_fn(require_secure_transport))
}
