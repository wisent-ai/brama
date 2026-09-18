//! Reading this workload's own signing key off disk, and refusing it when the
//! file is not exclusively ours.
//!
//! The checks are the point: the key is the proof of identity the authority
//! verifies, so a file someone else can read or replace is not a key, it is a
//! way in. Ownership, mode, type, size and symlink following are all refused
//! before a byte is parsed.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use ed25519_dalek::SigningKey;
use zeroize::Zeroizing;

/// Mode bits that would let the group or others touch the key.
const GROUP_OR_OTHER_ACCESS: u32 = 0o077;
/// An Ed25519 seed is 32 raw bytes, or those bytes as 64 hex characters.
const SEED_BYTES: usize = 32;
const SEED_HEX_CHARS: usize = 64;

use super::purpose::CapabilityError;

const MAX_KEY_BYTES: u64 = 4096;

pub(super) fn read_owner_key(path: &Path) -> Result<SigningKey, CapabilityError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| CapabilityError::InvalidConfiguration)?;
    let metadata = file
        .metadata()
        .map_err(|_| CapabilityError::InvalidConfiguration)?;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & GROUP_OR_OTHER_ACCESS != 0
        || metadata.len() == 0
        || metadata.len() > MAX_KEY_BYTES
    {
        return Err(CapabilityError::InvalidConfiguration);
    }
    let mut encoded = Zeroizing::new(Vec::with_capacity(metadata.len() as usize));
    file.read_to_end(&mut encoded)
        .map_err(|_| CapabilityError::InvalidConfiguration)?;
    parse_signing_key(&encoded)
}

fn parse_signing_key(encoded: &[u8]) -> Result<SigningKey, CapabilityError> {
    let trimmed = encoded
        .strip_suffix(b"\n")
        .unwrap_or(encoded)
        .strip_suffix(b"\r")
        .unwrap_or_else(|| encoded.strip_suffix(b"\n").unwrap_or(encoded));
    let mut raw = Zeroizing::new([0_u8; SEED_BYTES]);
    if trimmed.len() == SEED_BYTES {
        raw.copy_from_slice(trimmed);
    } else if trimmed.len() == SEED_HEX_CHARS
        && trimmed
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        hex::decode_to_slice(trimmed, raw.as_mut_slice())
            .map_err(|_| CapabilityError::InvalidConfiguration)?;
    } else {
        return Err(CapabilityError::InvalidConfiguration);
    }
    Ok(SigningKey::from_bytes(&raw))
}
