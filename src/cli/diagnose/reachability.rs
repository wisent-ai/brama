//! Where the gateway answers, and what this boot has said about itself. A
//! gateway can be perfectly configured and still be reached by nobody, and it
//! can refuse to start for a reason written down only once, in the stream
//! nothing reads.

use std::net::Ipv4Addr;
use std::process::Command;

use serde_json::Value;

use super::Layout;

/// What the gateway logs when it binds, followed by the address.
const ANNOUNCEMENT: &str = "Starting brama server on ";
/// The first line `start-with-skarbiec` writes on every boot attempt, before
/// any stage runs: the current attempt is everything from its last occurrence.
const BOOT_MARKER: &str = "brama launcher: boot attempt of ";

/// The HTTPS origins `tailscale serve status --json` publishes: each key of
/// its `Web` map is a `host:port` the tailnet serves, with the handlers it
/// forwards to. Printed as they stand; an answer without a `Web` map
/// publishes nothing over HTTPS.
fn served_origins(status: &Value) -> Vec<String> {
    let Some(web) = status.get("Web").and_then(Value::as_object) else {
        say!("  tailscale serve publishes no HTTPS origin: {status}");
        return Vec::new();
    };
    web.iter()
        .map(|(host_port, served)| {
            say!("  serve: https://{host_port} -> {}", served["Handlers"]);
            format!("https://{host_port}/health")
        })
        .collect()
}

/// The tailnet origins this host's tailscale serves, or nothing, said why.
fn tailnet_targets() -> Vec<String> {
    let mut serve_status = Command::new("tailscale");
    serve_status.args(["serve", "status", "--json"]);
    match stado_wait::output(&mut serve_status) {
        // A status that failed publishes nothing; its error says why.
        Ok(served) if !served.status.success() => {
            say!(
                "  tailscale serve status exited {}: {}; the tailnet origin is not probed",
                served.status,
                String::from_utf8_lossy(&served.stderr).trim()
            );
            Vec::new()
        }
        Ok(served) => match serde_json::from_slice::<Value>(&served.stdout) {
            Ok(status) => served_origins(&status),
            Err(error) => {
                say!(
                    "  tailscale serve status --json answered something that is not JSON ({error}); the tailnet origin is not probed"
                );
                Vec::new()
            }
        },
        Err(error) => {
            say!(
                "  tailscale serve status could not run ({error}); the tailnet origin is not probed"
            );
            Vec::new()
        }
    }
}

pub(super) async fn print_reachability(layout: &Layout) {
    say!("\n=== reachability");
    let mut targets = Vec::new();
    match layout.settings.get("PORT") {
        Some(port) => targets.push(format!("http://{}:{port}/health", Ipv4Addr::LOCALHOST)),
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
    targets.extend(tailnet_targets());
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
        match stado_wait::request(client.get(&target)).await {
            Ok(answer) => say!("  {target} -> {}", answer.status()),
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
