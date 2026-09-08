//! Redeeming a capability at the final-use boundary.
//!
//! This file owns the handshake: the handle Brama presents, the proof it signs
//! over, the line protocol with the local authority, and the secret that comes
//! back. What a capability may say lives in `purpose`, what comes back lives in
//! `secret`, and reading this workload's own key lives in `workload_key` —
//! three subjects that change for reasons of their own.

mod purpose;
mod secret;
mod workload_key;

use std::fmt;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use purpose::{is_lower_hex_64, valid_resource};
use workload_key::read_owner_key;

// The names the rest of the crate already uses, re-exported one by one so
// every existing path keeps working and nothing else travels with them.
pub use purpose::{CapabilityError, Purpose, TARGET};
pub use secret::Secret;

pub const WIRE_VERSION: &str = "skarbiec.redeem.v1";
const PROOF_DOMAIN: &[u8] = b"SKARBIEC-WORKLOAD-PROOF\0v1\0";
const MAX_CONTROL_LINE: usize = 4096;
const MAX_SECRET_BYTES: usize = 64 * 1024;

/// An opaque broker handle plus the tuple Brama expects it to represent.
///
/// The broker remains authoritative. This local tuple check prevents a caller
/// from accidentally presenting a capability at the wrong final-use seam.
pub struct CapabilityRef<'a> {
    id: &'a str,
    target: &'a str,
    purpose: Purpose,
    resource: &'a str,
}

impl fmt::Debug for CapabilityRef<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityRef")
            .field("id", &"[opaque]")
            .field("target", &self.target)
            .field("purpose", &self.purpose)
            .field("resource", &self.resource)
            .finish()
    }
}

impl<'a> CapabilityRef<'a> {
    pub fn new(
        id: &'a str,
        target: &'a str,
        purpose: Purpose,
        resource: &'a str,
    ) -> Result<Self, CapabilityError> {
        if target != TARGET || !is_lower_hex_64(id) || !valid_resource(purpose, resource) {
            return Err(CapabilityError::InvalidBinding);
        }
        Ok(Self {
            id,
            target,
            purpose,
            resource,
        })
    }

    pub fn provider(id: &'a str, resource: &'a str) -> Result<Self, CapabilityError> {
        Self::new(id, TARGET, Purpose::ProviderAuthenticate, resource)
    }

    pub fn request_sign(id: &'a str, resource: &'a str) -> Result<Self, CapabilityError> {
        Self::new(id, TARGET, Purpose::RequestSign, resource)
    }

    pub fn purpose(&self) -> Purpose {
        self.purpose
    }

    pub fn resource(&self) -> &str {
        self.resource
    }
}

pub struct CapabilityClient {
    socket: PathBuf,
    workload_id: String,
    signing_key: SigningKey,
}

impl fmt::Debug for CapabilityClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityClient")
            .field("socket", &"[configured]")
            .field("workload_id", &self.workload_id)
            .field("signing_key", &"[redacted]")
            .finish()
    }
}

impl CapabilityClient {
    pub fn from_env() -> Result<Self, CapabilityError> {
        let socket = std::env::var_os("SKARBIEC_CAP_SOCKET")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or(CapabilityError::InvalidConfiguration)?;
        let workload_id = std::env::var("SKARBIEC_WORKLOAD_ID")
            .ok()
            .filter(|value| !value.is_empty() && value.len() <= 128)
            .ok_or(CapabilityError::InvalidConfiguration)?;
        let key_path = std::env::var_os("SKARBIEC_WORKLOAD_SIGNING_KEY_FILE")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or(CapabilityError::InvalidConfiguration)?;
        Self::new(socket, workload_id, &key_path)
    }

    pub fn new(
        socket: PathBuf,
        workload_id: String,
        signing_key_file: &Path,
    ) -> Result<Self, CapabilityError> {
        if !socket.is_absolute() || workload_id.is_empty() || workload_id.len() > 128 {
            return Err(CapabilityError::InvalidConfiguration);
        }
        let signing_key = read_owner_key(signing_key_file)?;
        Ok(Self {
            socket,
            workload_id,
            signing_key,
        })
    }

    /// Redeem at the final-use boundary. Callers must not retain the returned
    /// plaintext beyond the immediately following provider/database/sign step.
    pub fn redeem(&self, capability: &CapabilityRef<'_>) -> Result<Secret, CapabilityError> {
        // Revalidate even though fields are private so this remains fail-closed
        // if construction changes in the future.
        if capability.target != TARGET
            || !is_lower_hex_64(capability.id)
            || !valid_resource(capability.purpose, capability.resource)
        {
            return Err(CapabilityError::InvalidBinding);
        }

        let mut nonce_bytes = [0_u8; 32];
        OpenOptions::new()
            .read(true)
            .open("/dev/urandom")
            .and_then(|mut source| source.read_exact(&mut nonce_bytes))
            .map_err(|_| CapabilityError::InvalidConfiguration)?;
        let nonce = hex::encode(nonce_bytes);
        nonce_bytes.zeroize();

        // The authority signs over capability id, nonce, workload id and the
        // operation, each followed by a separator, then the authorization id.
        // This built a shorter payload and omitted the last two parts, so no
        // proof it produced could ever verify -- a mismatch that reads, from
        // the outside, as a credential that is unavailable.
        const OPERATION: &str = "redeem";
        const AUTHORIZATION_ID: &str = "";
        let mut proof_input = Zeroizing::new(Vec::new());
        proof_input.extend_from_slice(PROOF_DOMAIN);
        for part in [
            capability.id,
            nonce.as_str(),
            self.workload_id.as_str(),
            OPERATION,
        ] {
            proof_input.extend_from_slice(part.as_bytes());
            proof_input.push(b'\0');
        }
        proof_input.extend_from_slice(AUTHORIZATION_ID.as_bytes());
        let proof = URL_SAFE_NO_PAD.encode(self.signing_key.sign(&proof_input).to_bytes());

        let request = RedeemRequest {
            version: WIRE_VERSION,
            operation: OPERATION,
            capability_id: capability.id,
            nonce: &nonce,
            workload_id: &self.workload_id,
            authorization_id: AUTHORIZATION_ID,
            proof,
        };
        let mut encoded = Zeroizing::new(
            serde_json::to_vec(&request).map_err(|_| CapabilityError::RedemptionDenied)?,
        );
        encoded.push(b'\n');

        let mut stream =
            UnixStream::connect(&self.socket).map_err(|_| CapabilityError::RedemptionDenied)?;
        stream
            .write_all(&encoded)
            .map_err(|_| CapabilityError::RedemptionDenied)?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .map_err(|_| CapabilityError::RedemptionDenied)?;

        let line = read_control_line(&mut stream)?;
        let control: RedeemControl =
            serde_json::from_slice(&line).map_err(|_| CapabilityError::RedemptionDenied)?;
        if control.version != WIRE_VERSION || control.status != "ok" {
            return Err(CapabilityError::RedemptionDenied);
        }
        let length = control
            .secret_len
            .filter(|length| *length > 0 && *length <= MAX_SECRET_BYTES)
            .ok_or(CapabilityError::RedemptionDenied)?;
        let mut secret = Zeroizing::new(vec![0_u8; length]);
        if stream.read_exact(&mut secret).is_err() {
            return Err(CapabilityError::RedemptionDenied);
        }
        let mut extra = [0_u8; 1];
        match stream.read(&mut extra) {
            Ok(0) => Ok(Secret::from_zeroizing(secret)),
            Ok(_) | Err(_) => Err(CapabilityError::RedemptionDenied),
        }
    }
}

/// The wire the authority reads.
///
/// `operation` and `authorization_id` were absent here while the authority
/// required the first and signed over both. It rejected every request at its
/// opening validation, replied with an opaque `denied`, and the gateway
/// reported a provider credential that was merely "unavailable" -- so the
/// missing field looked like a credential problem for as long as anyone cared
/// to look.
#[derive(Serialize)]
struct RedeemRequest<'a> {
    version: &'static str,
    operation: &'static str,
    capability_id: &'a str,
    nonce: &'a str,
    workload_id: &'a str,
    authorization_id: &'a str,
    proof: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RedeemControl {
    version: String,
    status: String,
    #[serde(default)]
    secret_len: Option<usize>,
}

fn read_control_line(stream: &mut UnixStream) -> Result<Vec<u8>, CapabilityError> {
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
