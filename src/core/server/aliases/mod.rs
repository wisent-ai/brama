//! What an alias points at, whether that destination can be served on this
//! host, and what to tell an operator when it cannot.
//!
//! An alias resolves to exactly one destination. [`table`] reads the launcher's
//! alias table and the route registry and answers "give me a route or nothing",
//! which is what dispatch needs; [`diagnosis`] answers "why is this alias in
//! the state it is in", which is what an operator needs; [`report`] assembles
//! that answer for a shell holding no bearer for the running gateway.
//!
//! The names below are the compiled-in contract. Every other alias is the
//! operator's to declare, in the launcher's policy or the route registry.

pub(in crate::core::server) mod diagnosis;
pub(in crate::core::server) mod report;
pub(in crate::core::server) mod table;

pub(in crate::core::server) const MODEL_ALIASES_ENV: &str = "BRAMA_MODEL_ALIASES";
/// The Wisent App backend's alias. It is the client's name and nothing else.
///
/// `wisent-backend/chat/primary` and its ranked sibling spelled a rank into the
/// name, and a rank is not something an alias has: one alias names one
/// destination, and when that destination cannot answer the caller is told so.
/// `wisent-backend/chat` then still spelled the shape, which is what the
/// endpoint the caller posts to already says. `POST /v1/chat/completions` is
/// the chat shape, so the name carries only who is asking.
pub(in crate::core::server) const WISENT_BACKEND_ALIAS: &str = "wisent-backend";
pub(in crate::core::server) const WISENT_EVALUATION_ALIAS: &str = "wisent-backend/evaluation";
/// The two aliases whose name still carries a purpose, and the only reason it
/// does: `POST /v1/embeddings` and `POST /v1/moderations` accept exactly this
/// one model name each, so the endpoint already names the shape — but the route
/// table is keyed by alias name alone and these two need a destination of their
/// own, different from chat's. Until a route can be declared per shape, the
/// suffix is where that destination hangs.
pub(in crate::core::server) const WISENT_EMBEDDING_ALIAS: &str = "wisent-backend/embeddings";
pub(in crate::core::server) const WISENT_MODERATION_ALIAS: &str = "wisent-backend/moderation";
/// The Weles workload's own alias. It is spelled as the client's name and
/// nothing more: `weles/agent/primary` carried a purpose and a rank in the
/// name, both of which changed under it twice while the name stayed, so the
/// name stopped describing anything. Which model serves it is the route table's
/// business, and `GET /v1/aliases` is where that is read.
pub(in crate::core::server) const WELES_ALIAS: &str = "weles";
pub const BEST_ALIAS: &str = "best";
pub(in crate::core::server) const WISENT_MODEL_ALIASES: &[&str] = &[
    WISENT_BACKEND_ALIAS,
    WISENT_EVALUATION_ALIAS,
    WISENT_EMBEDDING_ALIAS,
    WISENT_MODERATION_ALIAS,
];
pub(in crate::core::server) const MODEL_ALIASES: &[&str] = &[
    WISENT_BACKEND_ALIAS,
    WISENT_EVALUATION_ALIAS,
    WISENT_EMBEDDING_ALIAS,
    WISENT_MODERATION_ALIAS,
    WELES_ALIAS,
    BEST_ALIAS,
];

/// Which alias may carry which kind of route. `best` is a chat route like the
/// other chat aliases; what differs is who pays for it, not what it is.
///
/// The five `wisent-backend/*` aliases keep exact shapes because their names
/// are a promise to the caller: whoever asks for `embeddings` must never be
/// handed a chat model. Every other name is the operator's to invent — `smol`,
/// `dumb`, `best-vision` — and carries no such promise, so it is accepted on
/// the general-purpose shape. Rejecting unknown names outright, as this did,
/// made every new alias a Rust change and a gateway release.
///
/// A route naming `best` is delegation rather than a provider: the alias hands
/// the choice to subscription dispatch, which resolves it per caller identity.
pub(crate) fn alias_route_shape_supported(alias: &str, route: &str) -> bool {
    if route == BEST_ALIAS {
        return alias != WISENT_EMBEDDING_ALIAS && alias != WISENT_MODERATION_ALIAS;
    }
    match alias {
        WISENT_EMBEDDING_ALIAS => crate::providers::adapter::supports_embedding_route(route),
        WISENT_MODERATION_ALIAS => crate::providers::adapter::supports_moderation_route(route),
        _ => crate::providers::adapter::supports_chat_route(route),
    }
}

/// Whether this alias must resolve to a provider Brama holds a direct
/// credential for.
///
/// `best` resolves to a subscription route: the caller's HMAC identity selects
/// the subscription that pays, and Brama deliberately holds no direct provider
/// credential for it. Requiring a configured direct capability would reject the
/// only configuration the alias is ever meant to have.
///
/// The same is true of any alias whose route delegates to `best`, which is why
/// this asks about the route as well as the name: an operator-defined alias
/// pointing at the subscription route owns no direct credential either, and
/// keying the exemption on the alias name alone made `best` the only alias that
/// could ever reach a subscription-funded model.
pub(crate) fn alias_requires_direct_capability(alias: &str, route: &str) -> bool {
    alias != BEST_ALIAS && route != BEST_ALIAS
}
