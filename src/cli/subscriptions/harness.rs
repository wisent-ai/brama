//! The grants the harnesses on this machine hold, listed and taken.
//!
//! `held` shows what omp, Claude Code, Codex CLI and Kimi Code are signed
//! into here, never the grants themselves. `import` takes one and makes it
//! a subscription's credential: in this process where Brama runs, or
//! through a gateway elsewhere when the console's bearer is on stdin - the
//! desktop beside the harness and the gateway on another host are the
//! usual arrangement.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use zeroize::Zeroizing;

use brama::subscription_dispatch::sign_in::manual::{self, Harness, HeldGrant, ManualSignIn};

/// Handing a grant to a gateway may take a while; the gateway answers 2xx on success.
const POST_TIMEOUT_SECONDS: u64 = 180;
const HTTP_SUCCESS: std::ops::Range<u16> = 200..300;

pub(crate) fn home(override_: Option<&str>) -> PathBuf {
    override_
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()))
}

/// `brama subscription held`: every grant every harness here holds.
pub(crate) fn held(provider: Option<&str>, home_override: Option<&str>, json: bool) {
    let home = home(home_override);
    match manual::held(&home, provider) {
        Ok(grants) => {
            let views: Vec<_> = grants
                .iter()
                .map(|grant| manual::harness::view(grant, &home))
                .collect();
            if json {
                crate::cli::print_json(&serde_json::to_value(&views).expect("views serialize"));
            } else if views.is_empty() {
                println!(
                    "no harness under {} holds a grant{}",
                    home.display(),
                    provider
                        .map(|provider| format!(" for `{provider}`"))
                        .unwrap_or_default()
                );
            } else {
                for view in &views {
                    println!(
                        "{:<7} {:<12} {:<40} {}",
                        view.harness.name(),
                        view.provider,
                        view.account.as_deref().unwrap_or("account not recorded"),
                        view.store
                    );
                }
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

/// The one grant `import` takes: the named harness and account, or the only
/// grant any harness here holds for the provider, or a refusal naming the
/// choices.
pub(crate) fn choose(
    provider: &str,
    from: Option<&str>,
    account: Option<&str>,
    home_override: Option<&str>,
) -> Result<HeldGrant, String> {
    let home = home(home_override);
    let provider: &'static str = brama::subscription_dispatch::dispatch::SUBSCRIPTION_PROVIDERS
        .into_iter()
        .find(|known| *known == provider)
        .ok_or_else(|| format!("no harness holds grants Brama can take for `{provider}`; claude-code, codex and kimi are the ones it can"))?;
    let mut grants = match from {
        Some(name) => {
            let harness = Harness::parse(name).ok_or_else(|| {
                format!(
                    "`{name}` is not a harness Brama reads grants from; name one of {}",
                    Harness::ALL.map(Harness::name).join(", ")
                )
            })?;
            manual::harness::read(harness, provider, &home)?
        }
        None => manual::held(&home, Some(provider))?,
    };
    if let Some(account) = account {
        let wanted = account.trim();
        return grants
            .into_iter()
            .find(|grant| {
                grant
                    .account
                    .as_deref()
                    .is_some_and(|held| held.eq_ignore_ascii_case(wanted))
            })
            .ok_or_else(|| {
                format!("no harness here holds an enabled `{provider}` grant for {wanted}")
            });
    }
    match grants.len() {
        0 => Err(format!(
            "no harness under {} holds an enabled `{provider}` grant; sign one in there first, or sign in by hand",
            home.display()
        )),
        1 => Ok(grants.remove(0)),
        _ => Err(format!(
            "{} grants for `{provider}` are held here; name one with --from and --account: {}",
            grants.len(),
            grants
                .iter()
                .map(|grant| format!(
                    "{} ({})",
                    grant.harness.name(),
                    grant.account.as_deref().unwrap_or("account not recorded")
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Take the grant in this process, where Brama runs.
pub(crate) async fn import_here(
    provider: &str,
    subscription_id: &str,
    reason: &str,
    grant: HeldGrant,
) -> Result<ManualSignIn, String> {
    manual::adopt(
        provider,
        subscription_id.trim(),
        grant.document,
        grant.account,
        manual::Origin::Harness(grant.harness),
        reason,
    )
    .await
}

fn post_timeout() -> Duration {
    Duration::from_secs(POST_TIMEOUT_SECONDS)
}

/// Hand the grant to a gateway elsewhere, as the console does, with the
/// console's bearer read from stdin so it never lands in `argv`. The
/// provider goes with it: an id the pool does not hold yet is created
/// under that provider rather than refused.
pub(crate) async fn import_through(
    gateway: &str,
    provider: &str,
    subscription_id: &str,
    reason: &str,
    grant: HeldGrant,
) -> Result<ManualSignIn, String> {
    let mut bearer = Zeroizing::new(String::new());
    std::io::stdin()
        .read_to_string(&mut bearer)
        .map_err(|error| format!("reading the console bearer from stdin: {error}"))?;
    let bearer = bearer.trim();
    if bearer.is_empty() {
        return Err("--gateway needs the console's bearer on stdin, and stdin was empty".into());
    }
    import_through_with(gateway, bearer, provider, subscription_id, reason, grant).await
}

/// The handover itself, with a bearer the caller already holds.
pub(crate) async fn import_through_with(
    gateway: &str,
    bearer: &str,
    provider: &str,
    subscription_id: &str,
    reason: &str,
    grant: HeldGrant,
) -> Result<ManualSignIn, String> {
    let client = reqwest::Client::builder()
        .timeout(post_timeout())
        .build()
        .map_err(|error| format!("gateway client: {error}"))?;
    let response = client
        .post(format!(
            "{}/v1/admin/subscription-pool/grant",
            gateway.trim_end_matches('/')
        ))
        .bearer_auth(bearer)
        .json(&json!({
            "subscription_id": subscription_id.trim(),
            "provider": provider,
            "reason": reason,
            "harness": grant.harness.name(),
            "account": grant.account,
            "document": &*grant.document,
        }))
        .send()
        .await
        .map_err(|error| format!("the gateway {gateway} did not answer: {error}"))?;
    let status = response.status().as_u16();
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("the gateway's answer is not JSON: {error}"))?;
    if !HTTP_SUCCESS.contains(&status) {
        let message = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        return Err(format!(
            "the gateway refused the grant with HTTP {status}: {message}"
        ));
    }
    let text = |name: &str| body.get(name).and_then(Value::as_str).map(str::to_owned);
    Ok(ManualSignIn {
        provider: text("provider").unwrap_or_default(),
        subscription_id: text("subscription_id").unwrap_or_default(),
        account: text("account"),
        result: match text("result").as_deref() {
            Some("signed_in") => "signed_in",
            Some("unchanged") => "unchanged",
            _ => "failed",
        },
        detail: text("detail").unwrap_or_default(),
    })
}
