//! The credential commands that end in one manual verdict: the by-hand
//! sign-in on a terminal, done here or by the gateway that serves the pool
//! from another host, and the grant taken from a harness.

use brama::subscription_dispatch::sign_in::manual;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::remote::Destination;

/// Print one manual verdict the way every credential command does, and exit
/// unsuccessfully unless the account is signed in.
pub(crate) fn finish(verdict: Result<Value, String>, json: bool) {
    match verdict {
        Ok(verdict) => {
            let field = |name: &str| verdict.get(name).and_then(Value::as_str);
            if json {
                crate::cli::print_json(&verdict);
            } else {
                if let Some(provider) = field("provider") {
                    println!("provider: {provider}");
                }
                if let Some(subscription) = field("subscription_id") {
                    println!("subscription: {subscription}");
                }
                if let Some(account) = field("account") {
                    println!("account: {account}");
                }
                if let Some(result) = field("result") {
                    println!("result: {result}");
                }
                if let Some(detail) = field("detail") {
                    println!("detail: {detail}");
                }
            }
            let result = field("result");
            if result != Some("signed_in") && result != Some("unchanged") {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

/// The manual sign-in on a terminal: the page to open, the paste, and the
/// shared exchange-store-prove that Brama Desktop's route also ends in. The
/// code is a credential, so it is read from stdin (a terminal paste or a
/// pipe), never taken on the command line.
///
/// Naming a gateway hands both steps to the gateway that serves the pool: it
/// draws the verifier, keeps it, exchanges the pasted code and stores the
/// grant in its own vault, so a sign-in run on a workstation reaches the
/// pool that answers requests instead of this host's leftover vault copy.
pub(crate) async fn sign_in(
    provider: &str,
    subscription_id: &str,
    reason: &str,
    destination: Destination,
) -> Result<Value, String> {
    if reason.trim().is_empty() {
        return Err("--reason must say why this sign-in is being run".into());
    }
    if destination.gateway.is_some() || destination.gateway_consumer.is_some() {
        return sign_in_on_gateway(provider, subscription_id, reason, destination).await;
    }
    // The grant is stored in the vault this host's Skarbiec holds, so a host
    // that holds none (Skarbiec refuses its leftover copy) is refused here,
    // before the operator logs in for a grant the gateway would never read.
    brama::gateway::broker::subscription_account(subscription_id, provider)
        .await
        .map_err(|said| {
            format!(
                "no sign-in was started: the grant could not be stored on this host ({said}); \
                 name the gateway that serves the pool with --gateway-consumer <CONSUMER> \
                 --bearer-role <ROLE> (or --gateway <URL> --bearer-role <ROLE>) so it stores \
                 the grant in its own vault"
            )
        })?;
    let request = manual::begin(provider, subscription_id)?;
    let pasted = paste(&request.url)?;
    let verdict = manual::complete(request, &pasted, reason).await?;
    serde_json::to_value(&verdict)
        .map_err(|error| format!("the verdict did not serialize: {error}"))
}

/// Print the page to open and read the one pasted line.
fn paste(url: &str) -> Result<Zeroizing<String>, String> {
    eprintln!("Open this page in your own browser and log in:");
    eprintln!();
    eprintln!("  {url}");
    eprintln!();
    eprintln!(
        "When it shows a code, paste it here (the `code#state` text, or the whole redirect URL):"
    );
    let mut pasted = Zeroizing::new(String::new());
    std::io::stdin()
        .read_line(&mut pasted)
        .map_err(|error| format!("reading the pasted code: {error}"))?;
    Ok(pasted)
}

/// Both steps on the gateway: `POST /v1/admin/subscription-pool/sign-in` with
/// `"by": "hand"` for the page, then the paste to `…/sign-in/:sign_in_id`.
async fn sign_in_on_gateway(
    provider: &str,
    subscription_id: &str,
    reason: &str,
    destination: Destination,
) -> Result<Value, String> {
    // stdin carries the pasted code, so the console's bearer cannot come from
    // it as it does for the other gateway commands.
    if destination.bearer_role.is_none() {
        return Err(
            "a sign-in on a gateway reads the console's bearer with --bearer-role <ROLE>: stdin \
             carries the pasted code"
                .into(),
        );
    }
    let (origin, bearer) = destination.resolve(&Zeroizing::new(String::new())).await?;
    let origin = origin.ok_or("name --gateway or --gateway-consumer to sign in on a gateway")?;
    let origin = origin.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| format!("gateway client: {error}"))?;
    let begun = post(
        &client,
        &format!("{origin}/v1/admin/subscription-pool/sign-in"),
        bearer.trim(),
        &json!({"subscription_id": subscription_id, "reason": reason, "by": "hand"}),
        "begin the sign-in",
    )
    .await?;
    let text = |name: &str| {
        begun
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("the gateway began the sign-in without a {name}: {begun}"))
    };
    let gateway_provider = text("provider")?;
    if gateway_provider != provider.trim() {
        return Err(format!(
            "no sign-in was started: the gateway holds {subscription_id} as a {gateway_provider} \
             subscription, not {}",
            provider.trim()
        ));
    }
    let sign_in_id = text("sign_in_id")?;
    let pasted = paste(text("url")?)?;
    let code = pasted.trim();
    if code.is_empty() {
        return Err("nothing was pasted".into());
    }
    post(
        &client,
        &format!("{origin}/v1/admin/subscription-pool/sign-in/{sign_in_id}"),
        bearer.trim(),
        &json!({"code": code}),
        "complete the sign-in",
    )
    .await
}

/// One admin request to the gateway: its JSON answer, or what it refused,
/// with the whole answer it gave.
async fn post(
    client: &reqwest::Client,
    url: &str,
    bearer: &str,
    body: &Value,
    action: &str,
) -> Result<Value, String> {
    let response = client
        .post(url)
        .bearer_auth(bearer)
        .json(body)
        .send()
        .await
        .map_err(|error| format!("the gateway at {url} did not answer: {error}"))?;
    let status = response.status();
    let answer: Value = response.json().await.map_err(|error| {
        format!("the gateway's answer to {action} ({status}) is not JSON: {error}")
    })?;
    if !status.is_success() {
        return Err(format!(
            "the gateway refused to {action}: {status}: {answer}"
        ));
    }
    Ok(answer)
}
