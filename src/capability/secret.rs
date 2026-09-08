//! The redeemed plaintext, and the two ways it may be looked at.
//!
//! It is its own module because every part of Brama that touches credential
//! material holds one of these, and none of them should have to read the
//! redemption wire to understand what it has: bytes that wipe themselves, and
//! a `Debug` that never prints them.

use std::fmt;

use zeroize::Zeroizing;

use super::purpose::CapabilityError;

pub struct Secret(Zeroizing<Vec<u8>>);

impl Secret {
    pub(crate) fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Only for the redemption path, which reads the secret straight off the
    /// socket into the buffer this wraps.
    pub(super) fn from_zeroizing(bytes: Zeroizing<Vec<u8>>) -> Self {
        Self(bytes)
    }

    pub fn expose(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub fn expose_utf8(&self) -> Result<&str, CapabilityError> {
        std::str::from_utf8(self.expose()).map_err(|_| CapabilityError::RedemptionDenied)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret([redacted])")
    }
}
