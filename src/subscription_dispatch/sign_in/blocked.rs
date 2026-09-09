//! Why an automatic sign-in cannot happen, named against the architecture it
//! contradicts.
//!
//! The design has no manual step in it. Brama runs on its own host, reads the
//! subscription list out of Skarbiec, and when a credential is missing or the
//! provider has refused it, asks Weles to sign that account in. Brama Desktop
//! is a window onto that gateway. Nobody logs a provider in by hand and no
//! operator command is part of the loop.
//!
//! Every variant here is one way a deployment stops matching that design, and
//! each is a statement about a declaration rather than about a provider: an
//! account Skarbiec never mapped to a Weles account, a Weles that holds no
//! account for the provider or two it cannot choose between, a named account
//! Weles does not hold, a Weles release that cannot be told which account to
//! use, a Weles the gateway cannot reach, and a gateway running where the
//! registry places none.
//!
//! They live in one place because they are read from three: the sweep logs the
//! one it hit, the pool document carries it per account so the console and
//! Desktop show the same words, and readiness reports the set so a host that
//! can serve traffic while repairing nothing says so out loud. Before this,
//! all of them arrived as `no credential`, which describes the symptom of
//! every one and the cause of none.

use std::fmt;

use serde_json::{json, Value};
use wisent_errors::Code;

use crate::core::failure;
use crate::gateway::broker::SubscriptionEntry;

/// The failure point every one of these is reported under.
pub const POINT: &str = "brama.subscriptions.automatic-sign-in";
/// What one blocked account costs.
pub const IMPACT: &str = "one automatic sign-in";
/// The Skarbiec tag that maps a subscription to the Weles account which signs
/// it in. Named in the sentences because it is the repair.
pub const LOGIN_TAG: &str = "brama:login:";

/// One reason the automatic loop cannot repair one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// Skarbiec holds the subscription but never says which Weles account
    /// signs it in, and the provider has more than one.
    NoWelesAccount { provider: String },
    /// Weles holds no sign-in account for this provider at all.
    WelesHoldsNoAccount { provider: String, detail: String },
    /// Weles holds several accounts for the provider, none declared primary,
    /// and this subscription names none.
    WelesAccountAmbiguous { provider: String, detail: String },
    /// The named Weles account does not exist, or belongs to another provider.
    WelesAccountUnknown { provider: String, detail: String },
    /// This Weles release cannot be told which account to sign in, so it would
    /// choose one itself.
    WelesCannotTargetAccount { detail: String },
    /// The gateway cannot reach Weles.
    WelesUnreachable { detail: String },
    /// This process is a gateway on a host the registry places none on.
    NotPlacedHost {
        placed_on: String,
        this_host: String,
    },
}

impl Blocked {
    /// The stable word a machine reads, and the key an operator greps for.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoWelesAccount { .. } => "no_weles_account",
            Self::WelesHoldsNoAccount { .. } => "weles_holds_no_account",
            Self::WelesAccountAmbiguous { .. } => "weles_account_ambiguous",
            Self::WelesAccountUnknown { .. } => "weles_account_unknown",
            Self::WelesCannotTargetAccount { .. } => "weles_cannot_target_account",
            Self::WelesUnreachable { .. } => "weles_unreachable",
            Self::NotPlacedHost { .. } => "not_placed_host",
        }
    }

    /// The fleet code: a declaration that does not match the world is
    /// configuration, and a dependency that does not answer is a dependency.
    pub fn failure_code(&self) -> Code {
        match self {
            Self::WelesUnreachable { .. } => Code::InfraDown,
            _ => Code::Config,
        }
    }

    /// The provider whose account this is about, when it is about one.
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::NoWelesAccount { provider }
            | Self::WelesHoldsNoAccount { provider, .. }
            | Self::WelesAccountAmbiguous { provider, .. }
            | Self::WelesAccountUnknown { provider, .. } => Some(provider),
            Self::WelesCannotTargetAccount { .. }
            | Self::WelesUnreachable { .. }
            | Self::NotPlacedHost { .. } => None,
        }
    }

    /// What is inconsistent and where the repair is, in one sentence, because
    /// it is read in a log line, a console row and a Desktop panel.
    pub fn detail(&self) -> String {
        match self {
            Self::NoWelesAccount { provider } => format!(
                "Skarbiec lists this subscription but its item carries no `{LOGIN_TAG}<account>` \
                 tag, so nothing maps it to a Weles sign-in account; the gateway signs accounts in \
                 by itself and will not guess between this deployment's `{provider}` accounts, so \
                 this one stays unrepaired until that tag names its Weles account"
            ),
            Self::WelesHoldsNoAccount { provider, detail } => format!(
                "Weles declares no sign-in account for `{provider}`, so the gateway has nothing to \
                 drive and no credential for this subscription can be obtained: {detail}"
            ),
            Self::WelesAccountAmbiguous { provider, detail } => format!(
                "Weles declares several sign-in accounts for `{provider}` and none as primary, and \
                 this subscription's Skarbiec item names none through `{LOGIN_TAG}<account>`, so \
                 the gateway would have to guess which account this subscription is: {detail}"
            ),
            Self::WelesAccountUnknown { provider, detail } => format!(
                "this subscription's `{LOGIN_TAG}` tag names a `{provider}` Weles account that \
                 Weles does not hold under that name, so the declaration and Weles disagree about \
                 which account exists: {detail}"
            ),
            Self::WelesCannotTargetAccount { detail } => format!(
                "this Weles release cannot be told which account to sign in, so it would choose \
                 one itself and the gateway refuses to hand it a real account on that basis: \
                 {detail}"
            ),
            Self::WelesUnreachable { detail } => format!(
                "the gateway cannot reach Weles, which owns every sign-in, so no refused \
                 credential on this deployment can be replaced: {detail}"
            ),
            Self::NotPlacedHost {
                placed_on,
                this_host,
            } => format!(
                "this gateway runs on `{this_host}` while the registry places brama on \
                 `{placed_on}`; a second gateway reads the same vault and signs the same accounts \
                 in, so its sign-ins race the placed host's and its answers are not the ones \
                 clients are routed to"
            ),
        }
    }

    /// The object a pool document, readiness answer or Desktop panel reads.
    pub fn to_json(&self) -> Value {
        json!({
            "blocked_by": self.code(),
            "detail": self.detail(),
        })
    }

    /// The same reason as a fleet failure, for the `errors` array a console
    /// prints and a gate fails on.
    pub fn failure(&self, subscription_id: Option<&str>) -> Value {
        let mut envelope = failure::envelope(POINT, self.failure_code(), IMPACT, self.detail())
            .with_context("blocked_by", self.code());
        if let Some(subscription_id) = subscription_id {
            envelope = envelope.with_context("subscription", subscription_id);
        }
        if let Some(provider) = self.provider() {
            envelope = envelope.with_context("provider", provider);
        }
        serde_json::from_str(&envelope.to_json()).expect("Wisent failure serialization is JSON")
    }
}

impl fmt::Display for Blocked {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail())
    }
}

/// Why one sign-in did not produce a verdict.
///
/// The two halves are acted on differently and used to arrive as one string.
/// A [`Blocked`] reason is a declaration this deployment is missing: it will
/// read the same on the next sweep and the next, so it is reported with its
/// own word and its own envelope. A dependency failure is the caller's or the
/// moment's -- an unknown provider, an empty reason, a refresh that would not
/// run -- and says nothing about the fleet's declarations.
#[derive(Debug, Clone)]
pub enum SignInError {
    Blocked(Blocked),
    Dependency(String),
}

impl SignInError {
    /// The blocked reason, when the sign-in stopped on a declaration.
    pub fn blocked(&self) -> Option<&Blocked> {
        match self {
            Self::Blocked(blocked) => Some(blocked),
            Self::Dependency(_) => None,
        }
    }
}

impl fmt::Display for SignInError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked(blocked) => formatter.write_str(&blocked.detail()),
            Self::Dependency(detail) => formatter.write_str(detail),
        }
    }
}

impl From<Blocked> for SignInError {
    fn from(blocked: Blocked) -> Self {
        Self::Blocked(blocked)
    }
}

impl From<String> for SignInError {
    fn from(detail: String) -> Self {
        Self::Dependency(detail)
    }
}

/// Whether this account can be signed in without a declaration being fixed
/// first, decided from the subscription listing alone.
///
/// Free: the answer is in the tags the vault listing already carried. It is
/// deliberately the cheap half of the question, so the pool document and every
/// screen reading it can state it without touching Weles.
pub fn declared_account(entry: &SubscriptionEntry) -> Result<&str, Blocked> {
    entry
        .login_item
        .as_deref()
        .map(str::trim)
        .filter(|account| !account.is_empty())
        .ok_or_else(|| Blocked::NoWelesAccount {
            provider: entry.provider.clone(),
        })
}
