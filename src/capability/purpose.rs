//! What a capability may say, and what it may not.
//!
//! The broker remains authoritative; these are the rules Brama applies before
//! it presents anything, so a capability minted for one seam can never be
//! handed to another one by accident.

use thiserror::Error;

/// The only target Brama presents capabilities as.
pub const TARGET: &str = "brama";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    ProviderAuthenticate,
    RequestSign,
}

impl Purpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderAuthenticate => "brama.provider.authenticate",
            Self::RequestSign => "brama.request.sign",
        }
    }

    const fn resource_prefix(self) -> &'static str {
        match self {
            Self::ProviderAuthenticate => "provider:",
            Self::RequestSign => "agent:",
        }
    }
}

/// A resource belongs to its purpose, names something concrete, and carries no
/// glob: a capability that could match a pattern is a capability that could be
/// presented for the wrong account.
pub(super) fn valid_resource(purpose: Purpose, resource: &str) -> bool {
    let Some(concrete) = resource.strip_prefix(purpose.resource_prefix()) else {
        return false;
    };
    !concrete.is_empty()
        && !concrete.trim().is_empty()
        && concrete == concrete.trim()
        && !concrete
            .chars()
            .any(|ch| matches!(ch, '*' | '?' | '[' | ']'))
}

pub(super) fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

#[derive(Error, Debug, PartialEq, Eq)]
pub enum CapabilityError {
    #[error("invalid capability binding")]
    InvalidBinding,
    #[error("invalid capability client configuration")]
    InvalidConfiguration,
    #[error("capability redemption denied")]
    RedemptionDenied,
}
