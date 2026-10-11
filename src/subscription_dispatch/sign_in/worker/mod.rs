//! The account worker a sign-in talks to: where it is, what proves a call to
//! it, and how one account is resolved through it.

pub(super) mod account;
pub(crate) mod api;
pub(crate) mod enrolment;
pub mod progress;

/// The Weles provider name the declared subscription provider signs in
/// through; `None` when its declaration names no Weles authorization
/// capability.
pub fn weles_provider(provider: &str) -> Option<&'static str> {
    crate::subscription_dispatch::acquire::declaration::provider(provider.trim())
        .ok()?
        .weles_provider
        .as_deref()
}
