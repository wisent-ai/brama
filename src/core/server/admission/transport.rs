//! Was this hop protected before it reached this process? Loopback, the host's
//! own bound address, a declared mesh peer and a trusted HTTPS proxy are the
//! four ways the answer is yes; everything else is asked to upgrade.

use std::net::{IpAddr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::core::server::refusal::api_error;

fn forwarded_proto_is_https(headers: &HeaderMap) -> Option<bool> {
    let forwarded = headers.get("forwarded").map(|value| {
        value.to_str().is_ok_and(|value| {
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
        value.to_str().is_ok_and(|value| {
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

/// The addresses `variable` lists, comma-separated: the address this
/// process bound (`BRAMA_BIND_ADDRESS`), the TLS terminators whose forwarded
/// headers are trusted (`BRAMA_TRUSTED_PROXY_IPS`), and the mesh peers whose
/// hop is already encrypted (`BRAMA_ENCRYPTED_PEER_IPS`). Unset or blank
/// lists none. An entry that is not an IP address, or a value that is not
/// text, is a configuration fault named with the variable, never a peer
/// quietly dropped from the list.
fn declared_addresses(variable: &str) -> Result<Vec<IpAddr>, String> {
    let configured = match std::env::var(variable) {
        Ok(configured) => configured,
        Err(std::env::VarError::NotPresent) => return Ok(Vec::new()),
        Err(error) => return Err(format!("{variable} cannot be read: {error}")),
    };
    configured
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry.parse::<IpAddr>().map_err(|error| {
                format!("{variable} lists {entry:?}, which is not an IP address: {error}")
            })
        })
        .collect()
}

/// The three lists the guard reads, or the first one that cannot be read.
fn declared_peers() -> Result<(Vec<IpAddr>, Vec<IpAddr>, Vec<IpAddr>), String> {
    Ok((
        declared_addresses("BRAMA_BIND_ADDRESS")?,
        declared_addresses("BRAMA_TRUSTED_PROXY_IPS")?,
        declared_addresses("BRAMA_ENCRYPTED_PEER_IPS")?,
    ))
}

/// Loopback, the address the gateway itself bound, a declared mesh peer
/// whose hop is already encrypted, and a trusted TLS terminator whose
/// forwarded headers say `https` pass; anything else is asked to upgrade.
///
/// A request from the bound address came from this machine and never
/// crossed a network, so it has the protection a loopback request has;
/// without it a gateway on a routable address cannot be exercised from its
/// own host. A mesh peer's hop is encrypted node to node, which cannot be
/// seen from inside the request; the list is written by whoever renders
/// this unit, from the hosts the registry declares.
pub(in crate::core::server) async fn require_secure_transport(
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let forwarded = forwarded_proto_is_https(request.headers());
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| address.ip());
    if peer.is_some_and(|address| address.is_loopback()) {
        return next.run(request).await;
    }
    let (bound, proxies, encrypted) = match declared_peers() {
        Ok(lists) => lists,
        Err(fault) => {
            return api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!(
                    "the transport guard's configuration cannot be read, so only loopback is served: {fault}"
                ),
            )
            .into_response();
        }
    };
    let named = |list: &[IpAddr]| peer.is_some_and(|address| list.contains(&address));
    if named(&bound) || named(&encrypted) || (named(&proxies) && forwarded == Some(true)) {
        return next.run(request).await;
    }
    api_error(
        StatusCode::UPGRADE_REQUIRED,
        "HTTPS is required except for direct loopback requests",
    )
    .into_response()
}
