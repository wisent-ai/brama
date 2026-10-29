//! Redeeming a capability at the final-use boundary.
//!
//! This file owns the handshake: the handle Brama presents, the proof it signs
//! over, the line protocol with the local authority, and the secret that comes
//! back. What a capability may say lives in `purpose`, what comes back lives in
//! `secret`, and reading this workload's own key lives in `workload_key` —
//! three subjects that change for reasons of their own.

mod configuration;
mod purpose;
mod secret;
mod workload_key;

/// The owner-only Ed25519 key reader, also for Brama's answer key.
pub(crate) use workload_key::read_owner_key;

/// The proof nonce's length in random bytes, as Skarbiec publishes it.
const NONCE_BYTES: usize = 32; // https://skarbiec.wisent.com/docs/credential-model "one 32-byte random value"

use std::fmt;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use purpose::{is_lower_hex_64, valid_resource};

// The names the rest of the crate already uses, re-exported one by one so
// every existing path keeps working and nothing else travels with them.
pub use purpose::{CapabilityError, Purpose, TARGET};
pub use secret::Secret;

pub const WIRE_VERSION: &str = "skarbiec.redeem.v1";
const PROOF_DOMAIN: &[u8] = b"SKARBIEC-WORKLOAD-PROOF\0v1\0";

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

        let mut nonce_bytes = Zeroizing::new(Vec::with_capacity(NONCE_BYTES));
        let wanted = u64::try_from(NONCE_BYTES).map_err(|error| {
            CapabilityError::InvalidConfiguration(format!(
                "{NONCE_BYTES} nonce bytes cannot be counted: {error}"
            ))
        })?;
        OpenOptions::new()
            .read(true)
            .open("/dev/urandom")
            .and_then(|source| source.take(wanted).read_to_end(&mut nonce_bytes))
            .map_err(|error| {
                CapabilityError::InvalidConfiguration(format!(
                    "read capability proof nonce from /dev/urandom: {error}"
                ))
            })?;
        if nonce_bytes.len() != NONCE_BYTES {
            return Err(CapabilityError::InvalidConfiguration(format!(
                "/dev/urandom gave {} of the {NONCE_BYTES} nonce bytes",
                nonce_bytes.len()
            )));
        }
        let nonce = hex::encode(nonce_bytes.as_slice());
        drop(nonce_bytes);

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
            serde_json::to_vec(&request)
                .map_err(|error| CapabilityError::RequestEncoding(error.to_string()))?,
        );
        encoded.push(b'\n');

        // A socket that cannot be reached, written or read is not a denial:
        // the authority never answered, and the io error says why.
        let socket = self.socket.display();
        let unreachable = |step: &str, error: std::io::Error| {
            CapabilityError::AuthorityUnreachable(format!("{step} {socket}: {error}"))
        };
        let mut stream = UnixStream::connect(&self.socket)
            .map_err(|error| unreachable("connecting to", error))?;
        stream
            .write_all(&encoded)
            .map_err(|error| unreachable("writing to", error))?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .map_err(|error| unreachable("closing the request to", error))?;

        let line = read_control_line(&mut stream)
            .map_err(|error| unreachable("reading the control line from", error))?;
        let control: RedeemControl = serde_json::from_slice(&line).map_err(|error| {
            CapabilityError::AuthorityAnswer(format!(
                "control line from {socket} is not JSON ({error}): {}",
                String::from_utf8_lossy(&line)
            ))
        })?;
        if control.version != WIRE_VERSION || control.status != "ok" {
            return Err(CapabilityError::RedemptionDenied(format!(
                "{socket} answered status {:?} at wire version {:?}; a secret comes with status \"ok\" at {WIRE_VERSION:?}",
                control.status, control.version
            )));
        }
        let length = control
            .secret_len
            .and_then(std::num::NonZeroUsize::new)
            .map(std::num::NonZeroUsize::get)
            .ok_or_else(|| {
                CapabilityError::RedemptionDenied(format!(
                    "{socket} answered ok with secret length {:?}, so no secret follows",
                    control.secret_len
                ))
            })?;
        let mut secret = Zeroizing::new(Vec::with_capacity(length));
        let wanted = u64::try_from(length).map_err(|error| {
            CapabilityError::AuthorityAnswer(format!(
                "{socket} announced a {length}-byte secret: {error}"
            ))
        })?;
        Read::by_ref(&mut stream)
            .take(wanted)
            .read_to_end(&mut secret)
            .map_err(|error| unreachable("reading the secret from", error))?;
        if secret.len() != length {
            return Err(CapabilityError::AuthorityAnswer(format!(
                "{socket} closed after {} of the {length}-byte secret",
                secret.len()
            )));
        }
        match read_byte(&mut stream) {
            Ok(None) => Ok(Secret::from_zeroizing(secret)),
            Ok(Some(_)) => Err(CapabilityError::AuthorityAnswer(format!(
                "response from {socket} contains trailing bytes after the {length}-byte secret"
            ))),
            Err(error) => Err(unreachable("reading the secret boundary from", error)),
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

fn read_control_line(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut line = Vec::new();
    // One byte at a time: the secret follows the line on the same stream.
    while let Some(byte) = read_byte(stream)? {
        match byte {
            b'\n' => return Ok(line),
            other => line.push(other),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::UnexpectedEof,
        format!(
            "authority closed response after {} control-line bytes before its terminator",
            line.len()
        ),
    ))
}

/// The stream's next byte, or none at its end. Unbuffered on purpose: a read
/// buffer would hold the secret that follows the control line outside
/// `Zeroizing`.
fn read_byte(stream: &mut UnixStream) -> std::io::Result<Option<u8>> {
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Ok(None),
            Ok(_) => return Ok(Some(byte[0])),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}
