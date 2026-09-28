//! The fleet's versioning rule (github.com/lbartoszcze/AutoVersion, SPEC.md at
//! v0.1.0): given the published version, the published surface and the
//! candidate surface, what kind of change this is and what the next version
//! is. The SPEC keeps one small implementation per consumer, held identical by
//! its shared fixtures; `brama version-gate conformance` runs those fixtures
//! against this port, and the version-check workflow runs it before it trusts
//! a verdict from it.

use std::collections::BTreeSet;
use std::fmt;

use serde_json::Value;

/// The value a version slot resets to when a higher slot advances, and the
/// value of an unstable major (SPEC.md, "What moves").
const SLOT_RESET: u64 = 0;
/// How far one change advances one slot (SPEC.md, "What moves").
const SLOT_STEP: u64 = 1;

/// A refusal, named the way the rule's fixtures name it.
#[derive(Debug)]
pub(super) struct Refusal {
    pub(super) name: &'static str,
    pub(super) message: String,
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "refused ({}): {}", self.name, self.message)
    }
}

fn refuse(name: &'static str, message: String) -> Refusal {
    Refusal { name, message }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    Breaking,
    Additive,
    Internal,
}

impl Change {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Breaking => "breaking",
            Self::Additive => "additive",
            Self::Internal => "internal",
        }
    }
}

struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl fmt::Display for Version {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A segment that survives a URL path and a filesystem key unchanged.
fn canonical(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

impl Version {
    fn parse(value: &str) -> Result<Self, Refusal> {
        if !canonical(value) {
            return Err(refuse(
                "not-canonical",
                format!("{value:?} is not a canonical coordinate: expected a non-empty segment of alphanumerics, '.', '_' and '-', with no surrounding whitespace"),
            ));
        }
        let slots = value.split('.').collect::<Vec<_>>();
        let [major, minor, patch] = slots.as_slice() else {
            return Err(refuse(
                "not-a-triple",
                format!("{value:?} is not a major.minor.patch triple, so there is no slot to advance; name the next version explicitly"),
            ));
        };
        let numeric = |slot: &str| {
            (!slot.is_empty() && slot.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| slot.parse::<u64>().ok())
                .flatten()
        };
        match (numeric(major), numeric(minor), numeric(patch)) {
            (Some(major), Some(minor), Some(patch)) => Ok(Self { major, minor, patch }),
            _ => Err(refuse(
                "not-numeric",
                format!("{value:?} has a non-numeric slot, so advancing it would invent an ordering; name the next version explicitly"),
            )),
        }
    }

    /// The version this change produces. While the major slot is zero, the
    /// minor slot carries compatibility.
    fn advance(&self, change: Change) -> Self {
        let (major, minor, patch) = match (change, self.major == SLOT_RESET) {
            (Change::Breaking, true) => (self.major, self.minor + SLOT_STEP, SLOT_RESET),
            (Change::Breaking, false) => (self.major + SLOT_STEP, SLOT_RESET, SLOT_RESET),
            (Change::Additive, false) => (self.major, self.minor + SLOT_STEP, SLOT_RESET),
            _ => (self.major, self.minor, self.patch + SLOT_STEP),
        };
        Self {
            major,
            minor,
            patch,
        }
    }
}

pub(super) struct Decision {
    pub(super) current: String,
    pub(super) change: Change,
    pub(super) next: String,
    pub(super) removed: Vec<String>,
    pub(super) added: Vec<String>,
}

fn surface(names: &[String], side: &str) -> Result<BTreeSet<String>, Refusal> {
    let collected = names.iter().cloned().collect::<BTreeSet<_>>();
    if collected.is_empty() {
        return Err(refuse(
            "empty-surface",
            format!("the {side} surface is empty, which is far more likely to be a broken extractor than a product that promises nothing"),
        ));
    }
    Ok(collected)
}

/// The whole answer. A declared break may only escalate the class.
pub(super) fn decide(
    current: &str,
    published: &[String],
    candidate: &[String],
    declared_breaking: bool,
) -> Result<Decision, Refusal> {
    let version = Version::parse(current)?;
    let before = surface(published, "published")?;
    let after = surface(candidate, "candidate")?;
    let removed = before.difference(&after).cloned().collect::<Vec<_>>();
    let added = after.difference(&before).cloned().collect::<Vec<_>>();
    let change = if declared_breaking || !removed.is_empty() {
        Change::Breaking
    } else if !added.is_empty() {
        Change::Additive
    } else {
        Change::Internal
    };
    Ok(Decision {
        current: version.to_string(),
        change,
        next: version.advance(change).to_string(),
        removed,
        added,
    })
}

fn names(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The JSON between the first pair of ``` fences of FIXTURES.md, or the whole
/// text when it is already JSON.
fn cases(text: &str) -> Result<Value, String> {
    if let Ok(value) = serde_json::from_str(text) {
        return Ok(value);
    }
    let block = text
        .split("```")
        .nth(1)
        .ok_or("the fixtures file has no fenced block")?;
    let body = block.split_once('\n').map_or(block, |(_, rest)| rest);
    serde_json::from_str(body).map_err(|error| format!("the fenced fixtures are not JSON: {error}"))
}

/// Every `classify` case must yield exactly the recorded class, next version,
/// removed and added names; every `refuse` case exactly the recorded refusal.
/// Prints one line per case; `Ok(false)` when any differs.
pub(super) fn conformance(text: &str) -> Result<bool, String> {
    let fixtures = cases(text)?;
    let (mut failures, mut total) = (0usize, 0usize);
    for group in ["classify", "refuse"] {
        let list = fixtures[group]
            .as_array()
            .ok_or(format!("the fixtures declare no `{group}` list"))?;
        for case in list {
            total += 1;
            let observed = match decide(
                case["current"].as_str().unwrap_or(""),
                &names(&case["published"]),
                &names(&case["candidate"]),
                case["declared_breaking"].as_bool().unwrap_or(false),
            ) {
                Ok(answer) => serde_json::json!({
                    "class": answer.change.name(),
                    "next": answer.next,
                    "removed": answer.removed,
                    "added": answer.added,
                }),
                Err(refusal) => serde_json::json!({ "refusal": refusal.name }),
            };
            let name = case["name"].as_str().unwrap_or("unnamed case");
            if observed == case["expect"] {
                println!("OK   {name}");
            } else {
                failures += 1;
                println!(
                    "FAIL {name}: expected {}, observed {observed}",
                    case["expect"]
                );
            }
        }
    }
    println!("{} of {total} fixture case(s) reproduced", total - failures);
    Ok(failures == 0)
}
