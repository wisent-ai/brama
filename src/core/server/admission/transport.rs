//! Was this hop protected before it reached this process? Loopback, the host's
//! own bound address, a declared mesh peer and a trusted HTTPS proxy are the
//! four ways the answer is yes; everything else is asked to upgrade.

use std::net::SocketAddr;

use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::core::server::refusal::api_error;

fn forwarded_proto_is_https(headers: &HeaderMap) -> Option<bool> {
    let forwarded = headers.get("forwarded").map(|value| {
        value.to_str().ok().is_some_and(|value| {
            value
                .split(',')
                .next()
                .into_iter()
                .flat_map(|entry| entry.split(';'))
                .filter_map(|part| part.trim().split_once('='))
                .find(|(name, _)| name.eq_ignore_ascii_case("proto"))
                .is_some_and(|(_, proto)| {
                    proto.trim().trim_matches('"').eq_ignore_ascii_case("https")
                })
        })
    });
    let x_forwarded = headers.get("x-forwarded-proto").map(|value| {
        value.to_str().ok().is_some_and(|value| {
            value
                .split(',')
                .next()
                .is_some_and(|proto| proto.trim().eq_ignore_ascii_case("https"))
        })
    });
    match (forwarded, x_forwarded) {
        (Some(left), Some(right)) => Some(left && right),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

/// Peers whose hop is already encrypted before it reaches this process.
///
/// The fleet's mesh authenticates and encrypts node to node, so a request
/// arriving from one of those nodes has had exactly the protection this guard
/// exists to demand -- it simply cannot be seen from inside the request. That
/// is a narrow statement about named peers, not a general exemption for plain
/// HTTP: the list is written out by whoever renders this unit, from the hosts
/// the registry declares, and an address that is not on it is refused like any
/// other.
fn encrypted_transport_peer(peer: std::net::IpAddr) -> bool {
    std::env::var("BRAMA_ENCRYPTED_PEER_IPS")
        .ok()
        .is_some_and(|configured| {
            configured
                .split(',')
                .filter_map(|value| value.trim().parse::<std::net::IpAddr>().ok())
                .any(|trusted| trusted == peer)
        })
}

fn trusted_forwarded_peer(peer: std::net::IpAddr) -> bool {
    std::env::var("BRAMA_TRUSTED_PROXY_IPS")
        .ok()
        .is_some_and(|configured| {
            configured
                .split(',')
                .filter_map(|value| value.trim().parse::<std::net::IpAddr>().ok())
                .any(|trusted| trusted == peer)
        })
}

/// The address this process itself bound.
///
/// A request whose source is the address the gateway listens on came from this
/// machine and never crossed a network, so it has the protection this guard
/// demands for the same reason a loopback request does. Without this, a
/// gateway placed on a routable address cannot be exercised from the host it
/// runs on at all: `/health` answers, every other path returns 426, and an
/// operator diagnosing it concludes the gateway is broken. That is not a
/// hypothetical - it is where this comment came from.
fn own_bound_address(peer: std::net::IpAddr) -> bool {
    std::env::var("BRAMA_BIND_ADDRESS")
        .ok()
        .and_then(|configured| configured.trim().parse::<std::net::IpAddr>().ok())
        .is_some_and(|bound| bound == peer)
}

pub(in crate::core::server) async fn require_secure_transport(
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let forwarded = forwarded_proto_is_https(request.headers());
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect_info| connect_info.0.ip());
    let loopback = peer.is_some_and(|address| address.is_loopback() || own_bound_address(address));
    let trusted_https_proxy = peer.is_some_and(trusted_forwarded_peer) && forwarded == Some(true);
    let already_encrypted = peer.is_some_and(encrypted_transport_peer);
    if loopback || trusted_https_proxy || already_encrypted {
        return next.run(request).await;
    }
    api_error(
        StatusCode::UPGRADE_REQUIRED,
        "HTTPS is required except for direct loopback requests",
    )
    .into_response()
}
