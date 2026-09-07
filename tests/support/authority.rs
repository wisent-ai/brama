//! The Wisent Identity authority the pool capability proves a session
//! against, isolated.
//!
//! Brama owns routing, not identity: it asks Wisent Identity whether a
//! session belongs to the user and organization it claims, and derives the
//! account holder's pool from that verified answer. The question and the
//! verification are real product paths; the project answering them is not a
//! test fixture, so an isolated authority answers here instead. The isolated
//! vault behind the same gateway is `tests/support`'s.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};

use serde_json::json;

/// A Wisent user session, proved against the isolated identity authority.
pub const ACCOUNT_BEARER: &str = "brama-pool-account-session";
pub const ACCOUNT_USER_ID: &str = "6f2b4c1e-3d5a-4f77-9b8e-1c2d3e4f5a6b";
pub const ORGANIZATION_ID: &str = "0a1b2c3d-4e5f-4a6b-8c9d-0e1f2a3b4c5d";

/// The account holder's pool is keyed on the verified session's user id, never
/// on anything the caller sent.
pub fn account_agent_id() -> String {
    format!("user-{}", ACCOUNT_USER_ID.replace('-', ""))
}

/// The isolated Wisent Identity authority: it answers the two requests the
/// gateway makes about a human session and refuses every other bearer, so the
/// account holder's identity is proved by the product's own code against an
/// authority that is not the production project.
pub fn spawn_identity_authority() -> u16 {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .expect("bind the isolated identity authority");
    let port = listener.local_addr().expect("authority address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            answer_identity_request(stream);
        }
    });
    port
}

fn answer_identity_request(mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone authority stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut authorized = false;
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let lowered = header.to_ascii_lowercase();
        if lowered.starts_with("authorization:") {
            authorized = header.ends_with(ACCOUNT_BEARER);
        }
        if let Some(value) = lowered.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or_default();
        }
    }
    if length > 0 {
        let mut body = vec![0u8; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
    }
    let body = if !authorized {
        None
    } else if request_line.contains("/auth/v1/user") {
        Some(json!({"id": ACCOUNT_USER_ID}).to_string())
    } else if request_line.contains("/rest/v1/rpc/authorize_organization") {
        Some(
            json!({
                "user_id": ACCOUNT_USER_ID,
                "organization_id": ORGANIZATION_ID,
                "role": "owner",
            })
            .to_string(),
        )
    } else {
        None
    };
    let response = match body {
        Some(body) => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
        None => {
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned()
        }
    };
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}
