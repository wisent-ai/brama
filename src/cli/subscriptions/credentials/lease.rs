//! Carrying out `brama subscription lease`: taking, releasing and listing the
//! leases that bind one session to one subscription of the pool.

use super::super::{leases, remote};
use super::LeaseCommand;

/// Carry out one lease command against the gateway it names.
pub(super) async fn run(command: LeaseCommand) {
    match command {
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
    }
}
