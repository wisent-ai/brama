//! Part of `capability`, split out to keep every file inside the line limit.
#![allow(unused_imports)]

use super::*;
use std::fmt;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

/// The wire the authority reads.
///
/// `operation` and `authorization_id` were absent here while the authority
/// required the first and signed over both. It rejected every request at its
/// opening validation, replied with an opaque `denied`, and the gateway
/// reported a provider credential that was merely "unavailable" -- so the
/// missing field looked like a credential problem for as long as anyone cared
/// to look.
#[derive(Serialize)]
pub(crate) struct RedeemRequest<'a> {
    pub(crate) version: &'static str,
    pub(crate) operation: &'static str,
    pub(crate) capability_id: &'a str,
    pub(crate) nonce: &'a str,
    pub(crate) workload_id: &'a str,
    pub(crate) authorization_id: &'a str,
    pub(crate) proof: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RedeemControl {
    pub(crate) version: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) secret_len: Option<usize>,
}

pub(crate) fn read_control_line(stream: &mut UnixStream) -> Result<Vec<u8>, CapabilityError> {
    let mut line = Vec::with_capacity(256);
    let mut byte = [0_u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(1) if byte[0] == b'\n' => return Ok(line),
            Ok(1) if line.len() < MAX_CONTROL_LINE => line.push(byte[0]),
            _ => return Err(CapabilityError::RedemptionDenied),
        }
    }
}

pub(crate) fn read_owner_key(path: &Path) -> Result<SigningKey, CapabilityError> {
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
        || metadata.mode() & 0o077 != 0
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

pub(crate) fn parse_signing_key(encoded: &[u8]) -> Result<SigningKey, CapabilityError> {
    let trimmed = encoded
        .strip_suffix(b"\n")
        .unwrap_or(encoded)
        .strip_suffix(b"\r")
        .unwrap_or_else(|| encoded.strip_suffix(b"\n").unwrap_or(encoded));
    let mut raw = Zeroizing::new([0_u8; 32]);
    if trimmed.len() == 32 {
        raw.copy_from_slice(trimmed);
    } else if trimmed.len() == 64
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
