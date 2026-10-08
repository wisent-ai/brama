//! The last answer each alias got from what it routes to.
//!
//! `brama aliases` and `stado inference route show` compared declarations
//! and credential presence only, so an alias whose provider refused every
//! request (an OpenRouter or OpenAI account without credits) read as serving
//! for days while every caller was refused (f78995c7). The gateway already
//! classes each answer; this keeps the newest one per alias in
//! `$BRAMA_STATE_DIR/alias-answers.json`, written only when an alias's answer
//! changes (answered to refused, or one refusal class to another), so a
//! reader that is not the gateway — the CLI, Stado over the host channel —
//! sees what the gateway saw.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

/// The newest answer one alias got.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastAnswer {
    /// Whether the route produced a generation.
    pub answered: bool,
    /// The refusal's contract code (`provider_quota_exhausted`, ...) when it
    /// did not.
    pub refusal: Option<String>,
    /// The refusal's own sentence, when it gave one.
    pub message: Option<String>,
    /// When this answer was first seen (RFC 3339).
    pub at: String,
}

fn path() -> PathBuf {
    super::state_dir().join("alias-answers.json")
}

/// Every alias's newest answer as the gateway recorded it; an absent file is
/// a gateway that has served nothing yet.
pub fn last_answers() -> Result<BTreeMap<String, LastAnswer>, std::io::Error> {
    let path = path();
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not an alias answer record: {error}", path.display()),
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error),
    }
}

/// The gateway's copy, read once; an unreadable record is replaced by the
/// next write, and said so.
static LAST: LazyLock<Mutex<BTreeMap<String, LastAnswer>>> = LazyLock::new(|| {
    match last_answers() {
        Ok(known) => Mutex::new(known),
        Err(error) => {
            tracing::warn!(event = "alias_answers_unreadable", %error, "the alias answer record is rewritten from this process's answers");
            Mutex::new(BTreeMap::new())
        }
    }
});

/// Record what `alias` was just answered: `None` for a generation, else the
/// refusal's contract code and sentence. Writes only when the alias's answer
/// changed.
pub fn record(alias: &str, refusal: Option<(&str, Option<&str>)>) {
    let Ok(mut last) = LAST.lock() else {
        return;
    };
    let answered = refusal.is_none();
    let class = refusal.map(|(code, _)| code.to_string());
    if last
        .get(alias)
        .is_some_and(|known| known.answered == answered && known.refusal == class)
    {
        return;
    }
    last.insert(
        alias.to_string(),
        LastAnswer {
            answered,
            refusal: class,
            message: refusal.and_then(|(_, message)| message.map(str::to_string)),
            at: super::now(),
        },
    );
    if let Err(error) = write(&last) {
        tracing::warn!(event = "alias_answers_unwritten", %error, alias, "the alias answer record could not be written");
    }
}

/// Replace the record in one rename, so a reader never sees half of it.
fn write(answers: &BTreeMap<String, LastAnswer>) -> Result<(), std::io::Error> {
    let path = path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staged = path.with_extension("json.next");
    std::fs::write(&staged, serde_json::to_vec_pretty(answers)?)?;
    std::fs::rename(&staged, &path)
}

/// The running gateway's own copy of every alias's newest answer, for the
/// HTTP view: the same record `brama aliases` reads from the file.
pub fn current() -> BTreeMap<String, LastAnswer> {
    match LAST.lock() {
        Ok(last) => last.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}
