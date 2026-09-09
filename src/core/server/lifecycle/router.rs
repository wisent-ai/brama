//! Every path this gateway answers, in one table.
//!
//! Two layers wrap it and the order is the point: the transport guard is
//! outermost, so an unprotected hop is refused before any credential is read,
//! and the bearer guard sits inside it around everything except `/health` and
//! `/readyz` — the two paths a deployment probe reaches with nothing at all.

use axum::middleware;
use axum::routing::{get, post, put};
use axum::{Extension, Router};

use crate::core::server::administration::adoption::{apply_admin_adoption, preview_admin_adoption};
use crate::core::server::administration::credentials::{
    delete_admin_credential, list_admin_credentials, put_admin_credential,
};
use crate::core::server::administration::routes::{delete_admin_route, update_admin_route};
use crate::core::server::administration::snapshot::admin_snapshot;
use crate::core::server::admission::ingress::ModelIngressAuth;
use crate::core::server::admission::{require_model_bearer, transport::require_secure_transport};
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::catalog::list_aliases;
use crate::core::server::catalog::models::list_models;
use crate::core::server::chat::chat_completions;
use crate::core::server::chat::dialects::{anthropic_messages, openai_responses};
use crate::core::server::readiness::{health, readyz};
use crate::core::server::subscriptions::probe::{
    probe_admin_subscription, refresh_admin_subscription_pool,
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
    let protected = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        // The two first-party formats callers already speak. Same routing
        // decision, same identities, same bounded attempts as the line above.
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/responses", post(openai_responses))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/moderations", post(moderations))
        .route("/v1/models", get(list_models))
        .route("/v1/aliases", get(list_aliases))
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
        .route(
            "/v1/account/subscription-sign-in/:subscription_id",
            post(sign_in_account_subscription),
        )
        .route("/stats", get(get_stats))
        .route("/v1/admin/snapshot", get(admin_snapshot))
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
            "/v1/admin/subscription-pool/refresh",
            post(refresh_admin_subscription_pool),
        )
        .layer(Extension(aliases))
        .layer(middleware::from_fn_with_state(
            ingress_auth,
            require_model_bearer,
        ));
    Router::new()
        .route("/health", get(health))
        .route("/readyz", get(readyz))
        .merge(protected)
        .layer(middleware::from_fn(require_secure_transport))
}
