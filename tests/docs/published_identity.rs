//! Nothing this repository publishes may describe a particular deployment.
//!
//! This repository is public, and prose written while repairing one
//! installation carries that installation with it: account identifiers,
//! machine names, how many accounts somebody holds, what a provider
//! measured for them on a given evening. Every such sentence is somebody's
//! private operational record, published for good, and scrubbing one after
//! the fact protects nothing -- the next change writes another.
//!
//! So the rule is enforced on what a change introduces, and it is enforced
//! by a judge: the text a change adds to a published path is answered by
//! Brama's own decision alias, which is the product's way of deciding
//! something no list of forbidden words can decide. A pattern catches an
//! address; it cannot catch the same disclosure written as a story.
//!
//! The judge is asked one question per added line, about that line alone,
//! and anything it classifies as deployment-specific fails this test with
//! the line quoted. A judge that cannot be reached fails it too: a gate that
//! skips itself when the product is unavailable is not a gate.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

/// Where published text lives: the site, the code, the tests and the
/// repository's own documents.
const PUBLISHED: [&str; 5] = ["vercel-ingress/docs", "src", "tests", "README.md", "docs"];

/// The alias this judgement is answered through. It is the product's own
/// decision capability, so the gate cannot drift from what Brama serves.
const JUDGE_ALIAS: &str = "decision-model";

/// A line longer than this is judged by its beginning; a disclosure is at the
/// start of a sentence, and an entire minified page is not a sentence.
const LINE_CHARACTERS: usize = 400;

/// A line shorter than this carries no statement about anything.
const SHORTEST_JUDGED_LINE: usize = 24;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// The revision this change is measured against: the published history, or
/// the revision a caller names in `BRAMA_PUBLISHED_BASELINE` when a whole
/// stretch of history has to be judged at once -- what a release qualifies,
/// or a cleanup pass over text that was published before this gate existed.
fn baseline() -> String {
    if let Ok(named) = std::env::var("BRAMA_PUBLISHED_BASELINE") {
        if !named.trim().is_empty() {
            return named.trim().to_owned();
        }
    }
    for reference in ["origin/main", "main"] {
        let found = Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", reference])
            .current_dir(repository())
            .output();
        if let Ok(found) = found {
            if found.status.success() {
                return String::from_utf8_lossy(&found.stdout).trim().to_owned();
            }
        }
    }
    panic!("this repository has no published history to measure a change against");
}

/// Every line this working tree and its commits add to a published path,
/// against the published history. Only what a change introduces is judged:
/// text that is already published is a cleanup, not a new disclosure, and
/// judging it again on every unrelated change would spend a provider request
/// per line of the repository.
fn added_lines() -> Vec<String> {
    let mut arguments = vec![
        "diff".to_owned(),
        "--unified=0".to_owned(),
        "--no-color".to_owned(),
        baseline(),
        "--".to_owned(),
    ];
    arguments.extend(PUBLISHED.iter().map(|path| (*path).to_owned()));
    let diff = Command::new("git")
        .args(&arguments)
        .current_dir(repository())
        .output()
        .expect("git diff runs in this checkout");
    assert!(
        diff.status.success(),
        "git diff refused: {}",
        String::from_utf8_lossy(&diff.stderr)
    );
    String::from_utf8_lossy(&diff.stdout)
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .map(|line| line[1..].trim().to_owned())
        .filter(|line| line.chars().count() >= SHORTEST_JUDGED_LINE)
        .map(|line| line.chars().take(LINE_CHARACTERS).collect())
        .collect()
}

/// The one question every line is judged by.
///
/// A choice rather than a yes/no, and with worked examples of both labels:
/// asked abstractly, a judge calls every line of a source file specific to
/// wherever it was written, and the gate then refuses everything. The
/// examples are of the product's own prose, so what they teach is the
/// distinction rather than a vocabulary.
fn question() -> Value {
    json!({
        "line": {
            "type": "choice",
            "instructions": "One line from a public source repository is in the state. \
                Decide what the line itself says. Examples of product: 'the command refuses \
                an empty id', 'a member belongs to an account through the address recorded \
                against it', 'writes the tag brama:account:<address>'. Examples of \
                deployment: 'on that host three paid accounts sat in the vault', 'the five \
                accounts this deployment retired', 'measured on a machine named in the \
                line', 'the operator was signed out that day'.",
            "criteria": {
                "product": "the line states a rule, a behaviour, a refusal, a field, a \
                    command or a placeholder",
                "deployment": "the line states a fact about one installation: a machine, a \
                    person, an address, an identifier built from an address, a count of \
                    somebody's accounts, or something measured or that happened there",
            },
        }
    })
}

/// Where the gateway that judges is, and the bearer to present to it.
///
/// Both are the deployment's own: the origin is resolved through Stado's
/// service directory as a named consumer, and the bearer is read from the
/// vault item that consumer is entitled to. Neither is written into this
/// repository, which is the same rule this gate enforces.
fn gateway() -> (String, String) {
    let consumer = std::env::var("BRAMA_JUDGE_CONSUMER").unwrap_or_else(|_| "operator".into());
    let resolved = Command::new("stado")
        .args(["service", "directory", "connect", "brama", "--consumer"])
        .arg(&consumer)
        .output()
        .expect("stado resolves where this machine reaches Brama");
    let origin = String::from_utf8_lossy(&resolved.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned();
    assert!(
        origin.starts_with("http"),
        "no Brama is reachable for consumer `{consumer}`, so this change is unjudged: {}{}",
        String::from_utf8_lossy(&resolved.stdout),
        String::from_utf8_lossy(&resolved.stderr)
    );
    let item = std::env::var("BRAMA_JUDGE_BEARER_ITEM")
        .expect("BRAMA_JUDGE_BEARER_ITEM names the vault item#field holding this gateway's bearer");
    let (item, field) = item
        .split_once('#')
        .expect("BRAMA_JUDGE_BEARER_ITEM is `<item>#<field>`");
    let read = Command::new("skarbiec")
        .args(["get", item, "--field", field])
        .output()
        .expect("the vault answers this machine's own read");
    assert!(
        read.status.success(),
        "the judge's bearer could not be read, so this change is unjudged: {}",
        String::from_utf8_lossy(&read.stderr)
    );
    (
        origin,
        String::from_utf8_lossy(&read.stdout).trim().to_owned(),
    )
}

/// Ask the product about one line. The alias, the route behind it and the
/// provider are the deployment's own, so this gate judges with whatever
/// Brama is configured to decide with.
///
/// One line per request on purpose: asked about twenty lines at once, a
/// judge answers about the whole state and every line inherits the verdict
/// of the worst one.
fn judged_as_deployment(origin: &str, bearer: &str, line: &str) -> bool {
    let answered = reqwest::blocking::Client::new()
        .post(format!("{}/v1/decisions", origin.trim_end_matches('/')))
        .bearer_auth(bearer)
        .json(&json!({
            "model": JUDGE_ALIAS,
            "state": line,
            "questions": question(),
        }))
        .send()
        .expect("the gateway answers the decision request");
    let status = answered.status();
    let answer: Value = answered.json().unwrap_or(Value::Null);
    assert!(
        status.is_success(),
        "the published-text judge refused through `{JUDGE_ALIAS}`, so this change is \
         unjudged: HTTP {status}: {answer}"
    );
    answer
        .pointer("/answers/line/choice")
        .and_then(Value::as_str)
        == Some("deployment")
}

/// Every line this change adds to a published path is about the product, not
/// about the installation it was written on.
#[test]
fn nothing_this_change_publishes_describes_one_deployment() {
    let lines = added_lines();
    if lines.is_empty() {
        return;
    }
    let (origin, bearer) = gateway();
    let found: Vec<&String> = lines
        .iter()
        .filter(|line| judged_as_deployment(&origin, &bearer, line))
        .collect();
    assert!(
        found.is_empty(),
        "these lines describe one deployment rather than the product, and this repository is \
         public; write them about what the product does, with placeholders where an identifier \
         is needed:\n{}",
        found
            .iter()
            .map(|line| line.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}
