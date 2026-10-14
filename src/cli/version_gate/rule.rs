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

/// One surface list of a fixture case. A case without it, or with an entry
/// that is not text, is malformed: read as empty it reproduced the
/// `empty-surface` refusal by accident.
fn names(case: &Value, field: &str) -> Result<Vec<String>, String> {
    case[field]
        .as_array()
        .ok_or_else(|| format!("the case states no {field} array"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("the case lists {item} in {field}, which is not text"))
        })
        .collect()
}

/// The two groups of AutoVersion's fixtures document; a document missing
/// either is refused by serde naming it.
#[derive(serde::Deserialize)]
struct Fixtures {
    classify: Vec<Value>,
    refuse: Vec<Value>,
}

/// The fixtures between the first pair of ``` fences of FIXTURES.md, or the
/// whole text when it is already JSON.
fn cases(text: &str) -> Result<Fixtures, String> {
    if let Ok(fixtures) = serde_json::from_str(text) {
        return Ok(fixtures);
    }
    let (_, after) = text
        .split_once("```")
        .ok_or("the fixtures file has no fenced block")?;
    let block = match after.split_once("```") {
        Some((block, _)) => block,
        None => after,
    };
    let body = block.split_once('\n').map_or(block, |(_, rest)| rest);
    serde_json::from_str(body)
        .map_err(|error| format!("the fenced fixtures are not valid: {error}"))
}

/// The rule's answer to one case, or what makes the case malformed. A case
/// without `current` was decided on "", so a broken fixture could pass as a
/// refusal it never tested. The refuse cases leave the declaration out: an
/// absent declaration is an unset one, as AutoVersion's fixtures write it.
fn outcome(case: &Value) -> Result<Value, String> {
    let current = case["current"]
        .as_str()
        .ok_or("the case states no current version")?;
    let declared_breaking = match case.get("declared_breaking") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("declared_breaking is {value}, not a boolean"))?,
    };
    Ok(
        match decide(
            current,
            &names(case, "published")?,
            &names(case, "candidate")?,
            declared_breaking,
        ) {
            Ok(answer) => serde_json::json!({
                "class": answer.change.name(),
                "next": answer.next,
                "removed": answer.removed,
                "added": answer.added,
            }),
            Err(refusal) => serde_json::json!({ "refusal": refusal.name }),
        },
    )
}

/// Every `classify` case must yield exactly the recorded class, next version,
/// removed and added names; every `refuse` case exactly the recorded refusal.
/// Prints one line per case; `Ok(false)` when any differs or is malformed. A
/// case without a name is printed whole, not as "unnamed case".
pub(super) fn conformance(text: &str) -> Result<bool, String> {
    let fixtures = cases(text)?;
    let all: Vec<&Value> = fixtures.classify.iter().chain(&fixtures.refuse).collect();
    let mut failed = Vec::new();
    for case in &all {
        let name = match case["name"].as_str() {
            Some(name) => name.to_string(),
            None => format!("(a case that states no name: {case})"),
        };
        match outcome(case) {
            Ok(observed) if observed == case["expect"] => println!("OK   {name}"),
            Ok(observed) => {
                println!(
                    "FAIL {name}: expected {}, observed {observed}",
                    case["expect"]
                );
                failed.push(name);
            }
            Err(malformed) => {
                println!("FAIL {name}: malformed case: {malformed}");
                failed.push(name);
            }
        }
    }
    println!(
        "{} of {} fixture case(s) reproduced",
        all.len() - failed.len(),
        all.len()
    );
    Ok(failed.is_empty())
}
