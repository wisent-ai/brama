//! Whether a grant a harness on THIS machine holds may be handed to a
//! gateway somewhere else.
//!
//! It may not, and this module is why. A provider issues one OAuth pair per
//! sign-in, and it rotates that pair when the grant is used from a second
//! place: the harness that holds it loses its session. It happened twice on
//! this fleet. On 2026-09-17 two accounts `omp` was signed into died within
//! the hour of the gateway's first refresh of their copies. On 2026-09-20 the
//! sweep ran from `lukasz-macbook` into the gateway on `charless-mac-mini`,
//! and the operator's own Claude session on the laptop answered `OAuth access
//! token has been revoked` — in the middle of the work that had installed the
//! sweep.
//!
//! So borrowing stays local: a harness may hand its grant to the gateway
//! running beside it, where one machine holds both and there is no second
//! session to lose. A gateway on another host is repaired by signing in
//! there, which is what `subscription sign-in` and `stado credentials
//! seed-enrol` exist for.

/// The loopback hosts a gateway beside this process answers on.
const LOCAL_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

/// The refusal an operator reads instead of losing a session.
pub(crate) const CROSS_HOST_REFUSAL: &str =
    "a grant a harness here holds cannot be handed to a gateway on another machine: the provider \
     rotates the pair and this machine loses the session it is signed into (2026-09-17, and again \
     2026-09-20 under the operator's own Claude session). Repair that gateway where it runs: \
     `brama subscription sign-in` on its host, or `stado credentials seed-enrol` to give its \
     account an authenticator first";

/// Whether this gateway origin is the one running beside the harnesses.
///
/// Only the host matters: a loopback origin cannot reach another machine, and
/// an ssh forward that publishes a remote gateway on loopback is exactly the
/// arrangement this refuses — so a forwarded origin is named by the caller
/// through `--gateway-consumer`, which resolves to the host that answers.
pub(crate) fn is_local_gateway(origin: &str) -> bool {
    let after_scheme = origin.split("://").nth(1).unwrap_or(origin);
    let authority = after_scheme.split('/').next().unwrap_or_default();
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|character| character.is_ascii_digit()) => host,
        _ => authority,
    };
    LOCAL_HOSTS.contains(&host)
}

/// The refusal for a gateway that is not this machine's own, or nothing.
pub(crate) fn refuse_cross_host(origin: &str, allowed: bool) -> Option<String> {
    if allowed || is_local_gateway(origin) {
        return None;
    }
    Some(format!("{CROSS_HOST_REFUSAL} (gateway: {origin})"))
}

#[cfg(test)]
mod tests {
    use super::{is_local_gateway, refuse_cross_host};

    /// Fixture origins, not configuration: each is an input whose HOST the
    /// function under test judges, and the remote one is the exact gateway
    /// whose use revoked the operator's Claude session on 2026-09-20.
    const LOCAL_ORIGINS: [&str; 3] = [
        "http://127.0.0.1:18080",
        "http://localhost:8080/",
        "http://[::1]:18080",
    ];
    const REMOTE_ORIGIN: &str = "http://charless-mac-mini.local:18080";

    #[test]
    fn a_loopback_gateway_is_this_machines_own() {
        for origin in LOCAL_ORIGINS {
            assert!(is_local_gateway(origin), "{origin}");
        }
    }

    #[test]
    fn a_gateway_on_another_machine_is_refused_with_its_reason() {
        let refusal = refuse_cross_host(REMOTE_ORIGIN, false).expect("a remote gateway is refused");
        assert!(refusal.contains("loses the session"), "{refusal}");
        assert!(refusal.contains("charless-mac-mini"), "{refusal}");
        assert!(refusal.contains("seed-enrol"), "{refusal}");
    }

    /// An operator who says it anyway is not argued with; the refusal is a
    /// default, not a lock.
    #[test]
    fn an_explicit_allowance_passes() {
        assert_eq!(refuse_cross_host(REMOTE_ORIGIN, true), None);
    }
}
