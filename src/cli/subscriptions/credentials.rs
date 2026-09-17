//! `brama subscription`: acting on one provider's subscription credentials --
//! renewing them now, signing the account in through Weles or by hand, or
//! taking the grant a harness on this machine already holds.

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
    /// List the grants the harnesses on this machine hold - omp, Claude Code, Codex CLI, Kimi Code - without the grants themselves
    #[command(name = "held")]
    Held {
        /// Only grants for this provider: claude-code, codex or kimi
        #[arg(long)]
        provider: Option<String>,
        /// Read the harness stores below this directory instead of the home directory
        #[arg(long)]
        home: Option<String>,
        /// Print the list as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Take a grant a harness on this machine already holds and make it this subscription's credential
    #[command(name = "import")]
    Import {
        /// The provider whose grant should be taken: claude-code, codex or kimi
        provider: String,
        /// Exact Brama subscription whose grant this import replaces
        #[arg(long)]
        subscription_id: String,
        /// Why this import is being run; recorded in the journal beside the verdict
        #[arg(long)]
        reason: String,
        /// The harness to take it from: omp, claude, codex or kimi; without it, the only grant held here for the provider
        #[arg(long)]
        from: Option<String>,
        /// Which account's grant to take, when more than one is held
        #[arg(long)]
        account: Option<String>,
        /// Read the harness stores below this directory instead of the home directory
        #[arg(long)]
        home: Option<String>,
        /// Hand the grant to this gateway instead of storing it here; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Every grant a harness on this machine holds joins the pool: one stable member per account, ids already present are left to Brama's own refresh
    #[command(name = "sync")]
    Sync {
        /// Why this sync is being run; recorded in the journal beside each verdict
        #[arg(long)]
        reason: String,
        /// Read the harness stores below this directory instead of the home directory
        #[arg(long)]
        home: Option<String>,
        /// Hand the grants to this gateway instead of storing them here; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Print the sweep as JSON instead of lines
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
        } => super::manual::finish(
            super::manual::sign_in(&provider, &subscription_id, &reason, code).await,
            json,
        ),
        SubscriptionCommand::Held {
            provider,
            home,
            json,
        } => super::harness::held(provider.as_deref(), home.as_deref(), json),
        SubscriptionCommand::Import {
            provider,
            subscription_id,
            reason,
            from,
            account,
            home,
            gateway,
            json,
        } => super::manual::finish(
            super::manual::import(
                &provider,
                &subscription_id,
                &reason,
                from.as_deref(),
                account.as_deref(),
                home.as_deref(),
                gateway.as_deref(),
            )
            .await,
            json,
        ),
        SubscriptionCommand::Sync {
            reason,
            home,
            gateway,
            json,
        } => super::sync::finish(
            super::sync::sync(&reason, home.as_deref(), gateway.as_deref()).await,
            json,
        ),
    }
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
