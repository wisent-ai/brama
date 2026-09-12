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
        } => {
            let verdict = sign_in_manual(&provider, &subscription_id, &reason, code).await;
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
    }
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
    manual::finish(request, &pasted, reason).await
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
