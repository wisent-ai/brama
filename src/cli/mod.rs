//! The `brama` command line, one module per command family.

pub(crate) mod adoption;
pub(crate) mod aliases;
pub(crate) mod catalogue;
pub(crate) mod decisions;
pub(crate) mod diagnose;
pub(crate) mod diagnostics;
pub(crate) mod launcher;
pub(crate) mod maintain;
pub(crate) mod media;
pub(crate) mod onboarding;
pub(crate) mod probe;
pub(crate) mod review;
pub(crate) mod serving;
pub(crate) mod subscriptions;
pub(crate) mod version_gate;
pub(crate) mod workload;

use serde_json::Value;

/// One report as the desktop console consumes it. A `Value` always renders
/// as JSON, so the alternate form prints it pretty with no failure to hide.
fn print_json(report: &Value) {
    println!("{report:#}");
}

/// End the command unsuccessfully, as every Brama verb does.
pub(crate) fn fail() -> ! {
    std::process::exit(1) // https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/stdlib.h.html
}

/// One answer as `--json` prints it, or `key: value` lines for a person: a
/// string as itself, null as `-`, anything nested as compact JSON; a list
/// prints one compact item per line.
fn print_answer(answer: &Value, json: bool) {
    if json {
        return print_json(answer);
    }
    let line = |value: &Value| match value {
        Value::String(text) => text.clone(),
        Value::Null => "-".to_string(),
        other => other.to_string(),
    };
    match answer {
        Value::Object(fields) => fields
            .iter()
            .for_each(|(key, value)| println!("{key}: {}", line(value))),
        Value::Array(items) => items.iter().for_each(|item| println!("{}", line(item))),
        other => println!("{}", line(other)),
    }
}
