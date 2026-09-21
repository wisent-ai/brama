//! Who is in the pool: recording which account a member belongs to, putting
//! a retired member back, and giving one away.
//!
//! These three act on membership rather than on a credential, which is why
//! they are not beside the sign-ins: none of them obtains, rotates or reads a
//! grant. They travel together because they answer one question -- which
//! accounts does this deployment use -- and because a retirement that
//! nothing could take back leaves declared accounts unusable while the pool
//! reports itself empty.

use std::future::Future;

use serde_json::Value;

use super::remote::Destination;
use super::text;

/// Record which account each member of one provider belongs to, read from
/// that member's own grant, and exit non-zero while any member is left
/// unattributed: the pool's account count is short by exactly those.
pub(super) async fn attribute(provider: &str, json: bool) {
    match brama::subscription_dispatch::pool::record_accounts(provider).await {
        Ok(verdict) => {
            if json {
                crate::cli::print_json(&verdict);
            } else {
                print_attribution(&verdict);
            }
            if verdict
                .get("unattributed")
                .and_then(Value::as_array)
                .is_some_and(|left| !left.is_empty())
            {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

/// Put one retired member back in the rotation.
///
/// Named a gateway, this asks that gateway; named none, it acts on this
/// host's own pool through the same code the gateway's route runs, because a
/// deployment whose pool is on this machine has no remote to ask.
pub(super) async fn reinstate(destination: Destination, subscription_id: &str, reason: &str) {
    if destination.gateway.is_none() && destination.gateway_consumer.is_none() {
        match brama::subscription_dispatch::pool::reinstate_member(subscription_id, reason).await {
            Ok(verdict) => println!(
                "{}: {}",
                text(&verdict, "subscription_id").unwrap_or_default(),
                text(&verdict, "detail").unwrap_or_default()
            ),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    act(destination, |gateway, bearer| {
        let subscription_id = subscription_id.to_owned();
        let reason = reason.to_owned();
        async move { super::remote::reinstate(&gateway, &bearer, &subscription_id, &reason).await }
    })
    .await;
}

/// Give one member back on the gateway that holds it.
pub(super) async fn disown(destination: Destination, subscription_id: &str, reason: &str) {
    act(destination, |gateway, bearer| {
        let subscription_id = subscription_id.to_owned();
        let reason = reason.to_owned();
        async move { super::remote::disown(&gateway, &bearer, &subscription_id, &reason).await }
    })
    .await;
}

/// Resolve the gateway and its bearer once, then run one membership call
/// against it. Both calls refuse identically when no gateway is named,
/// because the member and its journal live on the gateway, not in a shell.
async fn act<Call, Running>(destination: Destination, call: Call)
where
    Call: FnOnce(String, String) -> Running,
    Running: Future<Output = Result<String, String>>,
{
    match destination.resolve_reading_stdin().await {
        Ok((Some(gateway), bearer)) => match call(gateway, bearer.trim().to_owned()).await {
            Ok(said) => println!("{said}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        },
        Ok((None, _)) => {
            eprintln!("name the gateway holding the member: --gateway or --gateway-consumer");
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

/// What the attribution recorded, and what it could not.
///
/// The members it could not attribute are printed with the reason, because
/// the pool's account count is short by exactly them, and an operator
/// comparing that count with the accounts they hold needs to see which
/// member is missing rather than a number that disagrees.
fn print_attribution(verdict: &Value) {
    let rows = |field: &str| -> Vec<&Value> {
        verdict
            .get(field)
            .and_then(Value::as_array)
            .map(|rows| rows.iter().collect())
            .unwrap_or_default()
    };
    let recorded = rows("recorded");
    let unattributed = rows("unattributed");
    println!(
        "{} of {} member(s) name an account",
        recorded.len(),
        recorded.len().saturating_add(unattributed.len())
    );
    for row in recorded {
        println!(
            "  {} -> {}",
            text(row, "member").unwrap_or_default(),
            text(row, "account").unwrap_or_default()
        );
    }
    for row in unattributed {
        println!(
            "  {}: {}",
            text(row, "member").unwrap_or_default(),
            text(row, "reason").unwrap_or_default()
        );
    }
}
