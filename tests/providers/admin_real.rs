//! Real administration lifecycles against the real Brama binary, Skarbiec,
//! and OpenRouter. No provider replacement, canned response, or dry run.
//!
//! One story per panel, so a failing panel names itself: `gateway` prepares
//! the real gateway over a private route registry, `routes` drives the alias
//! route panel, `credentials` the provider credential panel, `surfaces` every
//! chat surface and operational read, and `subscriptions` the agent
//! subscription pool. Integration tests bind their modules by path, as the
//! neighbouring suites in `tests/` do.

#[path = "admin_real/gateway.rs"]
mod gateway;

#[path = "admin_real/routes.rs"]
mod routes;

#[path = "admin_real/credentials.rs"]
mod credentials;

#[path = "admin_real/surfaces.rs"]
mod surfaces;

#[path = "admin_real/subscriptions.rs"]
mod subscriptions;
