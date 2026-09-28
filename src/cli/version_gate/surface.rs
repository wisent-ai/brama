//! Brama's public surface: the top-level commands the `brama` binary
//! advertises. Adding one is a capability; removing or renaming one breaks
//! whoever scripted it. Hidden commands and clap's own `help` are not part of
//! it. HTTP routes are reachable only through `brama serve`, so the command
//! list is the outer boundary and the thing versioned.
//!
//! The candidate's surface is read from this binary's own clap declaration.
//! A published revision's surface is read from that revision's `src/main.rs`
//! through `git show`, by scanning the enum the `#[derive(Parser)]` struct's
//! `#[command(subcommand)]` field names; `surface` refuses when the scan of
//! the working tree disagrees with the binary, so the scan stays honest.

use std::path::Path;
use std::process::Command;

/// The meta-command every clap binary prints; framework, not product.
const FRAMEWORK_COMMAND: &str = "help";
const MAIN: &str = "src/main.rs";
const MANIFEST: &str = "Cargo.toml";

/// The commands a clap command tree advertises, sorted.
pub(super) fn advertised(command: &clap::Command) -> Vec<String> {
    let mut names: Vec<String> = command
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set() && sub.get_name() != FRAMEWORK_COMMAND)
        .map(|sub| sub.get_name().to_string())
        .collect();
    names.sort();
    names
}

/// `git show REFERENCE:PATH` in `root`.
pub(super) fn git_show(root: &Path, reference: &str, path: &str) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", &format!("{reference}:{path}")])
        .output()
        .map_err(|error| format!("git show: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{reference}:{path} is not readable here: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The `[package] version` a manifest text declares.
pub(super) fn manifest_version(text: &str) -> Result<String, String> {
    text.lines()
        .find_map(|line| {
            let rest = line
                .strip_prefix("version")?
                .trim_start()
                .strip_prefix('=')?;
            let quoted = rest.trim().strip_prefix('"')?;
            quoted.split('"').next().map(str::to_string)
        })
        .filter(|version| !version.is_empty())
        .ok_or_else(|| format!("{MANIFEST} declares no package version"))
}

/// A revision's scanned surface and declared version.
pub(super) fn of_revision(root: &Path, reference: &str) -> Result<(Vec<String>, String), String> {
    let surface = declared(&git_show(root, reference, MAIN)?)?;
    let version = manifest_version(&git_show(root, reference, MANIFEST)?)?;
    Ok((surface, version))
}

/// The working tree's scanned surface.
pub(super) fn of_tree(root: &Path) -> Result<Vec<String>, String> {
    let path = root.join(MAIN);
    declared(
        &std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?,
    )
}

/// Clap's default kebab-case: split where a lowercase letter or digit meets
/// an uppercase one, and where an uppercase run meets an uppercase letter
/// followed by a lowercase one.
fn kebab(variant: &str) -> String {
    let characters: Vec<char> = variant.chars().collect();
    let mut rendered = String::new();
    for (index, character) in characters.iter().enumerate() {
        let previous = index.checked_sub(1).map(|at| characters[at]);
        let next = characters.get(index + 1);
        let boundary = character.is_ascii_uppercase()
            && previous.is_some_and(|before| {
                before.is_ascii_lowercase()
                    || before.is_ascii_digit()
                    || (before.is_ascii_uppercase() && next.is_some_and(char::is_ascii_lowercase))
            });
        if boundary {
            rendered.push('-');
        }
        rendered.push(if *character == '_' {
            '-'
        } else {
            character.to_ascii_lowercase()
        });
    }
    rendered
}

/// The type of the `#[command(subcommand)]` field after `#[derive(... Parser ...)]`.
fn subcommand_enum(source: &str) -> Result<String, String> {
    let parser = source
        .match_indices("#[derive(")
        .find(|(at, _)| {
            source[*at..]
                .split(')')
                .next()
                .is_some_and(|derive| derive.contains("Parser"))
        })
        .map(|(at, _)| at)
        .ok_or(format!("no #[derive(Parser)] entry point in {MAIN}"))?;
    let after = &source[parser..];
    let field = after
        .find("(subcommand)]")
        .map(|at| &after[at + "(subcommand)]".len()..])
        .ok_or(format!(
            "the Parser struct in {MAIN} declares no subcommand field"
        ))?;
    let declaration = field
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("//"))
        .unwrap_or("");
    let kind = declaration
        .split_once(':')
        .map(|(_, kind)| kind)
        .unwrap_or("");
    let kind = kind
        .trim()
        .trim_start_matches("Option")
        .trim_start_matches('<')
        .trim();
    let name: String = kind
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        return Err(format!("the subcommand field in {MAIN} names no type"));
    }
    Ok(name)
}

/// Every command the clap declaration in a `src/main.rs` text advertises.
pub(super) fn declared(source: &str) -> Result<Vec<String>, String> {
    let name = subcommand_enum(source)?;
    let opener = format!("enum {name} {{");
    let start = source
        .find(&opener)
        .ok_or(format!("no `enum {name}` in {MAIN}"))?;
    let attached: Vec<&str> = source[..start]
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("//"))
        .take_while(|line| line.starts_with("#["))
        .collect();
    if attached
        .iter()
        .any(|line| line.contains("rename_all") && !line.contains("kebab-case"))
    {
        return Err(format!(
            "enum {name} overrides rename_all; only clap's default kebab-case is rendered"
        ));
    }
    let body_start = start + opener.len();
    let mut depth = 1usize;
    let body_end = source[body_start..]
        .char_indices()
        .find_map(|(at, character)| {
            match character {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(body_start + at)
        })
        .ok_or(format!("`enum {name}` is never closed in {MAIN}"))?;
    let mut names = std::collections::BTreeSet::new();
    let mut attributes = String::new();
    let mut nesting = 0i64;
    for line in source[body_start..body_end].lines().map(str::trim) {
        if nesting == 0 && !line.is_empty() && !line.starts_with("//") {
            if line.starts_with("#[") {
                attributes.push_str(line);
                attributes.push(' ');
            } else if line.starts_with(|c: char| c.is_ascii_uppercase()) {
                let variant: String = line
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                let hidden = attributes.contains("hide = true") || attributes.contains("skip");
                if !hidden {
                    let named = attributes
                        .split_once("name = \"")
                        .and_then(|(_, rest)| rest.split('"').next())
                        .map(str::to_string);
                    names.insert(named.unwrap_or_else(|| kebab(&variant)));
                }
                attributes.clear();
            }
        }
        let opens = line.matches(['{', '(', '[']).count() as i64;
        let closes = line.matches(['}', ')', ']']).count() as i64;
        nesting += opens - closes;
    }
    names.remove(FRAMEWORK_COMMAND);
    if names.is_empty() {
        return Err(format!("no commands found in {MAIN}"));
    }
    Ok(names.into_iter().collect())
}
