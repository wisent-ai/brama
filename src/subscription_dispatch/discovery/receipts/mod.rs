//! Read purchase receipts through Skrzynka's public mailbox/message interface.

mod cache;
mod infer;
use super::harness::DiscoveryReport;
use serde_json::Value;

async fn command(arguments: &[&str]) -> Result<Value, String> {
    let operation = format!("skrzynka {}", arguments.join(" "));
    let mut child = tokio::process::Command::new("skrzynka");
    child.args(arguments).kill_on_drop(true);
    let output = stado_wait::output_async(&mut child)
        .await
        .map_err(|error| format!("{operation}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{operation} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("{operation}: invalid JSON: {error}"))
}

pub async fn collect() -> DiscoveryReport {
    let mut report = DiscoveryReport::default();
    if let Err(error) = read(&mut report).await {
        report.errors.push(error);
    }
    report
}

async fn read(report: &mut DiscoveryReport) -> Result<(), String> {
    let mailboxes = command(&["mailbox", "list"]).await?;
    let mailboxes = mailboxes
        .as_array()
        .ok_or_else(|| "skrzynka mailbox list did not return a mailbox array".to_owned())?;
    let mut cache = cache::Cache::open()?;
    for mailbox in mailboxes {
        if mailbox["enabled"].as_bool() != Some(true) {
            continue;
        }
        let id = mailbox["id"]
            .as_str()
            .ok_or_else(|| "Skrzynka mailbox has no id".to_owned())?;
        if let Some(error) = mailbox["last_error_message"].as_str() {
            report.errors.push(format!(
                "Skrzynka mailbox {id} cannot supply purchase receipts: {}: {error}",
                mailbox["last_error_code"]
            ));
            continue;
        }
        if mailbox["last_sync_at"].is_null() {
            report.errors.push(format!("Skrzynka mailbox {id} has never synchronized; receipt discovery cannot claim the inbox was read"));
            continue;
        }
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let offset = seen.len().to_string();
            let page = command(&["message", "list", "--mailbox", id, "--offset", &offset]).await?;
            let messages = page
                .as_array()
                .ok_or_else(|| format!("Skrzynka mailbox {id}: message list is not an array"))?;
            if messages.is_empty() {
                break;
            }
            for message in messages {
                let message_id = message["id"]
                    .as_str()
                    .ok_or_else(|| format!("Skrzynka mailbox {id}: message has no id"))?;
                if !seen.insert(message_id.to_owned()) {
                    return Err(format!("Skrzynka mailbox {id}: pagination repeated message {message_id}; complete receipt coverage is unconfirmed"));
                }
                let source = format!("skrzynka:{id}:{message_id}");
                if let Some(accounts) = cache.entries.get(&source) {
                    report.accounts.extend(accounts.iter().cloned());
                    continue;
                }
                let full = command(&["message", "show", message_id]).await?;
                match infer::accounts(&full, &source).await {
                    Ok(accounts) => {
                        report.accounts.extend(accounts.iter().cloned());
                        cache.store(source, accounts)?;
                    }
                    Err(infer::Failure::Message(error)) => report.errors.push(error),
                    Err(infer::Failure::Route(error)) => return Err(error),
                }
            }
        }
    }
    Ok(())
}
