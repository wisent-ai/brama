//! Carrying out one credential command: refreshing here or on the gateway that
//! holds the credentials, signing an account in, reporting second factors, and
//! putting a retired member back in the rotation or giving one away.

use serde_json::Value;

use super::super::text;
use super::super::verdicts::enrolment::enrol_authenticator;
use super::super::verdicts::{print_refresh, print_sign_in};
use super::SubscriptionCommand;

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
        SubscriptionCommand::SecondFactor { provider, json } => {
            super::membership::second_factor(provider.as_deref(), json).await;
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
