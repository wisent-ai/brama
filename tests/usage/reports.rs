//! The provider's own usage report, isolated.
//!
//! Plan usage is a real provider GET carrying the account's own credential,
//! and the accounts in this story are not real ones. A vendor's usage endpoint
//! is not a test fixture: asking it would measure somebody's live plan, and
//! asking it with a credential this test made up would teach us only what a
//! rejected request looks like. `BRAMA_PROVIDER_<ID>_BASE_URL` is the
//! deployment's own override, already honoured by the reader under test, so
//! Brama's own code path runs unchanged against an endpoint owned here.
//!
//! Two answers, because two outcomes have to be told apart: a report the
//! gateway can read, and a report the provider will not give it. `claude-code`
//! answers its OAuth usage document; `kimi` refuses with a status and a
//! sentence, which is what a provider refusal actually looks like.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::json;

/// Anthropic states utilization as a percentage per window and names the
/// instant each window resets; the ledger stores the fraction.
pub const FIVE_HOUR_PERCENT: f64 = 42.5;
pub const FIVE_HOUR_FRACTION: f64 = 0.425;
pub const SEVEN_DAY_PERCENT: f64 = 12.0;
pub const FIVE_HOUR_RESET: &str = "2027-01-01T00:00:00Z";
/// What the refusing provider answers. The status is in the recorded envelope,
/// so a story can assert the provider was actually reached and actually said
/// no, rather than that something somewhere failed.
pub const REFUSED_STATUS: u16 = 503;
pub const REFUSED_DETAIL: &str = "usage is unavailable for this account right now";
/// Where each provider publishes its report. Absolute paths, because a usage
/// route is not always a sibling of the chat route.
const ANTHROPIC_USAGE_PATH: &str = "/api/oauth/usage";
const KIMI_USAGE_PATH: &str = "/coding/v1/usages";

/// The isolated report endpoint, and what was presented to it.
pub struct ProviderReports {
    port: u16,
    presented: Arc<Mutex<Vec<String>>>,
}

impl ProviderReports {
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Every credential this endpoint was shown, in order. A plan reading that
    /// arrived without the account's own grant would be a reading of nothing.
    pub fn presented(&self) -> Vec<String> {
        self.presented
            .lock()
            .expect("the presented credentials are readable")
            .clone()
    }
}

/// Serve both providers' usage reports until the story ends.
pub fn spawn_provider_reports() -> ProviderReports {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .expect("bind the isolated provider usage endpoint");
    let port = listener
        .local_addr()
        .expect("provider usage address")
        .port();
    let presented = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&presented);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => answer_usage_request(stream, &seen),
                Err(_) => return,
            }
        }
    });
    ProviderReports { port, presented }
}

/// Anthropic's own usage document: one object per window, `null` for a window
/// this plan does not have, and the optional per-model windows omitted.
fn anthropic_usage_report() -> String {
    json!({
        "five_hour": {"utilization": FIVE_HOUR_PERCENT, "resets_at": FIVE_HOUR_RESET},
        "seven_day": {"utilization": SEVEN_DAY_PERCENT, "resets_at": null},
    })
    .to_string()
}

fn answer_usage_request(mut stream: TcpStream, presented: &Arc<Mutex<Vec<String>>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone the provider stream"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut header = String::new();
    loop {
        header.clear();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) => {
                let line = header.trim_end().to_owned();
                if line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("authorization") {
                        record(presented, value);
                    }
                }
            }
            Err(_) => break,
        }
    }
    let (status, reason, body) = if request_line.contains(ANTHROPIC_USAGE_PATH) {
        (200, "OK", anthropic_usage_report())
    } else if request_line.contains(KIMI_USAGE_PATH) {
        (
            REFUSED_STATUS,
            "Service Unavailable",
            json!({"error": {"message": REFUSED_DETAIL}}).to_string(),
        )
    } else {
        (
            404,
            "Not Found",
            json!({"error": {"message": "this provider publishes no such report"}}).to_string(),
        )
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn record(presented: &Arc<Mutex<Vec<String>>>, value: &str) {
    if let Ok(mut seen) = presented.lock() {
        seen.push(value.trim().to_owned());
    }
}
