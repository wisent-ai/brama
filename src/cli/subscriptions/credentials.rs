//! `brama subscription`: acting on one provider's subscription credentials --
//! renewing them now, or signing the account in and proving it by a renewal.

use clap::Subcommand;
use serde_json::Value;

use super::text;

#[derive(Subcommand)]
pub(crate) enum SubscriptionCommand {
    /// Refresh this provider's subscription credentials now
    Refresh {
        /// The provider whose grants should be refreshed (`codex`, `claude-code`, `kimi`)
        provider: String,
        /// Why this refresh is being run; recorded in the journal beside the verdict
        #[arg(long)]
        reason: String,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Sign one provider account in through Weles, then prove it by a refresh
    #[command(name = "sign-in")]
    SignIn {
        /// The provider whose account should be signed in (`codex`, `claude-code`, `kimi`)
        provider: String,
        /// The exact Weles sign-in row to drive; without it the single row Weles holds for the provider is used, and two or more are never guessed between
        #[arg(long)]
        login_item: Option<String>,
        /// Exact Brama subscription whose grant must be replaced and refreshed
        #[arg(long)]
        subscription_id: Option<String>,
        /// Why this sign-in is being run; recorded in the journal beside the verdict
        #[arg(long)]
        reason: String,
        /// How long Weles may spend driving the browser, in milliseconds
        #[arg(long, default_value_t = 900_000)]
        login_timeout_ms: u64,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Sign one Claude account in by hand: open the printed URL in your own browser, log in, and paste the code it shows
    #[command(name = "sign-in-manual")]
    SignInManual {
        /// The provider whose account should be signed in; `claude-code` is the one with a manual flow
        provider: String,
        /// Exact Brama subscription whose grant this sign-in replaces
        #[arg(long)]
        subscription_id: String,
        /// Why this sign-in is being run; recorded in the journal beside the verdict
        #[arg(long)]
        reason: String,
        /// The `code#state` or redirect URL, when it is already at hand; without it the command asks on the terminal
        #[arg(long)]
        code: Option<String>,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Take a Claude grant the operator's harness already holds and make it this subscription's credential
    #[command(name = "import")]
    Import {
        /// The provider whose grant should be taken; `claude-code` is the one the harness holds
        provider: String,
        /// Exact Brama subscription whose grant this import replaces
        #[arg(long)]
        subscription_id: String,
        /// Why this import is being run; recorded in the journal beside the verdict
        #[arg(long)]
        reason: String,
        /// Read the grant from omp's credential store on this machine
        #[arg(long, default_value_t = false)]
        from_omp: bool,
        /// Which account's grant to take, when the harness holds more than one
        #[arg(long)]
        account: Option<String>,
        /// The harness store to read instead of ~/.omp/agent/agent.db
        #[arg(long)]
        store: Option<String>,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

pub(crate) async fn run(command: SubscriptionCommand) {
    match command {
        SubscriptionCommand::Refresh {
            provider,
            reason,
            json,
        } => match brama::subscription_dispatch::pool::refresh_provider(&provider, &reason).await {
            Ok(verdict) => {
                if json {
                    crate::cli::print_json(&verdict);
                } else {
                    print_refresh(&verdict);
                }
                // Partial renewal is a failure, even if some grants now work.
                if text(&verdict, "result") != Some("refreshed") {
                    std::process::exit(1);
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        },
        SubscriptionCommand::SignIn {
            provider,
            login_item,
            subscription_id,
            reason,
            login_timeout_ms,
            json,
        } => match brama::subscription_dispatch::sign_in::sign_in_provider(
            brama::subscription_dispatch::sign_in::SignInOptions {
                provider,
                login_item,
                subscription_id,
                reason,
                login_timeout_ms,
            },
        )
        .await
        {
            Ok(verdict) => {
                if json {
                    crate::cli::print_json(&verdict);
                } else {
                    print_sign_in(&verdict);
                }
                // A sign-in that did not end in a refreshed credential
                // exits non-zero after reporting, because the caller is
                // repairing a refused subscription and needs to know from
                // the status whether it is still refused.
                if text(&verdict, "result") != Some("signed_in") {
                    std::process::exit(1);
                }
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        },
        SubscriptionCommand::SignInManual {
            provider,
            subscription_id,
            reason,
            code,
            json,
        } => finish_manual(
            sign_in_manual(&provider, &subscription_id, &reason, code).await,
            json,
        ),
        SubscriptionCommand::Import {
            provider,
            subscription_id,
            reason,
            from_omp,
            account,
            store,
            json,
        } => {
            if !from_omp {
                eprintln!("--from-omp names the only store this command reads; nothing else holds a grant Brama can take");
                std::process::exit(1);
            }
            finish_manual(
                import_from_omp(
                    &provider,
                    &subscription_id,
                    &reason,
                    account.as_deref(),
                    store.as_deref(),
                )
                .await,
                json,
            )
        }
    }
}

/// Print one manual verdict the way every credential command does, and exit
/// unsuccessfully unless the account is signed in.
fn finish_manual(
    verdict: Result<brama::subscription_dispatch::sign_in::manual::ManualSignIn, String>,
    json: bool,
) {
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
            if verdict.result != "signed_in" {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

/// The grant the harness holds for the named account, made this
/// subscription's credential and proved with one completion.
async fn import_from_omp(
    provider: &str,
    subscription_id: &str,
    reason: &str,
    account: Option<&str>,
    store: Option<&str>,
) -> Result<brama::subscription_dispatch::sign_in::manual::ManualSignIn, String> {
    use brama::subscription_dispatch::sign_in::manual::{adopt, omp, Origin};
    if reason.trim().is_empty() {
        return Err("--reason must say why this sign-in is being run".into());
    }
    if subscription_id.trim().is_empty() {
        return Err("an exact subscription id is required".into());
    }
    let store = store.map(str::to_owned).unwrap_or_else(omp::default_store);
    let held = omp::account(&store, provider, account)?;
    adopt(
        provider,
        subscription_id.trim(),
        held.grant,
        Origin::Harness,
        reason,
    )
    .await
}

/// The manual sign-in on a terminal: the page to open, the paste, and the
/// shared exchange-store-prove that Brama Desktop's route also ends in.
async fn sign_in_manual(
    provider: &str,
    subscription_id: &str,
    reason: &str,
    code: Option<String>,
) -> Result<brama::subscription_dispatch::sign_in::manual::ManualSignIn, String> {
    use brama::subscription_dispatch::sign_in::manual;
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

/// What one refresh came to, as lines.
fn print_refresh(verdict: &Value) {
    println!(
        "provider: {}",
        text(verdict, "provider").unwrap_or_default()
    );
    println!(
        "attempted: {}",
        verdict
            .get("attempted")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    );
    println!("result: {}", text(verdict, "result").unwrap_or_default());
    println!("detail: {}", text(verdict, "detail").unwrap_or_default());
}

/// What one sign-in came to, as lines.
fn print_sign_in(verdict: &Value) {
    println!(
        "provider: {}",
        text(verdict, "provider").unwrap_or_default()
    );
    println!(
        "login_item: {}",
        text(verdict, "login_item").unwrap_or_default()
    );
    if let Some(account) = text(verdict, "account").filter(|account| !account.is_empty()) {
        println!("account: {account}");
    }
    println!("result: {}", text(verdict, "result").unwrap_or_default());
    println!("detail: {}", text(verdict, "detail").unwrap_or_default());
}
