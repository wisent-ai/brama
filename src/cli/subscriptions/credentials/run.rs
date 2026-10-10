//! Carrying out one credential command: refreshing here or on the gateway that
//! holds the credentials, signing an account in, reporting second factors, and
//! putting a retired member back in the rotation or giving one away.

use serde_json::Value;

use super::super::text;
use super::super::verdicts::enrolment::enrol_authenticator;
use super::super::verdicts::{print_refresh, print_sign_in};
use super::super::{acquisition, leases, manual, membership, remote};
use super::{AuthenticatorCommand, LeaseCommand, SignInBy, SubscriptionCommand};

pub(crate) async fn run(command: SubscriptionCommand) {
    match command {
        SubscriptionCommand::List(args) => super::super::report(args).await,
        SubscriptionCommand::Refresh {
            provider,
            reason,
            gateway,
            gateway_consumer,
            bearer_role,
            json,
        } => {
            // A block that empties a pool lives in the gateway's journal, so a
            // refresh run in a shell cannot clear it. Naming a gateway sends
            // the refresh where the credentials and the journal are.
            let destination = remote::Destination {
                gateway,
                gateway_consumer,
                bearer_role,
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
            by: SignInBy::Hand,
            login_item,
            subscription_id,
            reason,
            gateway,
            gateway_consumer,
            bearer_role,
            json,
        } => {
            if let Some(login_item) = login_item {
                refuse_usage(format!(
                    "--login-item {login_item} names a Weles sign-in row, and --by hand signs in through your own browser; drop it or sign in --by weles"
                ));
            }
            let Some(subscription_id) = subscription_id else {
                refuse_usage(
                    "--by hand replaces one subscription's grant; name it with --subscription-id"
                        .to_string(),
                );
            };
            manual::finish(
                manual::sign_in(
                    &provider,
                    &subscription_id,
                    &reason,
                    remote::Destination {
                        gateway,
                        gateway_consumer,
                        bearer_role,
                    },
                )
                .await,
                json,
            )
        }
        SubscriptionCommand::SignIn {
            provider,
            by: SignInBy::Weles,
            login_item,
            subscription_id,
            reason,
            gateway,
            gateway_consumer,
            bearer_role,
            json,
        } => {
            if gateway.is_some() || gateway_consumer.is_some() || bearer_role.is_some() {
                refuse_usage(
                    "--gateway, --gateway-consumer and --bearer-role carry a sign-in --by hand to the serving gateway; --by weles runs on the host that reaches Weles, so drop them or sign in --by hand"
                        .to_string(),
                );
            }
            sign_in_through_weles(provider, login_item, subscription_id, reason, json).await
        }
        SubscriptionCommand::Authenticator {
            command:
                AuthenticatorCommand::Enrol {
                    provider,
                    subscription_id,
                    reason,
                    login_item,
                    json,
                },
        } => {
            enrol_authenticator(
                &provider,
                &subscription_id,
                &reason,
                login_item.as_deref(),
                json,
            )
            .await
        }
        SubscriptionCommand::Authenticator {
            command: AuthenticatorCommand::List { provider, json },
        } => {
            membership::second_factor(provider.as_deref(), json).await;
        }
        SubscriptionCommand::Attribute {
            provider,
            gateway,
            gateway_consumer,
            bearer_role,
            json,
        } => {
            membership::attribute(
                remote::Destination {
                    gateway,
                    gateway_consumer,
                    bearer_role,
                },
                &provider,
                json,
            )
            .await;
        }
        SubscriptionCommand::Reinstate {
            subscription_id,
            reason,
            gateway,
            gateway_consumer,
            bearer_role,
            json,
        } => {
            membership::reinstate(
                remote::Destination {
                    gateway,
                    gateway_consumer,
                    bearer_role,
                },
                &subscription_id,
                &reason,
                json,
            )
            .await;
        }
        SubscriptionCommand::Disown {
            subscription_id,
            reason,
            gateway,
            gateway_consumer,
            bearer_role,
            json,
        } => {
            membership::disown(
                remote::Destination {
                    gateway,
                    gateway_consumer,
                    bearer_role,
                },
                &subscription_id,
                &reason,
                json,
            )
            .await;
        }
        SubscriptionCommand::Acquire {
            provider,
            reason,
            gateway,
            gateway_consumer,
            bearer_role,
            json,
        } => {
            let destination = remote::Destination {
                gateway,
                gateway_consumer,
                bearer_role,
            };
            acquisition::acquire(destination, &provider, &reason, json).await;
        }
        SubscriptionCommand::HandOver {
            provider,
            harness,
            json,
        } => acquisition::hand_over(&provider, harness, json).await,
        SubscriptionCommand::Lease(command) => match command {
            LeaseCommand::Take {
                provider,
                session_id,
                holder,
                gateway,
                gateway_consumer,
                bearer_role,
                json,
            } => {
                leases::take(
                    remote::Destination {
                        gateway,
                        gateway_consumer,
                        bearer_role,
                    },
                    &provider,
                    &session_id,
                    &holder,
                    json,
                )
                .await
            }
            LeaseCommand::Release {
                lease_id,
                session_id,
                gateway,
                gateway_consumer,
                bearer_role,
                json,
            } => {
                leases::release(
                    remote::Destination {
                        gateway,
                        gateway_consumer,
                        bearer_role,
                    },
                    lease_id.as_deref(),
                    session_id.as_deref(),
                    json,
                )
                .await
            }
            LeaseCommand::List {
                gateway,
                gateway_consumer,
                bearer_role,
                json,
            } => {
                leases::list(
                    remote::Destination {
                        gateway,
                        gateway_consumer,
                        bearer_role,
                    },
                    json,
                )
                .await
            }
        },
    }
}

/// Run one provider's refresh on the gateway that holds its credentials.
async fn refresh_on_gateway(
    destination: remote::Destination,
    provider: &str,
    reason: &str,
) -> Result<Value, String> {
    let (origin, bearer) = destination.resolve_reading_stdin().await?;
    let origin = origin.ok_or_else(|| {
        String::from("name --gateway or --gateway-consumer to refresh another gateway's pool")
    })?;
    remote::refresh(&origin, bearer.trim(), provider, reason).await
}

/// Refuse flags that contradict a sign-in's method the way clap refuses any
/// usage error: the sentence, the usage, and clap's usage exit status.
fn refuse_usage(sentence: String) -> ! {
    use clap::CommandFactory;
    crate::Cli::command()
        .error(clap::error::ErrorKind::ArgumentConflict, sentence)
        .exit()
}

/// Drive the exact Weles sign-in row and prove the account with a refresh.
/// Every step Weles reports, and above all a wait for the operator, is
/// printed as it happens on stderr, because a browser sign-in has no clock
/// that ends it and stdout carries the verdict. A sign-in that did not end in
/// a refreshed credential exits non-zero after reporting, because the caller
/// is repairing a refused subscription and needs to know from the status
/// whether it is still refused.
async fn sign_in_through_weles(
    provider: String,
    login_item: Option<String>,
    subscription_id: Option<String>,
    reason: String,
    json: bool,
) {
    let verdict = brama::subscription_dispatch::sign_in::sign_in_provider(
        brama::subscription_dispatch::sign_in::SignInOptions {
            provider,
            login_item,
            subscription_id,
            reason,
            progress: Some(std::sync::Arc::new(|event: &serde_json::Value| {
                if let Some(sentence) =
                    brama::subscription_dispatch::sign_in::progress_sentence(event)
                {
                    eprintln!("{sentence}");
                }
            })),
        },
    )
    .await;
    match verdict {
        Ok(verdict) => {
            if json {
                crate::cli::print_json(&verdict);
            } else {
                print_sign_in(&verdict);
            }
            if text(&verdict, "result") != Some("signed_in") {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
