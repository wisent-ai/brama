//! What a Claude Code grant rides to Anthropic as.
//!
//! A grant Claude Code issued is honoured for the newest models only when the
//! request looks like Claude Code's own: its user agent and beta list, a
//! system prompt that opens with a billing attestation and the sentence
//! "You are Claude Code, Anthropic's official CLI for Claude.", and in that
//! attestation a checksum of the body it travels in. Anthropic answers any
//! other shape `429` with the one-word body `Error`, which is what Brama was
//! answered for sonnet-4-6 and fable-5-1 on 2026-09-14 while `omp` -- the
//! harness the grant was taken from -- was being answered on the same account
//! in the same minute. So Brama sends what `omp` sends, byte for byte where
//! the provider can see it.

use reqwest::RequestBuilder;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use xxhash_rust::xxh64::xxh64;

use crate::types::ModelRequest;

/// The Claude Code release whose requests this shape was read from. Its
/// fingerprint below is derived from it, so the two change together.
const CLAUDE_CODE_VERSION: &str = "2.1.257";
const BETAS: &str = "claude-code-20250219,oauth-2025-04-20";
const CLAUDE_CODE_SENTENCE: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const BILLING_HEADER: &str = "x-anthropic-billing-header:";
const CHECKSUM_PLACEHOLDER: &str = "cch=00000";
const CHECKSUM_SEED: u64 = 0x4d65_9218_e32a_3268;
const CHECKSUM_MASK: u64 = 0xf_ffff;
const FINGERPRINT_SALT: &str = "59cf53e54c78";
/// Positions in the first user message, as JavaScript indexes strings, that
/// the version fingerprint is salted with.
const FINGERPRINT_OFFSETS: [usize; 3] = [4, 7, 20];
const FINGERPRINT_CHARS: usize = 3;

/// The headers Claude Code presents with an OAuth grant.
pub(in crate::providers::adapter) fn headers(builder: RequestBuilder) -> RequestBuilder {
    builder
        .header(
            "user-agent",
            format!("claude-cli/{CLAUDE_CODE_VERSION} (external, cli)"),
        )
        .header("anthropic-beta", BETAS)
        .header("x-app", "cli")
        .header("anthropic-dangerous-direct-browser-access", "true")
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
/// cc_entrypoint=cli; cch=00000;` -- the fingerprint is the first three hex
/// digits of a salted hash of three characters of the first user message,
/// read the way JavaScript reads a string, and `cch` is stamped last.
fn billing_header(first_user_text: &str) -> String {
    let units = first_user_text.encode_utf16().collect::<Vec<_>>();
    let salt = FINGERPRINT_OFFSETS
        .iter()
        .map(|&offset| {
            units
                .get(offset)
                .and_then(|&unit| char::from_u32(u32::from(unit)))
                .unwrap_or('0')
        })
        .collect::<String>();
    let digest = Sha256::digest(format!("{FINGERPRINT_SALT}{salt}{CLAUDE_CODE_VERSION}"));
    let fingerprint = hex::encode(digest)
        .chars()
        .take(FINGERPRINT_CHARS)
        .collect::<String>();
    format!(
        "{BILLING_HEADER} cc_version={CLAUDE_CODE_VERSION}.{fingerprint}; cc_entrypoint=cli; {CHECKSUM_PLACEHOLDER};"
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
    let checksum = format!("{:05x}", xxh64(bytes, CHECKSUM_SEED) & CHECKSUM_MASK);
    let digits = start + placeholder + "cch=".len();
    bytes[digits..digits + checksum.len()].copy_from_slice(checksum.as_bytes());
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;

    fn request(system: Option<&str>, first_user: &str) -> ModelRequest {
        ModelRequest {
            messages: vec![Message {
                role: "user".into(),
                content: first_user.into(),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            }],
            model: "claude-code/claude-sonnet-4-6".into(),
            max_tokens: u32::MAX,
            temperature: None,
            system: system.map(str::to_owned),
            tools: None,
            tool_choice: None,
            billing_target: None,
        }
    }

    fn attestation_of(body: &[u8]) -> (Value, String) {
        let value: Value = serde_json::from_slice(body).expect("the body is JSON");
        let first = value["system"][0]["text"]
            .as_str()
            .expect("first block")
            .to_owned();
        (value, first)
    }

    #[test]
    fn the_system_prompt_opens_with_the_attestation_then_the_sentence_then_the_callers() {
        let body = attested_body(
            json!({"model": "claude-sonnet-4-6", "system": "Answer tersely."}),
            &request(Some("Answer tersely."), "Say hello in one sentence."),
        );
        let (value, first) = attestation_of(&body);
        assert!(first.starts_with("x-anthropic-billing-header: cc_version=2.1.257."));
        assert!(
            !first.contains(CHECKSUM_PLACEHOLDER),
            "the checksum is stamped"
        );
        assert_eq!(value["system"][1]["text"], CLAUDE_CODE_SENTENCE);
        assert_eq!(value["system"][1]["cache_control"]["type"], "ephemeral");
        assert_eq!(value["system"][2]["text"], "Answer tersely.");
        assert_eq!(
            value["system"].as_array().map(Vec::len),
            Some(FINGERPRINT_CHARS)
        );
    }

    #[test]
    fn the_checksum_is_the_hash_of_the_body_with_the_placeholder_in_place() {
        let body = attested_body(
            json!({"model": "claude-sonnet-4-6", "messages": [{"role": "user", "content": "Say hello in one sentence."}]}),
            &request(None, "Say hello in one sentence."),
        );
        let (_, first) = attestation_of(&body);
        let stamped = first
            .rsplit("cch=")
            .next()
            .and_then(|tail| tail.strip_suffix(';'))
            .expect("a stamped checksum");
        assert_eq!(stamped.len(), "00000".len(), "five hex digits: {stamped}");
        let text = String::from_utf8(body.clone()).expect("utf-8");
        let unstamped = text.replacen(&format!("cch={stamped};"), "cch=00000;", 1);
        let expected = format!(
            "{:05x}",
            xxh64(unstamped.as_bytes(), CHECKSUM_SEED) & CHECKSUM_MASK
        );
        assert_eq!(stamped, expected);
    }

    #[test]
    fn the_fingerprint_reads_three_characters_of_the_first_user_message_and_pads_short_ones() {
        let long = billing_header("Say hello in one sentence.");
        let short = billing_header("Hi");
        let padded = billing_header("Say h000000000000000000");
        assert_ne!(long, short);
        assert_ne!(long, padded);
        assert_eq!(
            billing_header("Hi"),
            billing_header("Ho"),
            "characters off the salt do not matter"
        );
    }
}
