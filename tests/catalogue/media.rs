//! The three media endpoints and the refusals that keep a wrong model name
//! legible.
//!
//! Nothing here reaches a provider. Every story asserts a refusal decided
//! before a credential is redeemed, which is exactly where these contracts
//! have to hold: a caller that named a chat model on the image endpoint must
//! learn that from Brama, not from somebody else's 404 minutes later.

use reqwest::Method;
use serde_json::json;

use brama::core::server::{IMAGE_ALIAS, VIDEO_ALIAS, VOICE_ALIAS};

use crate::gateway::{Gateway, AGENT, AGENT_BEARER, AGENT_SIGNING_SECRET, CONSOLE_BEARER};

use super::facets::{post, read, ACCOUNTS};

/// Two requests deliberately outside the endpoint ceilings, so the refusal
/// is the endpoint's own bound rather than a provider's invoice.
const IMAGES_ABOVE_CEILING: u32 = 99;
const SECONDS_ABOVE_CEILING: u32 = 600;

/// The media endpoints refuse a name that cannot produce what they return,
/// and they say which name would.
///
/// `openai/gpt-4o-mini` is the case that matters: its provider does declare
/// an image and a speech path, so only the catalogue's own answer about
/// what that model produces keeps the request from being paid for.
#[test]
fn a_media_endpoint_refuses_a_model_that_cannot_produce_its_artifact() {
    let gateway = Gateway::start("media-wrong-model", ACCOUNTS);
    for (path, body, sentence) in [
        (
            "/v1/images/generations",
            json!({"model": "openai/gpt-4o-mini", "prompt": "a red square"}),
            "`openai/gpt-4o-mini` is a text model: name `image-model` or a provider/model route the catalogue lists as image",
        ),
        (
            "/v1/videos",
            json!({"model": "openai/gpt-4o-mini", "prompt": "a red square"}),
            "`openai/gpt-4o-mini` is a text model: name `video-model` or a provider/model route the catalogue lists as video",
        ),
        (
            "/v1/audio/speech",
            json!({"model": "openai/gpt-4o-mini", "input": "hello", "voice": "alloy"}),
            "`openai/gpt-4o-mini` is a text model: name `voice-model` or a provider/model route the catalogue lists as audio",
        ),
    ] {
        let (status, answer) = post(&gateway, path, &body);
        assert_eq!(status, 400, "{path} must refuse a chat model: {answer}");
        assert_eq!(answer["error"]["message"], sentence, "{answer}");
    }
}

/// A media alias nobody routed is this host's configuration fault, so it
/// answers 503 naming the command that repairs it — never "your model name
/// is wrong".
#[test]
fn an_unrouted_media_alias_names_the_command_that_declares_it() {
    let gateway = Gateway::start("media-unrouted", ACCOUNTS);
    for (path, body, alias) in [
        (
            "/v1/images/generations",
            json!({"model": IMAGE_ALIAS, "prompt": "a red square"}),
            IMAGE_ALIAS,
        ),
        (
            "/v1/videos",
            json!({"model": VIDEO_ALIAS, "prompt": "a red square"}),
            VIDEO_ALIAS,
        ),
        (
            "/v1/audio/speech",
            json!({"model": VOICE_ALIAS, "input": "hello", "voice": "alloy"}),
            VOICE_ALIAS,
        ),
    ] {
        let (status, answer) = post(&gateway, path, &body);
        assert_eq!(status, 503, "{alias} is unrouted here: {answer}");
        let message = answer["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(alias) && message.contains("brama routes set"),
            "the refusal names the repair: {message}"
        );
    }
}

/// A media alias promises an artifact the chat endpoints cannot return, and
/// the chat endpoint says so by name rather than calling the caller's model
/// name malformed.
#[test]
fn the_chat_endpoint_refuses_a_media_alias_by_name() {
    let gateway = Gateway::start("media-chat-refusal", ACCOUNTS);
    let (status, body) = gateway.request(
        "/v1/chat/completions",
        Method::POST,
        Some(AGENT_BEARER),
        Some(&json!({
            "model": IMAGE_ALIAS,
            "messages": [{"role": "user", "content": "draw"}],
        })),
        Some((AGENT, AGENT_SIGNING_SECRET)),
    );
    assert_eq!(
        status, 403,
        "a bearer allowed only `best` cannot name it: {body}"
    );

    let (status, body) = gateway.request(
        "/v1/chat/completions",
        Method::POST,
        Some(CONSOLE_BEARER),
        Some(&json!({
            "model": VOICE_ALIAS,
            "messages": [{"role": "user", "content": "speak"}],
        })),
        None,
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        body["error"]["message"],
        "`voice-model` is a media alias: it generates on POST /v1/images/generations or POST /v1/videos, not chat completions",
        "{body}"
    );
}

/// Reading a video job back needs the model that started it, because this
/// gateway stores nothing about the job.
#[test]
fn a_video_status_read_names_the_model_that_started_the_job() {
    let gateway = Gateway::start("media-video-status", ACCOUNTS);
    let (status, body) = read(&gateway, "/v1/videos/video_123");
    assert_eq!(
        status, 400,
        "a status read with no model is refused: {body}"
    );

    let (status, body) = read(&gateway, "/v1/videos/video_123?model=openai/gpt-4o-mini");
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        body["error"]["message"],
        "`openai/gpt-4o-mini` is a text model: name `video-model` or a provider/model route the catalogue lists as video",
        "{body}"
    );
}

/// The ceilings the endpoints apply before anything is billed.
#[test]
fn a_request_outside_the_accepted_bounds_is_refused_before_a_provider_is_reached() {
    let gateway = Gateway::start("media-bounds", ACCOUNTS);
    let (status, body) = post(
        &gateway,
        "/v1/images/generations",
        &json!({"model": IMAGE_ALIAS, "prompt": "  "}),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        body["error"]["message"], "prompt must not be empty",
        "{body}"
    );

    let mut many = json!({"model": IMAGE_ALIAS, "prompt": "a red square"});
    many["n"] = json!(IMAGES_ABOVE_CEILING);
    let (status, body) = post(&gateway, "/v1/images/generations", &many);
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        body["error"]["message"], "n must be between 1 and 10",
        "{body}"
    );

    let (status, body) = post(
        &gateway,
        "/v1/audio/speech",
        &json!({"model": VOICE_ALIAS, "input": "hello", "voice": " "}),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        body["error"]["message"], "voice must name one of the provider's voices",
        "{body}"
    );

    let mut long = json!({"model": VIDEO_ALIAS, "prompt": "a red square"});
    long["seconds"] = json!(SECONDS_ABOVE_CEILING);
    let (status, body) = post(&gateway, "/v1/videos", &long);
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        body["error"]["message"], "seconds must be between 1 and 60",
        "{body}"
    );
}

/// A route that cannot render is refused where it is declared, not where it
/// is called: the image alias promises an image, so the registry never takes
/// a chat model for it.
#[test]
fn the_registry_refuses_a_media_alias_pointed_at_a_chat_model() {
    let gateway = Gateway::start("media-declared-route", ACCOUNTS);
    let (status, body) = gateway.console(
        "/v1/admin/routes",
        Method::PUT,
        Some(&json!({"alias": IMAGE_ALIAS, "primary": "openai/gpt-4o-mini"})),
    );
    assert_eq!(
        status, 400,
        "a chat route cannot stand behind the image alias: {body}"
    );

    let (status, body) = read(&gateway, "/v1/aliases");
    assert_eq!(status, 200, "{body}");
    let aliases = body["aliases"].as_array().expect("an alias list");
    assert!(
        aliases
            .iter()
            .all(|entry| entry["alias"] != IMAGE_ALIAS || entry["state"] != "serving"),
        "an undeclared media alias never reads as serving: {body}"
    );
}
