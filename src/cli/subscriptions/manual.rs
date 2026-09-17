//! The credential commands that end in one manual verdict: the by-hand
//! sign-in on a terminal, and the grant taken from a harness.

use brama::subscription_dispatch::sign_in::manual::{self, ManualSignIn};

/// Print one manual verdict the way every credential command does, and exit
/// unsuccessfully unless the account is signed in.
pub(crate) fn finish(verdict: Result<ManualSignIn, String>, json: bool) {
    match verdict {
        Ok(verdict) => {
            if json {
                crate::cli::print_json(
                    &serde_json::to_value(&verdict).expect("verdict serializes"),
                );
            } else {
                println!("provider: {}", verdict.provider);
                println!("subscription: {}", verdict.subscription_id);
                if let Some(account) = &verdict.account {
                    println!("account: {account}");
                }
                println!("result: {}", verdict.result);
                println!("detail: {}", verdict.detail);
            }
            if verdict.result != "signed_in" && verdict.result != "unchanged" {
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
/// shared exchange-store-prove that Brama Desktop's route also ends in.
pub(crate) async fn sign_in(
    provider: &str,
    subscription_id: &str,
    reason: &str,
    code: Option<String>,
) -> Result<ManualSignIn, String> {
    if reason.trim().is_empty() {
        return Err("--reason must say why this sign-in is being run".into());
    }
    let request = manual::begin(provider, subscription_id)?;
    let pasted = match code {
        Some(code) => code,
        None => {
            eprintln!("Open this page in your own browser and log in:");
            eprintln!();
            eprintln!("  {}", request.url);
            eprintln!();
            eprintln!("When it shows a code, paste it here (the `code#state` text, or the whole redirect URL):");
            let mut line = String::new();
            std::io::stdin()
                .read_line(&mut line)
                .map_err(|error| format!("reading the pasted code: {error}"))?;
            line
        }
    };
    manual::complete(request, &pasted, reason).await
}

/// The grant a harness holds, chosen, then taken here or through a gateway.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn import(
    provider: &str,
    subscription_id: &str,
    reason: &str,
    from: Option<&str>,
    account: Option<&str>,
    home: Option<&str>,
    gateway: Option<&str>,
) -> Result<ManualSignIn, String> {
    if reason.trim().is_empty() {
        return Err("--reason must say why this sign-in is being run".into());
    }
    if subscription_id.trim().is_empty() {
        return Err("an exact subscription id is required".into());
    }
    let grant = super::harness::choose(provider, from, account, home)?;
    match gateway {
        Some(gateway) => {
            super::harness::import_through(gateway, provider, subscription_id, reason, grant).await
        }
        None => super::harness::import_here(provider, subscription_id, reason, grant).await,
    }
}
