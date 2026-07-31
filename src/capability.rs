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

pub const TARGET: &str = "brama";
pub const WIRE_VERSION: &str = "skarbiec.redeem.v1";
const PROOF_DOMAIN: &[u8] = b"SKARBIEC-WORKLOAD-PROOF\0v1\0";
const MAX_CONTROL_LINE: usize = 4096;
const MAX_SECRET_BYTES: usize = 64 * 1024;
const MAX_KEY_BYTES: u64 = 4096;

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

fn valid_resource(purpose: Purpose, resource: &str) -> bool {
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

fn is_lower_hex_64(value: &str) -> bool {
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

pub struct Secret(Zeroizing<Vec<u8>>);

impl Secret {
    pub(crate) fn from_bytes(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
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

        let mut proof_input = Zeroizing::new(Vec::with_capacity(
            PROOF_DOMAIN.len() + capability.id.len() + nonce.len() + self.workload_id.len() + 2,
        ));
        proof_input.extend_from_slice(PROOF_DOMAIN);
        proof_input.extend_from_slice(capability.id.as_bytes());
        proof_input.push(0);
        proof_input.extend_from_slice(nonce.as_bytes());
        proof_input.push(0);
        proof_input.extend_from_slice(self.workload_id.as_bytes());
        let proof = URL_SAFE_NO_PAD.encode(self.signing_key.sign(&proof_input).to_bytes());

        let request = RedeemRequest {
            version: WIRE_VERSION,
            capability_id: capability.id,
            nonce: &nonce,
            workload_id: &self.workload_id,
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
            Ok(0) => Ok(Secret(secret)),
            Ok(_) | Err(_) => Err(CapabilityError::RedemptionDenied),
        }
    }
}

#[derive(Serialize)]
struct RedeemRequest<'a> {
    version: &'static str,
    capability_id: &'a str,
    nonce: &'a str,
    workload_id: &'a str,
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

fn read_owner_key(path: &Path) -> Result<SigningKey, CapabilityError> {
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

fn parse_signing_key(encoded: &[u8]) -> Result<SigningKey, CapabilityError> {
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
