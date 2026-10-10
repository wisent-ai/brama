//! The selected gateway owns authentication; the CLI only carries progress and verdicts.
use super::super::{remote, text, verdicts::print_sign_in};
use brama::subscription_dispatch::sign_in::{progress_sentence, SignInOptions};
use serde_json::{json, Value};

pub(super) async fn run(
    mut options: SignInOptions,
    destination: remote::Destination,
    as_json: bool,
) {
    let answer = async {
        if (destination.gateway.is_some() || destination.gateway_consumer.is_some())
            && options
                .subscription_id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty())
        {
            return Err(
                "remote Weles sign-in requires --subscription-id for the exact account".to_owned(),
            );
        }
        let (gateway, bearer) = destination.resolve_reading_stdin().await?;
        if let Some(gateway) = gateway {
            return on_gateway(&gateway, bearer.trim(), &options).await;
        }
        options.progress = Some(std::sync::Arc::new(|event| {
            if let Some(sentence) = progress_sentence(event) {
                eprintln!("{sentence}");
            }
        }));
        brama::subscription_dispatch::sign_in::sign_in_provider(options)
            .await
            .map_err(|error| error.to_string())
    }
    .await;
    match answer {
        Ok(verdict) => {
            if as_json {
                crate::cli::print_json(&verdict);
            } else {
                print_sign_in(&verdict);
            }
            if text(&verdict, "result") != Some("signed_in") {
                std::process::exit(libc::EXIT_FAILURE);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(libc::EXIT_FAILURE);
        }
    }
}

async fn on_gateway(gateway: &str, bearer: &str, options: &SignInOptions) -> Result<Value, String> {
    let member = options
        .subscription_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            "remote Weles sign-in requires --subscription-id for the exact account".to_owned()
        })?;
    if options.reason.trim().is_empty() {
        return Err("--reason must say why this sign-in is being run".into());
    }
    let pool = stado_wait::until(
        stado_wait::Kind::Network,
        "read selected subscription before sign-in",
        gateway,
        remote::pool_report(gateway, bearer),
    )
    .await?;
    let rows = pool["subscriptions"]
        .as_array()
        .ok_or_else(|| "gateway pool report lacks subscriptions".to_owned())?;
    let entry = rows
        .iter()
        .find(|entry| entry["id"] == member)
        .ok_or_else(|| format!("gateway has no subscription {member}"))?;
    if entry["provider"] != options.provider {
        return Err(format!(
            "no sign-in was started: the gateway holds {member} as {}, not {}",
            entry["provider"], options.provider
        ));
    }
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| format!("gateway sign-in client: {error}"))?;
    let mut response = stado_wait::http::request(
        client
            .post(format!(
                "{}/v1/admin/subscription-pool/sign-in",
                gateway.trim_end_matches('/')
            ))
            .bearer_auth(bearer)
            .json(&json!({"by": "weles", "subscription_id": member,
            "reason": options.reason, "login_item": options.login_item})),
    )
    .await
    .map_err(|error| format!("gateway sign-in request: {error}; execution is unconfirmed"))?;
    let status = response.status();
    if !status.is_success() {
        let body = stado_wait::until(
            stado_wait::Kind::Network,
            "read gateway sign-in refusal",
            gateway,
            response.text(),
        )
        .await
        .map_err(|error| format!("gateway sign-in HTTP {status}: unreadable refusal: {error}"))?;
        return Err(format!("gateway sign-in HTTP {status}: {body}"));
    }
    let mut pending = Vec::new();
    loop {
        let chunk = stado_wait::until(
            stado_wait::Kind::Network,
            "receive gateway sign-in progress",
            gateway,
            response.chunk(),
        )
        .await
        .map_err(|error| {
            format!("gateway sign-in stream failed: {error}; execution is unconfirmed")
        })?;
        let Some(chunk) = chunk else {
            return Err(
                "gateway sign-in stream ended without a verdict; execution is unconfirmed".into(),
            );
        };
        pending.extend_from_slice(&chunk);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let mut event: Value = serde_json::from_slice(&pending[..end]).map_err(|error| {
                format!("gateway sign-in event is invalid JSON: {error}; execution is unconfirmed")
            })?;
            pending.drain(..=end);
            match event["event"].as_str() {
                Some("verdict") => {
                    let verdict = &event["verdict"];
                    if verdict["subscription_id"] != member
                        || verdict["provider"] != options.provider
                    {
                        return Err("gateway sign-in verdict names a different subscription or provider; execution is unconfirmed".into());
                    }
                    return Ok(event["verdict"].take());
                }
                Some("refused") => return Err(format!("gateway sign-in refused: {event}")),
                _ => {
                    if let Some(sentence) = event["sentence"].as_str() {
                        eprintln!("{sentence}");
                    } else if let Some(sentence) = progress_sentence(&event) {
                        eprintln!("{sentence}");
                    } else {
                        eprintln!("gateway sign-in progress: {event}");
                    }
                }
            }
        }
    }
}
