//! `brama subscription`: acting on one provider's subscription credentials --
//! renewing them now, signing the account in through Weles or by hand,
//! enrolling the authenticator that makes every later sign-in unattended, or
//! giving a pool member back.

use clap::Subcommand;
use serde_json::Value;

use super::text;
use super::verdicts::enrolment::enrol_authenticator;
use super::verdicts::{print_refresh, print_sign_in};

#[derive(Subcommand)]
pub(crate) enum SubscriptionCommand {
    /// Refresh this provider's subscription credentials now, here or on the
    /// gateway that holds them
    Refresh {
        /// The provider whose grants should be refreshed (`codex`, `claude-code`, `kimi`)
        provider: String,
        /// Why this refresh is being run; recorded in the journal beside the verdict
        #[arg(long)]
        reason: String,
        /// The gateway to refresh; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// Read the console's bearer from the vault as `<item>#<field>` instead of from stdin
        #[arg(long)]
        bearer_item: Option<String>,
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
    /// Enrol an authenticator for the login behind one subscription, so every later sign-in answers the provider's second factor by itself
    #[command(name = "enrol-authenticator")]
    EnrolAuthenticator {
        /// The provider whose account needs a seed: claude-code, codex or kimi
        provider: String,
        /// Exact Brama subscription whose login gains the authenticator
        #[arg(long)]
        subscription_id: String,
        /// Why this enrolment is being run; recorded beside the verdict
        #[arg(long)]
        reason: String,
        /// Exact Skarbiec login item, when the subscription names more than one
        #[arg(long)]
        login_item: Option<String>,
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
    /// Record which account each pool member of one provider belongs to, read from the member's own grant
    Attribute {
        /// The provider whose members should be attributed (`codex`, `claude-code`, `kimi`)
        provider: String,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Put one retired pool member back in the rotation, because this deployment uses that account after all
    #[command(name = "reinstate")]
    Reinstate {
        /// The retired pool member to use again
        #[arg(long)]
        subscription_id: String,
        /// Why it is used again; recorded beside the gateway's own ledger entry
        #[arg(long)]
        reason: String,
        /// The gateway holding it; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// Read the console's bearer from the vault as `<item>#<field>` instead of from stdin
        #[arg(long)]
        bearer_item: Option<String>,
    },
    /// Give one pool member back: the gateway retires it and forgets its credential, and any machine that signed that account in keeps its own session
    #[command(name = "disown")]
    Disown {
        /// The pool member to give back
        #[arg(long)]
        subscription_id: String,
        /// Why it is given back; recorded beside the gateway's own ledger entry
        #[arg(long)]
        reason: String,
        /// The gateway holding it; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// Read the console's bearer from the vault as `<item>#<field>` instead of from stdin
        #[arg(long)]
        bearer_item: Option<String>,
    },
}

pub(crate) async fn run(command: SubscriptionCommand) {
    match command {
        SubscriptionCommand::Refresh {
            provider,
            reason,
            gateway,
            gateway_consumer,
            bearer_item,
            json,
        } => {
            // A block that empties a pool lives in the gateway's journal, so a
            // refresh run in a shell cannot clear it. Naming a gateway sends
            // the refresh where the credentials and the journal are.
            let destination = super::remote::Destination {
                gateway,
                gateway_consumer,
                bearer_item,
            };
            let verdict = if destination.gateway.is_some() || destination.gateway_consumer.is_some()
            {
                refresh_on_gateway(destination, &provider, &reason).await
            } else {
                brama::subscription_dispatch::pool::refresh_provider(&provider, &reason).await
            };
            match verdict {
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
            }
        }
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
        SubscriptionCommand::EnrolAuthenticator {
            provider,
            subscription_id,
            reason,
            login_item,
            login_timeout_ms,
            json,
        } => {
            enrol_authenticator(
                &provider,
                &subscription_id,
                &reason,
                login_item.as_deref(),
                login_timeout_ms,
                json,
            )
            .await
        }
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
        SubscriptionCommand::Attribute { provider, json } => {
            super::membership::attribute(&provider, json).await;
        }
        SubscriptionCommand::Reinstate {
            subscription_id,
            reason,
            gateway,
            gateway_consumer,
            bearer_item,
        } => {
            super::membership::reinstate(
                super::remote::Destination {
                    gateway,
                    gateway_consumer,
                    bearer_item,
                },
                &subscription_id,
                &reason,
            )
            .await;
        }
        SubscriptionCommand::Disown {
            subscription_id,
            reason,
            gateway,
            gateway_consumer,
            bearer_item,
        } => {
            super::membership::disown(
                super::remote::Destination {
                    gateway,
                    gateway_consumer,
                    bearer_item,
                },
                &subscription_id,
                &reason,
            )
            .await;
        }
    }
}

/// Run one provider's refresh on the gateway that holds its credentials.
async fn refresh_on_gateway(
    destination: super::remote::Destination,
    provider: &str,
    reason: &str,
) -> Result<Value, String> {
    let (origin, bearer) = destination.resolve_reading_stdin().await?;
    let origin = origin.ok_or_else(|| {
        String::from("name --gateway or --gateway-consumer to refresh another gateway's pool")
    })?;
    super::remote::refresh(&origin, bearer.trim(), provider, reason).await
}
