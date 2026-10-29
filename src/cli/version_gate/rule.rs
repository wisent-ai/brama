//! The fleet's versioning rule, taken from the AutoVersion crate
//! (github.com/lbartoszcze/AutoVersion) instead of a copy kept here: given
//! the published version, the published surface and the candidate surface,
//! what kind of change this is and what the next version is. `brama version
//! conformance` runs AutoVersion's fixtures against the crate Brama builds
//! with, and the version-check workflow runs it before it trusts a verdict.

use std::fmt;

use serde_json::Value;

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

impl From<autoversion::RuleError> for Refusal {
    fn from(error: autoversion::RuleError) -> Self {
        Self {
            name: error.refusal(),
            message: error.to_string(),
        }
    }
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
            Self::Breaking => autoversion::BREAKING,
            Self::Additive => autoversion::ADDITIVE,
            Self::Internal => autoversion::INTERNAL,
        }
    }

    /// The class AutoVersion answered. A class this Brama does not know is
    /// refused by name, so a newer rule is never read as an older class.
    fn answered(name: &str) -> Result<Self, Refusal> {
        match name {
            autoversion::BREAKING => Ok(Self::Breaking),
            autoversion::ADDITIVE => Ok(Self::Additive),
            autoversion::INTERNAL => Ok(Self::Internal),
            other => Err(Refusal {
                name: "unknown-change",
                message: format!(
                    "AutoVersion answered the class {other:?}, which this Brama does not know"
                ),
            }),
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

/// The whole answer, AutoVersion's. A declared break may only escalate the
/// class.
pub(super) fn decide(
    current: &str,
    published: &[String],
    candidate: &[String],
    declared_breaking: bool,
) -> Result<Decision, Refusal> {
    let version = autoversion::Version::parse(current)?;
    let (change, removed, added) = autoversion::classify(published, candidate, declared_breaking)?;
    Ok(Decision {
        current: version.to_string(),
        change: Change::answered(change)?,
        next: version.advance(change).to_string(),
        removed,
        added,
    })
}

/// One surface list of a fixture case. A case without it, or with an entry
/// that is not text, is malformed, so it never reproduces the `empty-surface`
/// refusal by accident.
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

/// The fixtures of FIXTURES.md: a text that opens with `{` is the JSON
/// document itself; any other is read between the first pair of ``` fences,
/// after the opening fence's info line.
fn cases(text: &str) -> Result<Fixtures, String> {
    if text.trim_start().starts_with('{') {
        return serde_json::from_str(text)
            .map_err(|error| format!("the fixtures JSON document is not valid: {error}"));
    }
    let (_, after) = text
        .split_once("```")
        .ok_or("the fixtures file is neither a JSON document nor holds a fenced block")?;
    let (block, _) = after
        .split_once("```")
        .ok_or("the fixtures file's fenced block is never closed")?;
    let (_info, body) = block
        .split_once('\n')
        .ok_or("the fixtures file's fenced block has no line after its opening fence")?;
    serde_json::from_str(body)
        .map_err(|error| format!("the fenced fixtures are not valid: {error}"))
}

/// The rule's answer to one case, or what makes the case malformed. A case
/// without `current` is malformed, so a broken fixture never passes as a
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
