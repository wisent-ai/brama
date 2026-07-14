//! AES-256-GCM compatible with the Node crypto impl in
//! trading-autonomy/web/lib/infra/core/encryption.ts.
//!
//! Format on the wire: `{iv_base64}:{authTag_base64}:{ciphertext_base64}`
//! Key derivation from `ENCRYPTION_KEY` (or `CARD_ENCRYPTION_KEY`): 64-char
//! hex → 32 bytes; 44-char base64 → 32 bytes; otherwise scrypt with salt
//! `wisent-salt` and Node default params N=16384, r=8, p=1.

use std::env;

use aes_gcm::aead::consts::U16;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::aes::Aes256;
use aes_gcm::AesGcm;
use aes_gcm::{Key, Nonce};

// Node's createCipheriv('aes-256-gcm', key, iv) with a 16-byte IV runs GHASH
// over the nonce per NIST SP 800-38D §7.2. Match that by using the 16-byte
// nonce-size generic parameter of AesGcm so ciphertexts produced by
// trading-autonomy web still decrypt here.
type Aes256GcmWide = AesGcm<Aes256, U16>;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use scrypt::{scrypt, Params};
use thiserror::Error;

const SCRYPT_SALT: &[u8] = b"wisent-salt";
const IV_LEN: usize = 16;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("ENCRYPTION_KEY (or CARD_ENCRYPTION_KEY) env var not set")]
    KeyMissing,
    #[error("invalid encrypted data format: {0}")]
    Format(String),
    #[error("base64 decode: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("hex decode: {0}")]
    Hex(#[from] hex::FromHexError),
    #[error("aes-gcm error: {0}")]
    Aead(String),
    #[error("scrypt params: {0}")]
    ScryptParams(String),
    #[error("scrypt derive: {0}")]
    ScryptDerive(String),
    #[error("utf8 decode: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

fn get_key() -> Result<[u8; 32], CryptoError> {
    let key_str = env::var("CARD_ENCRYPTION_KEY")
        .or_else(|_| env::var("ENCRYPTION_KEY"))
        .unwrap_or_default();
    if key_str.is_empty() {
        return Err(CryptoError::KeyMissing);
    }
    if key_str.len() == 64 {
        let bytes = hex::decode(&key_str)?;
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        return Ok(out);
    }
    if key_str.len() == 44 {
        let bytes = B64.decode(&key_str)?;
        if bytes.len() == 32 {
            let mut out = [0u8; 32];
            out.copy_from_slice(&bytes);
            return Ok(out);
        }
    }
    // Non-hex non-base64 strings: derive via scrypt (Node defaults N=16384, r=8, p=1).
    let params = Params::new(14, 8, 1, 32).map_err(|e| CryptoError::ScryptParams(e.to_string()))?;
    let mut out = [0u8; 32];
    scrypt(key_str.as_bytes(), SCRYPT_SALT, &params, &mut out)
        .map_err(|e| CryptoError::ScryptDerive(e.to_string()))?;
    Ok(out)
}

/// Encrypt a UTF-8 string; produces `{iv_b64}:{tag_b64}:{ct_b64}`.
/// IV is random 16 bytes (AES-GCM spec prefers 12 but Node's default for
/// createCipheriv with aes-256-gcm accepts any length ≥1; the TS impl uses
/// 16 so we match it exactly).
pub fn encrypt(plaintext: &str) -> Result<String, CryptoError> {
    let key_bytes = get_key()?;
    let key = Key::<Aes256GcmWide>::from_slice(&key_bytes);
    let cipher = Aes256GcmWide::new(key);

    let mut iv = [0u8; IV_LEN];
    use rand_iv::fill;
    fill(&mut iv);

    let nonce = Nonce::from_slice(&iv);
    let ciphertext_and_tag = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| CryptoError::Aead(e.to_string()))?;

    // aes-gcm's encrypt() output is ciphertext || tag. Split off the 16-byte
    // tag to mirror the Node layout (separate tag field).
    let split = ciphertext_and_tag.len().saturating_sub(16);
    let (ct, tag) = ciphertext_and_tag.split_at(split);

    Ok(format!(
        "{}:{}:{}",
        B64.encode(iv),
        B64.encode(tag),
        B64.encode(ct)
    ))
}

/// Decrypt a string produced by `encrypt()` or by the TS impl.
pub fn decrypt(encrypted: &str) -> Result<String, CryptoError> {
    let parts: Vec<&str> = encrypted.splitn(3, ':').collect();
    if parts.len() != 3 {
        return Err(CryptoError::Format("expected iv:tag:ct".into()));
    }
    let iv = B64.decode(parts[0])?;
    let tag = B64.decode(parts[1])?;
    let ct = B64.decode(parts[2])?;

    let key_bytes = get_key()?;
    let key = Key::<Aes256GcmWide>::from_slice(&key_bytes);
    let cipher = Aes256GcmWide::new(key);

    let mut payload = Vec::with_capacity(ct.len() + tag.len());
    payload.extend_from_slice(&ct);
    payload.extend_from_slice(&tag);

    let nonce = Nonce::from_slice(&iv);
    let plain = cipher
        .decrypt(nonce, payload.as_ref())
        .map_err(|e| CryptoError::Aead(e.to_string()))?;
    Ok(String::from_utf8(plain)?)
}

/// Matches the TS `isEncrypted()` helper — shape check only, no crypto.
pub fn is_encrypted(data: &str) -> bool {
    let parts: Vec<&str> = data.splitn(3, ':').collect();
    if parts.len() != 3 {
        return false;
    }
    B64.decode(parts[0]).is_ok() && B64.decode(parts[1]).is_ok()
}

mod rand_iv {
    use aes_gcm::aead::rand_core::{OsRng, RngCore};
    pub fn fill(buf: &mut [u8]) {
        OsRng.fill_bytes(buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that mutate process-wide env so `cargo test` (which
    // runs in parallel by default) doesn't race on ENCRYPTION_KEY.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn roundtrip_hex_key() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "ENCRYPTION_KEY",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        );
        std::env::remove_var("CARD_ENCRYPTION_KEY");
        let ct = encrypt("hello world").unwrap();
        assert!(is_encrypted(&ct));
        let pt = decrypt(&ct).unwrap();
        assert_eq!(pt, "hello world");
    }

    #[test]
    fn roundtrip_scrypt_key() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("ENCRYPTION_KEY", "short-passphrase");
        std::env::remove_var("CARD_ENCRYPTION_KEY");
        let ct = encrypt("sensitive").unwrap();
        let pt = decrypt(&ct).unwrap();
        assert_eq!(pt, "sensitive");
    }

    #[test]
    fn format_check_rejects_junk() {
        assert!(!is_encrypted("no-colons"));
        assert!(!is_encrypted("only:two"));
        assert!(!is_encrypted("!!not:@@base64:##junk"));
    }
}
