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

use ed25519_dalek::{SigningKey, SECRET_KEY_LENGTH};
use zeroize::Zeroizing;

use super::purpose::CapabilityError;

/// Mode bits that would let the group or others touch the key.
const GROUP_OR_OTHER_ACCESS: u32 = 0o077; // https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/sys_stat.h.html S_IRWXG | S_IRWXO
/// The signing seed is the curve library's secret key length in raw bytes,
/// or those bytes written as hexadecimal digits.
const HEX_DIGITS_PER_BYTE: usize = 2; // https://www.rfc-editor.org/rfc/rfc4648#section-8 "each octet ... as two hexadecimal digits"
const SEED_HEX_CHARS: usize = SECRET_KEY_LENGTH * HEX_DIGITS_PER_BYTE; // https://www.rfc-editor.org/rfc/rfc4648#section-8 base16 doubles the octet count

/// The longest file a valid key can be: the hex seed and a CRLF ending.
/// Anything longer is refused before it is read, because it cannot parse.
const MAX_KEY_BYTES: u64 = (SEED_HEX_CHARS + b"\r\n".len()) as u64; // https://www.rfc-editor.org/rfc/rfc5234#appendix-B.1 CRLF = CR LF

pub(crate) fn read_owner_key(path: &Path) -> Result<SigningKey, CapabilityError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            CapabilityError::KeyUnreadable(format!("open {}: {error}", path.display()))
        })?;
    let metadata = file.metadata().map_err(|error| {
        CapabilityError::KeyUnreadable(format!("inspect {}: {error}", path.display()))
    })?;
    if !metadata.file_type().is_file() {
        return Err(CapabilityError::InvalidConfiguration(format!(
            "workload signing key {} is not a regular file",
            path.display()
        )));
    }
    let owner = unsafe { libc::geteuid() };
    if metadata.uid() != owner {
        return Err(CapabilityError::InvalidConfiguration(format!(
            "workload signing key {} has owner {}; expected effective user {owner}",
            path.display(),
            metadata.uid()
        )));
    }
    if metadata.mode() & GROUP_OR_OTHER_ACCESS > 0 {
        return Err(CapabilityError::InvalidConfiguration(format!(
            "workload signing key {} permits group or other access; observed mode {:o}",
            path.display(),
            metadata.mode()
        )));
    }
    if metadata.len() == 0 || metadata.len() > MAX_KEY_BYTES {
        return Err(CapabilityError::InvalidConfiguration(format!(
            "workload signing key {} has invalid length {}; expected nonempty seed data at most {MAX_KEY_BYTES} bytes",
            path.display(), metadata.len()
        )));
    }
    let mut encoded = Zeroizing::new(Vec::with_capacity(metadata.len() as usize));
    file.read_to_end(&mut encoded).map_err(|error| {
        CapabilityError::KeyUnreadable(format!("read {}: {error}", path.display()))
    })?;
    parse_signing_key(&encoded).map_err(|error| {
        CapabilityError::InvalidConfiguration(format!(
            "decode workload signing key {}: {error}",
            path.display()
        ))
    })
}

/// The seed a key file holds. A file of exactly the seed's length is the raw
/// seed, every byte of it key material, a byte that happens to be a newline
/// included; any other file is the seed in lowercase hexadecimal, with the
/// line ending a text editor leaves after it.
fn parse_signing_key(encoded: &[u8]) -> Result<SigningKey, CapabilityError> {
    if let Ok(raw) = <[u8; SECRET_KEY_LENGTH]>::try_from(encoded) {
        let raw = Zeroizing::new(raw);
        return Ok(SigningKey::from_bytes(&raw));
    }
    let text = encoded.trim_ascii_end();
    let lowercase_hex = text
        .iter()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
    if text.len() != SEED_HEX_CHARS || !lowercase_hex {
        return Err(CapabilityError::InvalidConfiguration(format!(
            "seed must be {SECRET_KEY_LENGTH} raw bytes or {SEED_HEX_CHARS} lowercase hexadecimal characters; observed {} bytes",
            encoded.len()
        )));
    }
    let decoded = Zeroizing::new(hex::decode(text).map_err(|error| {
        CapabilityError::InvalidConfiguration(format!("decode hexadecimal seed: {error}"))
    })?);
    let raw = Zeroizing::new(
        <[u8; SECRET_KEY_LENGTH]>::try_from(decoded.as_slice()).map_err(|error| {
            CapabilityError::InvalidConfiguration(format!("hexadecimal seed length: {error}"))
        })?,
    );
    Ok(SigningKey::from_bytes(&raw))
}
