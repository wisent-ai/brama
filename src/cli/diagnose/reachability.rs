//! Where the gateway answers, and what this boot has said about itself. A
//! gateway can be perfectly configured and still be reached by nobody, and it
//! can refuse to start for a reason written down only once, in the stream
//! nothing reads.

use std::process::Command;

use super::Layout;

/// What the gateway logs when it binds, followed by the address.
const ANNOUNCEMENT: &str = "Starting brama server on ";
/// The first line `start-with-skarbiec` writes on every boot attempt, before
/// any stage runs: the current attempt is everything from its last occurrence.
const BOOT_MARKER: &str = "brama launcher: boot attempt of ";

pub(super) async fn print_reachability(layout: &Layout) {
    say!("\n=== reachability");
    let mut targets = Vec::new();
    match layout.settings.get("PORT") {
        Some(port) => targets.push(format!("http://127.0.0.1:{port}/health")),
        None => {
            say!("  service.env names no PORT; only the address the gateway announced is probed")
        }
    }
    // A log that cannot be read hides the address the gateway announced; that
    // is said, never read as a gateway that announced nothing.
    match std::fs::read(&layout.log) {
        Ok(bytes) => {
            if let Some(announced) = String::from_utf8_lossy(&bytes)
                .lines()
                .filter_map(|line| {
                    line.split_once(ANNOUNCEMENT)
                        .map(|(_, rest)| rest.trim().to_string())
                })
                .next_back()
            {
                targets.push(format!("http://{announced}/health"));
            }
        }
        Err(error) => say!(
            "  {}: not read ({error}); the address the gateway announced is not probed",
            layout.log.display()
        ),
    }
    let tailscale = [
        "/usr/local/bin/tailscale",
        "/opt/homebrew/bin/tailscale",
        "/Applications/Tailscale.app/Contents/MacOS/tailscale",
    ]
    .into_iter()
    .find(|path| std::path::Path::new(path).exists());
    if let Some(tailscale) = tailscale {
        let mut serve_status = Command::new(tailscale);
        serve_status.args(["serve", "status"]);
        match stado_wait::output(&mut serve_status) {
            Ok(served) => {
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
                    // Each published origin is probed as published; no port is
                    // assumed for the tailnet proxy.
                    for origin in line
                        .split_whitespace()
                        .filter(|word| word.starts_with("https://"))
                    {
                        targets.push(format!("{}/health", origin.trim_end_matches('/')));
                    }
                }
            }
            Err(error) => say!(
                "  {tailscale} serve status could not run ({error}); the tailnet origin is not probed"
            ),
        }
    }
    // The tailnet certificate is private to the fleet; reachability, not
    // trust, is the question here.
    let client = match reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            say!("  no HTTP client to probe with ({error}); no address was probed");
            return;
        }
    };
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
    match std::fs::read(&layout.log) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes);
            match text.rfind(BOOT_MARKER) {
                Some(at) => {
                    say!("\n=== current boot attempt");
                    say!("{}", text[at..].trim());
                }
                // A launcher that predates the marker leaves no boundary, so
                // the whole log is the only account that hides nothing.
                None => {
                    say!("\n=== the unit's log (no boot attempt marker in it)");
                    say!("{}", text.trim());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            say!("  {}: absent", layout.log.display())
        }
        Err(error) => say!("  {}: not read: {error}", layout.log.display()),
    }
}
