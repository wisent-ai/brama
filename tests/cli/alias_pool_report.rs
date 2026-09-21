//! What `brama aliases` says about an alias this gateway cannot check on its
//! own.
//!
//! `best` is a selector the subscription pool resolves per caller, and the
//! report printed it as `serving` with no further word. On 2026-09-21 that
//! read as a healthy gateway for an hour while every request for `best` was
//! refused with `subscription_reauthorization_required`, because the pool held
//! no live member — the diagnosis went to the route table, the fault was in
//! the pool. The pool's own count now stands beside the selector, and these
//! cases hold it there.

#[path = "../support/mod.rs"]
mod support;

#[path = "../support/cli.rs"]
mod cli;

use cli::command;
use serde_json::Value;
use support::TestDirectory;

/// The selector alias every deployment carries.
const SELECTOR: &str = "best";

fn said(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn the_selector_is_never_printed_as_serving_without_the_pools_own_count() {
    let directory = TestDirectory::new("alias-pool-lines");
    let output = command(&directory)
        .arg("aliases")
        .output()
        .expect("run brama aliases");
    let text = said(&output);
    assert!(
        text.lines()
            .any(|line| line.starts_with(SELECTOR) && line.contains("serving")),
        "{text}"
    );
    assert!(
        text.contains("subscription pool:"),
        "the selector was printed with no pool count: {text}"
    );
    // This fixture has no router binary, so nothing in the pool can be live.
    assert!(
        text.contains(
            "every request for a selector alias is refused with \
             subscription_reauthorization_required"
        ),
        "an empty pool did not state the refusal its selector gets: {text}"
    );
    assert!(
        text.contains("brama subscription sign-in"),
        "the refusal named no way out: {text}"
    );
}

#[test]
fn the_report_carries_the_count_as_data_and_marks_the_selector() {
    let directory = TestDirectory::new("alias-pool-json");
    let output = command(&directory)
        .args(["aliases", "--json"])
        .output()
        .expect("run brama aliases --json");
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("the report is not JSON: {error}\n{}", said(&output)));
    let pool = &report["subscription_pool"];
    assert!(pool["members"].is_u64(), "{report:#}");
    assert_eq!(pool["live"], 0, "{report:#}");
    assert!(
        pool["selectors"].as_u64().unwrap_or_default() > 0,
        "{report:#}"
    );
    let selector = report["aliases"]
        .as_array()
        .expect("aliases are an array")
        .iter()
        .find(|alias| alias["alias"] == SELECTOR)
        .expect("the selector alias is reported");
    assert_eq!(selector["subscription_resolved"], true, "{selector:#}");
    let routed = report["aliases"]
        .as_array()
        .expect("aliases are an array")
        .iter()
        .find(|alias| alias["alias"] != SELECTOR && alias["route"].is_string());
    if let Some(routed) = routed {
        assert_eq!(
            routed["subscription_resolved"], false,
            "a routed alias was marked as pool-resolved: {routed:#}"
        );
    }
}

/// `--strict` exists so a deployment check fails on a gateway that cannot
/// serve. A dry pool is exactly that, however healthy the route table looks.
#[test]
fn strict_fails_while_the_pool_holds_no_live_member() {
    let directory = TestDirectory::new("alias-pool-strict");
    let output = command(&directory)
        .args(["aliases", "--strict"])
        .output()
        .expect("run brama aliases --strict");
    assert_eq!(output.status.code(), Some(1), "{}", said(&output));
}
