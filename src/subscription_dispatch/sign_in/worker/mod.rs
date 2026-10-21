//! The account worker a sign-in talks to: where it is, what proves a call to
//! it, and how one account is resolved through it.

pub(super) mod account;
pub(crate) mod api;
pub(crate) mod enrolment;
pub mod progress;

/// The Weles provider name the declared subscription provider signs in and
/// buys through, or why there is none: the provider is not declared at all,
/// or its declaration names no Weles authorization capability. The one reader
/// of that declaration field, and the two causes are told apart.
pub fn weles_provider(provider: &str) -> Result<&'static str, String> {
    let provider = provider.trim();
    crate::subscription_dispatch::acquire::declaration::provider(provider)?
        .weles_provider
        .as_deref()
        .ok_or_else(|| format!("provider `{provider}` declares no Weles authorization capability"))
}
