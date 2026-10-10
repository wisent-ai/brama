//! Account facts exposed by a harness's public usage command, never its grants.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AccountObservation {
    pub provider: String,
    pub account: String,
    pub plan: Option<String>,
    pub source: String,
    pub observed_at_ms: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct DiscoveryReport {
    pub accounts: Vec<AccountObservation>,
    pub errors: Vec<String>,
}

/// Read the supported public interface. Authentication remains inside the harness.
pub async fn omp() -> DiscoveryReport {
    let mut report = match command(&["usage", "accounts", "--json"]).await {
        Ok(document) => parse_accounts(&document),
        Err(error) => refused(error),
    };
    match command(&["usage", "--json"]).await {
        Ok(document) => {
            let usage = parse_usage(
                &document,
                "omp usage --json",
                chrono::Utc::now().timestamp_millis(),
            );
            report.accounts.extend(usage.accounts);
            report.errors.extend(usage.errors);
        }
        Err(error) => report.errors.push(error),
    }
    report
}

async fn command(arguments: &[&str]) -> Result<Value, String> {
    let operation = format!("omp {}", arguments.join(" "));
    let mut command = tokio::process::Command::new("omp");
    command.args(arguments).kill_on_drop(true);
    let output = stado_wait::output_async(&mut command)
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

fn parse_accounts(document: &Value) -> DiscoveryReport {
    let source = "omp usage accounts --json";
    let Some(accounts) = document.get("accounts").and_then(Value::as_array) else {
        return refused(format!("{source}: missing accounts array"));
    };
    let mut report = DiscoveryReport::default();
    for row in accounts {
        let parsed = (|| {
            let name = row["provider"]
                .as_str()
                .ok_or_else(|| format!("{source}: account row has no provider"))?;
            let provider = super::provider_for_harness("omp", name)?;
            let identity = row["identityKey"]
                .as_str()
                .ok_or_else(|| format!("{source}: {name} has no identityKey"))?;
            let mut emails = identity
                .split('|')
                .filter_map(|part| part.strip_prefix("email:"));
            let account = emails
                .next()
                .ok_or_else(|| format!("{source}: {name} has no email identity"))?;
            if emails.next().is_some() {
                return Err(format!("{source}: {name} has multiple email identities"));
            }
            super::provider_account(provider, account)?;
            Ok(AccountObservation {
                provider: provider.to_owned(),
                account: account.to_lowercase(),
                plan: None,
                source: source.to_owned(),
                observed_at_ms: chrono::Utc::now().timestamp_millis(),
            })
        })();
        match parsed {
            Ok(account) => report.accounts.push(account),
            Err(error) => report.errors.push(error),
        }
    }
    report
}

fn refused(error: String) -> DiscoveryReport {
    DiscoveryReport {
        accounts: Vec::new(),
        errors: vec![error],
    }
}

/// Keep a missing identity visible rather than guessing an address from a label.
pub fn parse_usage(document: &Value, source: &str, observed_at_ms: i64) -> DiscoveryReport {
    let Some(reports) = document.get("reports").and_then(Value::as_array) else {
        return refused(format!("{source}: missing reports array"));
    };
    let mut result = DiscoveryReport::default();
    for report in reports {
        let Some(harness_provider) = report.get("provider").and_then(Value::as_str) else {
            result
                .errors
                .push(format!("{source}: usage row has no provider"));
            continue;
        };
        let provider = match super::provider_for_harness("omp", harness_provider) {
            Ok(provider) => provider,
            Err(error) => {
                result.errors.push(format!("{source}: {error}"));
                continue;
            }
        };
        let Some(account) = report
            .pointer("/metadata/email")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            result.errors.push(format!(
                "{source}: {harness_provider} reports no account address"
            ));
            continue;
        };
        let plan = report
            .pointer("/metadata/planType")
            .and_then(Value::as_str)
            .map(str::to_owned);
        result.accounts.push(AccountObservation {
            provider: provider.to_owned(),
            account: account.to_lowercase(),
            plan,
            source: source.to_owned(),
            observed_at_ms,
        });
    }
    result
}
