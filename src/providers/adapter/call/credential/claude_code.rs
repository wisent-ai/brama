//! What a Claude Code grant rides to Anthropic as.
//!
//! A grant Claude Code issued is honoured for the newest models only when the
//! request looks like Claude Code's own: its user agent and beta list, a
//! system prompt that opens with a billing attestation and the sentence
//! "You are a Claude agent, built on Anthropic's Claude Agent SDK.", and in that
//! attestation a checksum of the body it travels in. Anthropic answers any
//! other shape `429` with the one-word body `Error`, even while the harness
//! the grant was taken from is being answered on the same account in the
//! same minute. So Brama sends what that harness sends, byte for byte where
//! the provider can see it.

use reqwest::RequestBuilder;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use xxhash_rust::xxh64::xxh64;

use crate::types::ModelRequest;

/// One capture of the Claude Code release Brama presents as: every value
/// below was read from the requests that one release sends, so they change
/// together, never one at a time. A request still shaped as an older CLI
/// entrypoint is answered `Third-party apps now draw from your extra usage,
/// not your plan limits`, and the billing fingerprint is derived from the
/// release, so a value read from another release breaks the shape.
struct Capture {
    /// The Claude Code release the capture was read from.
    release: &'static str,
    /// The Agent SDK release that Claude Code release presents as.
    agent_sdk: &'static str,
    /// The desktop client version it sends in `anthropic-client-version`.
    client: &'static str,
    /// Its agent request betas, in its own order.
    betas: &'static str,
    /// The Stainless SDK headers it sends before the host's architecture and
    /// operating system, name and value, in its order.
    stainless: &'static [(&'static str, &'static str)],
    /// The Stainless SDK headers it sends after them: the SDK's own client
    /// deadline, sent as captured. It states no wait Brama holds; it stays
    /// until a plan-billed request is seen answered without it.
    stainless_after_host: &'static [(&'static str, &'static str)],
    /// The salt, the UTF-16 positions in the first user message and the
    /// count of hex digits of the version fingerprint.
    fingerprint_salt: &'static str,
    fingerprint_offsets: &'static [usize],
    fingerprint_chars: usize,
    /// The xxHash64 seed and the bit mask of the body checksum.
    checksum_seed: u64,
    checksum_mask: u64,
}

const CLAUDE_CODE: Capture = Capture {
    release: "2.1.165",
    agent_sdk: "0.3.165",
    client: "1.11187.4",
    betas: "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,mid-conversation-system-2026-04-07,advanced-tool-use-2025-11-20,extended-cache-ttl-2025-04-11",
    stainless: &[
        ("x-stainless-retry-count", "0"),
        ("x-stainless-runtime-version", "v24.3.0"),
        ("x-stainless-package-version", "0.94.0"),
        ("x-stainless-runtime", "node"),
        ("x-stainless-lang", "js"),
    ],
    stainless_after_host: &[("x-stainless-timeout", "900")],
    fingerprint_salt: "59cf53e54c78",
    fingerprint_offsets: &[4, 7, 20],
    fingerprint_chars: 3,
    checksum_seed: 0x4d65_9218_e32a_3268,
    checksum_mask: 0xf_ffff,
};
const CLAUDE_CODE_SENTENCE: &str = "You are a Claude agent, built on Anthropic's Claude Agent SDK.";
const ENTRYPOINT: &str = "local-agent";
const BILLING_HEADER: &str = "x-anthropic-billing-header:";
const CHECKSUM_PLACEHOLDER: &str = "cch=00000";

/// The headers Claude Code presents with an OAuth grant: the Agent SDK
/// user agent, the beta list, the Stainless client headers the SDK sends,
/// and a fresh client request id.
pub(in crate::providers::adapter) fn headers(builder: RequestBuilder) -> RequestBuilder {
    let builder = builder
        .header(
            "user-agent",
            format!(
                "claude-cli/{} (external, local-agent, agent-sdk/{})",
                CLAUDE_CODE.release, CLAUDE_CODE.agent_sdk
            ),
        )
        .header("anthropic-beta", CLAUDE_CODE.betas)
        .header("x-app", "cli")
        .header("anthropic-dangerous-direct-browser-access", "true")
        .header("anthropic-client-platform", "desktop_app")
        .header("anthropic-client-version", CLAUDE_CODE.client);
    let builder = captured(builder, CLAUDE_CODE.stainless)
        .header("x-stainless-arch", stainless_arch())
        .header("x-stainless-os", stainless_os());
    captured(builder, CLAUDE_CODE.stainless_after_host)
        .header("x-client-request-id", uuid::Uuid::new_v4().to_string())
}

/// `builder` with each captured header, name and value, in the capture's order.
fn captured(builder: RequestBuilder, headers: &[(&str, &str)]) -> RequestBuilder {
    headers.iter().fold(builder, |builder, (name, value)| {
        builder.header(*name, *value)
    })
}

fn stainless_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "MacOS",
        "windows" => "Windows",
        "linux" => "Linux",
        "freebsd" => "FreeBSD",
        _ => "Other::unknown",
    }
}

fn stainless_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        _ => "other::unknown",
    }
}

/// The body Claude Code would send for this request: the caller's payload
/// with its system prompt opened by the attestation and the Claude Code
/// sentence, serialized, and the attestation's checksum filled in.
pub(in crate::providers::adapter) fn attested_body(
    mut payload: Value,
    request: &ModelRequest,
) -> Vec<u8> {
    let attestation = billing_header(first_user_text(request));
    let mut blocks = vec![
        json!({"type": "text", "text": attestation}),
        json!({"type": "text", "text": CLAUDE_CODE_SENTENCE, "cache_control": {"type": "ephemeral"}}),
    ];
    match payload.get("system") {
        Some(Value::String(system)) if !system.is_empty() => {
            blocks.push(json!({"type": "text", "text": system}));
        }
        Some(Value::Array(existing)) => blocks.extend(existing.iter().cloned()),
        _ => {}
    }
    payload["system"] = Value::Array(blocks);
    let mut bytes = serde_json::to_vec(&payload).expect("a JSON value serializes");
    stamp_checksum(&mut bytes, &attestation);
    bytes
}

/// The first user message's text: the string it is, or its first text part,
/// or nothing -- the same reading `omp` salts the fingerprint with.
fn first_user_text(request: &ModelRequest) -> &str {
    let Some(message) = request
        .messages
        .iter()
        .find(|message| message.role == "user")
    else {
        return "";
    };
    match &message.content {
        Value::String(text) => text,
        Value::Array(parts) => parts
            .iter()
            .find(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|part| part.get("text"))
            .and_then(Value::as_str)
            .unwrap_or(""),
        _ => "",
    }
}

/// `x-anthropic-billing-header: cc_version=<version>.<fingerprint>;
/// cc_entrypoint=local-agent; cch=00000;` -- the fingerprint is the first
/// three hex digits of a salted hash of three characters of the first user
/// message, read the way JavaScript reads a string, and `cch` is stamped
/// last.
fn billing_header(first_user_text: &str) -> String {
    let units = first_user_text.encode_utf16().collect::<Vec<_>>();
    let salt = CLAUDE_CODE
        .fingerprint_offsets
        .iter()
        .map(|&offset| {
            units
                .get(offset)
                .and_then(|&unit| char::from_u32(u32::from(unit)))
                .unwrap_or('0')
        })
        .collect::<String>();
    let digest = Sha256::digest(format!(
        "{}{salt}{}",
        CLAUDE_CODE.fingerprint_salt, CLAUDE_CODE.release
    ));
    let fingerprint = hex::encode(digest)
        .chars()
        .take(CLAUDE_CODE.fingerprint_chars)
        .collect::<String>();
    format!(
        "{BILLING_HEADER} cc_version={}.{fingerprint}; cc_entrypoint={ENTRYPOINT}; {CHECKSUM_PLACEHOLDER};",
        CLAUDE_CODE.release
    )
}

/// Write the body's checksum over the placeholder inside the attestation:
/// the low twenty bits of xxHash64 of the whole body as it is with the
/// placeholder still in place, as five lowercase hex digits. The attestation
/// is located by its serialized text, quotes included, which no JSON string
/// value can contain unescaped.
fn stamp_checksum(bytes: &mut [u8], attestation: &str) {
    let serialized = serde_json::to_string(attestation).expect("a string serializes");
    let Some(start) = find(bytes, serialized.as_bytes()) else {
        return;
    };
    let Some(placeholder) = find(&bytes[start..], CHECKSUM_PLACEHOLDER.as_bytes()) else {
        return;
    };
    let checksum = format!(
        "{:05x}",
        xxh64(bytes, CLAUDE_CODE.checksum_seed) & CLAUDE_CODE.checksum_mask
    );
    let digits = start + placeholder + "cch=".len();
    bytes[digits..digits + checksum.len()].copy_from_slice(checksum.as_bytes());
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
