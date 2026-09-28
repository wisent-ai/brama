//! The three tools a reviewing model is given: read one file, search every
//! file, and submit the verdict. Reads resolve every path (symlinks included)
//! and refuse anything that lands outside the review root, so the model sees
//! the directory the caller named and nothing else on the machine.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    Approve,
    RequestChanges,
}

impl Verdict {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::RequestChanges => "request_changes",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        [Self::Approve, Self::RequestChanges]
            .into_iter()
            .find(|verdict| verdict.name() == value)
    }
}

pub(super) fn schema() -> Value {
    json!([
        {"type": "function", "function": {
            "name": "read_file",
            "description": "Read numbered lines of one file under the review root; without a range, the whole file. Lines count from one.",
            "parameters": {"type": "object", "additionalProperties": false, "required": ["path"], "properties": {
                "path": {"type": "string", "description": "Path relative to the review root"},
                "start_line": {"type": "integer"},
                "end_line": {"type": "integer"},
            }},
        }},
        {"type": "function", "function": {
            "name": "search",
            "description": "Find a non-empty literal string in every file under the review root; answers path:line:text for each matching line.",
            "parameters": {"type": "object", "additionalProperties": false, "required": ["query"], "properties": {
                "query": {"type": "string"},
                "case_sensitive": {"type": "boolean"},
            }},
        }},
        {"type": "function", "function": {
            "name": "submit_verdict",
            "description": "End the review: the verdict and the complete, non-empty review text posted for the author.",
            "parameters": {"type": "object", "additionalProperties": false, "required": ["verdict", "review"], "properties": {
                "verdict": {"type": "string", "enum": [Verdict::Approve.name(), Verdict::RequestChanges.name()]},
                "review": {"type": "string"},
            }},
        }},
    ])
}

fn name_and_arguments(call: &Value) -> Result<(&str, Value), String> {
    let function = call
        .get("function")
        .ok_or("a tool call names no function")?;
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .ok_or("a tool call names no function")?;
    let arguments = match function.get("arguments") {
        Some(Value::String(text)) if text.trim().is_empty() => json!({}),
        Some(Value::String(text)) => serde_json::from_str(text)
            .map_err(|error| format!("{name} arguments are not JSON: {error}"))?,
        Some(value) => value.clone(),
        None => json!({}),
    };
    Ok((name, arguments))
}

/// The verdict, when this call submits one. A submission that breaks the
/// schema fails the review instead of being read as either verdict.
pub(super) fn submitted(call: &Value) -> Result<Option<(Verdict, String)>, String> {
    let (name, arguments) = name_and_arguments(call)?;
    if name != "submit_verdict" {
        return Ok(None);
    }
    let verdict = arguments
        .get("verdict")
        .and_then(Value::as_str)
        .and_then(Verdict::parse)
        .ok_or_else(|| format!("submit_verdict carries no valid verdict: {arguments}"))?;
    let review = arguments
        .get("review")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or("submit_verdict carries no review text")?;
    Ok(Some((verdict, review.to_string())))
}

pub(super) struct Tree {
    root: PathBuf,
}

impl Tree {
    pub(super) fn open(root: &Path) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|error| format!("review root {}: {error}", root.display()))?;
        if !root.is_dir() {
            return Err(format!("review root {} is not a directory", root.display()));
        }
        Ok(Self { root })
    }

    /// The tool's answer, or the sentence saying why it refused; a refusal
    /// goes back to the model, which may correct the call.
    pub(super) fn answer(&self, call: &Value) -> String {
        let outcome = name_and_arguments(call).and_then(|(name, arguments)| match name {
            "read_file" => self.read(&arguments),
            "search" => self.search(&arguments),
            other => Err(format!("unknown tool {other}")),
        });
        outcome.unwrap_or_else(|reason| format!("tool refused: {reason}"))
    }

    fn inside(&self, relative: &str) -> Result<PathBuf, String> {
        let candidate = self
            .root
            .join(relative)
            .canonicalize()
            .map_err(|error| format!("{relative}: {error}"))?;
        if !candidate.starts_with(&self.root) || !candidate.is_file() {
            return Err(format!("{relative} is not a file under the review root"));
        }
        Ok(candidate)
    }

    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .display()
            .to_string()
    }

    fn read(&self, arguments: &Value) -> Result<String, String> {
        let relative = arguments
            .get("path")
            .and_then(Value::as_str)
            .ok_or("path is required")?;
        let text = std::fs::read(self.inside(relative)?)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|error| format!("{relative}: {error}"))?;
        let line = |key| {
            arguments
                .get(key)
                .and_then(Value::as_u64)
                .map(|value| value as usize)
        };
        let start = line("start_line").unwrap_or(1).max(1);
        let end = line("end_line").unwrap_or(usize::MAX);
        if end < start {
            return Err("end_line is before start_line".to_string());
        }
        let selected = text
            .lines()
            .enumerate()
            .map(|(index, text)| (index + 1, text))
            .filter(|(number, _)| (start..=end).contains(number))
            .map(|(number, text)| format!("{number}:{text}"))
            .collect::<Vec<_>>();
        Ok(if selected.is_empty() {
            "(no lines in range)".to_string()
        } else {
            selected.join("\n")
        })
    }

    fn search(&self, arguments: &Value) -> Result<String, String> {
        let query = arguments
            .get("query")
            .and_then(Value::as_str)
            .filter(|query| !query.is_empty())
            .ok_or("query is required")?;
        let exact = arguments
            .get("case_sensitive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let fold = |text: &str| {
            if exact {
                text.to_string()
            } else {
                text.to_lowercase()
            }
        };
        let needle = fold(query);
        let mut files = Vec::new();
        walk(&self.root, &mut files)?;
        files.sort();
        let mut matches = Vec::new();
        for path in files {
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("{}: {error}", self.relative(&path)))?;
            let text = String::from_utf8_lossy(&bytes);
            for (index, line) in text.lines().enumerate() {
                if fold(line).contains(&needle) {
                    matches.push(format!("{}:{}:{line}", self.relative(&path), index + 1));
                }
            }
        }
        Ok(if matches.is_empty() {
            "(no matches)".to_string()
        } else {
            matches.join("\n")
        })
    }
}

/// Every regular file below `directory`, skipping `.git`; symlinks are not
/// followed, so the walk cannot leave the root.
fn walk(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
        let kind = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        if kind.is_dir() && entry.file_name() != ".git" {
            walk(&entry.path(), files)?;
        } else if kind.is_file() {
            files.push(entry.path());
        }
    }
    Ok(())
}
