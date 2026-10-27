//! The signature on an answer Brama serves: proof, to a program that holds
//! Brama's public key, that this answer is the one answer Brama gives this
//! exact request.
//!
//! A program that keeps a model's answer to decide later calls (Tama's kept
//! judgements) runs as the same user as the sessions it judges, so anything
//! on that machine can write an answer, point the model call at a service of
//! its own, or ask Brama the same question again until the model says what
//! it wants. Brama runs on its own host. With `BRAMA_ANSWER_SIGNING_KEY_FILE`
//! naming an Ed25519 key that only Brama's user can read (the checks the
//! workload key gets: owner, mode, type, size, no symlink), a buffered chat
//! completion whose request carries `x-brama-answer-once` is answered once
//! per exact request body: the first answer is kept under
//! `$BRAMA_STATE_DIR/signed-answers/<SHA-256 of the request>.json` and every
//! later ask of the same body is answered with those same bytes and
//! signature, without asking a model, and marked `x-brama-answer-repeat`.
//! The answer goes out as the exact bytes signed, with
//! `x-brama-answer-signature`, the signature in standard base64 over the
//! domain, SHA-256 of the request body and SHA-256 of the response body, and
//! `x-brama-answer-key`, the lowercase hex SHA-256 of the public key. So a
//! valid signature can only ever be on the first answer to a body. A reader
//! verifies with the public key it was given out of band, never with one this
//! host sends. A request without the header is answered as before and
//! unsigned; without the variable nothing is signed and startup says so; a
//! key that cannot be read stops startup.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::core::server::refusal::api_error;

/// The variable naming the key file.
const KEY_FILE: &str = "BRAMA_ANSWER_SIGNING_KEY_FILE";
/// What the signature is over besides the two digests, so it never verifies
/// as a signature over anything else this key could sign.
const DOMAIN: &[u8] = b"brama answer\0";
const ONCE_HEADER: &str = "x-brama-answer-once";
const REPEAT_HEADER: &str = "x-brama-answer-repeat";
const SIGNATURE_HEADER: &str = "x-brama-answer-signature";
const KEY_HEADER: &str = "x-brama-answer-key";
/// The folder under Brama's state directory the first answers are kept in.
const STORE: &str = "signed-answers";

struct Keyed {
    key: SigningKey,
    id: String,
}

/// The answer key, when one is declared.
#[derive(Clone)]
pub(in crate::core::server) struct AnswerSigner(Option<Arc<Keyed>>);

/// One request's one signed answer, as kept.
#[derive(Serialize, Deserialize)]
struct Kept {
    response: String,
    signature: String,
    key: String,
}

impl AnswerSigner {
    /// The key `BRAMA_ANSWER_SIGNING_KEY_FILE` names, read with the workload
    /// key's checks; none when the variable is not set. A declaration that
    /// cannot be read, or a key file that fails a check, refuses startup.
    pub(in crate::core::server) fn from_env() -> Result<Self, std::io::Error> {
        let refused =
            |reason: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, reason);
        let Some(path) = crate::environment::declared_setting(KEY_FILE).map_err(refused)? else {
            warn!("{KEY_FILE} is not set: chat completions are served unsigned, and no reader can keep one as Brama's");
            return Ok(Self(None));
        };
        let path = Path::new(&path);
        if !path.is_absolute() {
            return Err(refused(format!(
                "{KEY_FILE} must be an absolute path; observed {}",
                path.display()
            )));
        }
        let key = crate::capability::read_owner_key(path)
            .map_err(|error| refused(format!("{KEY_FILE} {}: {error}", path.display())))?;
        let public = key.verifying_key();
        let id = hex::encode(Sha256::digest(public.as_bytes()));
        info!(
            key = %id,
            public = %STANDARD.encode(public.as_bytes()),
            "chat completions are signed with the answer key"
        );
        Ok(Self(Some(Arc::new(Keyed { key, id }))))
    }

    /// Whether this request asks for its one signed answer and a key can
    /// sign it.
    fn once<'a>(&'a self, headers: &HeaderMap) -> Option<&'a Keyed> {
        headers
            .contains_key(ONCE_HEADER)
            .then_some(self.0.as_deref())
            .flatten()
    }

    /// The answer this exact request was given before, when it asks for its
    /// one signed answer and one is kept; a kept answer that cannot be read
    /// is refused naming it, never answered afresh in its place.
    pub(in crate::core::server) fn recalled(
        &self,
        headers: &HeaderMap,
        request: &[u8],
    ) -> Option<Response> {
        self.once(headers)?;
        let path = kept_path(request);
        match std::fs::read(&path) {
            Ok(bytes) => Some(match serde_json::from_slice::<Kept>(&bytes) {
                Ok(kept) => repeated(kept, &path),
                Err(error) => unreadable(&path, &error.to_string()),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(unreadable(&path, &error.to_string())),
        }
    }

    /// `body` answered with status 200 as exact JSON bytes: signed and kept
    /// as this request's one answer when it asks for that and a key is
    /// declared (the answer kept first, when another ask of the same body
    /// kept one meanwhile), otherwise unsigned.
    pub(in crate::core::server) fn answered(
        &self,
        headers: &HeaderMap,
        request: &[u8],
        body: &serde_json::Value,
    ) -> Response {
        let bytes = match serde_json::to_vec(body) {
            Ok(bytes) => bytes,
            Err(error) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("the generation could not be written as a chat completion: {error}"),
                )
                .into_response()
            }
        };
        let Some(keyed) = self.once(headers) else {
            return json_response(bytes);
        };
        let mut message = DOMAIN.to_vec();
        message.extend_from_slice(&Sha256::digest(request));
        message.extend_from_slice(&Sha256::digest(&bytes));
        let kept = Kept {
            response: STANDARD.encode(&bytes),
            signature: STANDARD.encode(keyed.key.sign(&message).to_bytes()),
            key: keyed.id.clone(),
        };
        let path = kept_path(request);
        match keep_first(&path, &kept) {
            Ok(None) => signed(bytes, &kept.signature, &kept.key),
            Ok(Some(first)) => repeated(first, &path),
            Err(cause) => api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!(
                    "the signed answer could not be kept as this request's one answer: {cause}"
                ),
            )
            .into_response(),
        }
    }
}

fn kept_path(request: &[u8]) -> PathBuf {
    crate::journal::state_dir()
        .join(STORE)
        .join(format!("{}.json", hex::encode(Sha256::digest(request))))
}

/// Keep `kept` at `path` unless an answer is there already; that answer when
/// it is. Written whole beside its name and linked into place, which fails
/// when the name exists, so two asks of one body keep one answer.
fn keep_first(path: &Path, kept: &Kept) -> Result<Option<Kept>, String> {
    let folder = path
        .parent()
        .ok_or_else(|| format!("{} has no folder", path.display()))?;
    std::fs::create_dir_all(folder)
        .map_err(|error| format!("{} cannot be made: {error}", folder.display()))?;
    // Its own name per write, so two asks of one body in this process never
    // write the same staged file.
    let staged = path.with_extension(format!("{}.partial", uuid::Uuid::new_v4().simple()));
    let encoded = serde_json::to_vec(kept)
        .map_err(|error| format!("the answer cannot be encoded: {error}"))?;
    std::fs::File::create(&staged)
        .and_then(|mut file| file.write_all(&encoded))
        .map_err(|error| format!("{} cannot be written: {error}", staged.display()))?;
    let linked = std::fs::hard_link(&staged, path);
    std::fs::remove_file(&staged)
        .map_err(|error| format!("{} cannot be removed: {error}", staged.display()))?;
    match linked {
        Ok(()) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = std::fs::read(path)
                .map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
            serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|error| format!("{} is not a kept answer: {error}", path.display()))
        }
        Err(error) => Err(format!("{} cannot be kept: {error}", path.display())),
    }
}

fn json_response(bytes: Vec<u8>) -> Response {
    let mut response = (StatusCode::OK, bytes).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// `bytes` with the signature headers.
fn signed(bytes: Vec<u8>, signature: &str, key: &str) -> Response {
    let mut response = json_response(bytes);
    // Base64 and lowercase hex are visible ASCII, which every header value
    // admits; a value refused here is said, not dropped.
    for (name, text) in [(SIGNATURE_HEADER, signature), (KEY_HEADER, key)] {
        match HeaderValue::from_str(text) {
            Ok(value) => {
                response
                    .headers_mut()
                    .insert(HeaderName::from_static(name), value);
            }
            Err(error) => {
                return api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("the answer's {name} header cannot be written: {error}"),
                )
                .into_response()
            }
        }
    }
    response
}

/// The kept answer sent again, marked a repeat.
fn repeated(kept: Kept, path: &Path) -> Response {
    let bytes = match STANDARD.decode(&kept.response) {
        Ok(bytes) => bytes,
        Err(error) => return unreadable(path, &error.to_string()),
    };
    let mut response = signed(bytes, &kept.signature, &kept.key);
    response.headers_mut().insert(
        HeaderName::from_static(REPEAT_HEADER),
        HeaderValue::from_static("true"),
    );
    response
}

fn unreadable(path: &Path, cause: &str) -> Response {
    api_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        &format!("this request's kept signed answer {} cannot be read ({cause}); it is not answered afresh", path.display()),
    )
    .into_response()
}
