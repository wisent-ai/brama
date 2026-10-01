//! Where the gateway answers, and what this boot has said about itself. A
//! gateway can be perfectly configured and still be reached by nobody, and it
//! can refuse to start for a reason written down only once, in the stream
//! nothing reads.

use std::process::Command;

use serde_json::Value;

use super::Layout;

/// The loopback port the gateway listens on when service.env names none.
const DEFAULT_PORT: &str = "8080";
/// The port the tailnet serve proxy publishes the gateway on.
const TAILNET_PORT: &str = "8443";
/// What the gateway logs when it binds, followed by the address.
const ANNOUNCEMENT: &str = "Starting brama server on ";
/// What the launcher logs at the start of every boot attempt.
const BOOT_MARKER: &str = "Starting server";
/// How much of the unit log's end is shown: the launcher's own account of
/// provisioning and registration, which a slice starting at the last
/// announcement hides when a start never got that far.
const LOG_TAIL: usize = 60;

fn tail(lines: &[&str], count: usize) -> Vec<String> {
    lines[lines.len().saturating_sub(count)..]
        .iter()
        .map(|line| format!("  {line}"))
        .collect()
}

pub(super) async fn print_reachability(layout: &Layout) {
    say!("\n=== reachability");
    let port = layout
        .settings
        .get("PORT")
        .cloned()
        .unwrap_or_else(|| DEFAULT_PORT.to_string());
    let mut targets = vec![format!("http://127.0.0.1:{port}/health")];
    let log = std::fs::read(&layout.log)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    if let Some(announced) = log
        .lines()
        .filter_map(|line| {
            line.split_once(ANNOUNCEMENT)
                .map(|(_, rest)| rest.trim().to_string())
        })
        .last()
    {
        targets.push(format!("http://{announced}/health"));
    }
    let tailscale = [
        "/usr/local/bin/tailscale",
        "/opt/homebrew/bin/tailscale",
        "/Applications/Tailscale.app/Contents/MacOS/tailscale",
    ]
    .into_iter()
    .find(|path| std::path::Path::new(path).exists());
    if let Some(tailscale) = tailscale {
        if let Ok(status) = Command::new(tailscale).args(["status", "--json"]).output() {
            let document: Value = serde_json::from_slice(&status.stdout).unwrap_or(Value::Null);
            if let Some(name) = document
                .pointer("/Self/DNSName")
                .and_then(Value::as_str)
                .map(|name| name.trim_end_matches('.'))
                .filter(|name| !name.is_empty())
            {
                targets.push(format!("https://{name}:{TAILNET_PORT}/health"));
            }
        }
        if let Ok(served) = Command::new(tailscale).args(["serve", "status"]).output() {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&served.stdout),
                String::from_utf8_lossy(&served.stderr)
            );
            for line in text
                .lines()
                .map(str::trim)
                .filter(|line| line.contains("proxy") || line.contains("https://"))
            {
                say!("  serve: {line}");
            }
        }
    }
    // The tailnet certificate is private to the fleet; reachability, not
    // trust, is the question here.
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build();
    let Ok(client) = client else { return };
    let mut seen = Vec::new();
    for target in targets {
        if seen.contains(&target) {
            continue;
        }
        match client.get(&target).send().await {
            Ok(answer) => say!("  {target} -> {}", answer.status().as_u16()),
            Err(error) => say!("  {target} -> {error}"),
        }
        seen.push(target);
    }
}

pub(super) fn print_boot_attempt(layout: &Layout) {
    say!("\n=== current boot attempt");
    match std::fs::read(&layout.log) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            let latest = text
                .rfind(BOOT_MARKER)
                .map(|at| &text[at..])
                .unwrap_or(&text);
            say!("{}", latest.trim());
            say!("\n=== last lines of the unit's log");
            for line in tail(&text.lines().collect::<Vec<_>>(), LOG_TAIL) {
                say!("{line}");
            }
        }
        Err(_) => say!("  {}: absent", layout.log.display()),
    }
}
