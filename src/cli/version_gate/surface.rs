//! Brama's public surface: the top-level commands the `brama` binary
//! advertises. Adding one is a capability; removing or renaming one breaks
//! whoever scripted it. Hidden commands and clap's own `help` are not part of
//! it. HTTP routes are reachable only through `brama serve`, so the command
//! list is the outer boundary and the thing versioned.
//!
//! The candidate's surface is read from this binary's own clap declaration.
//! A published revision's surface is read from that revision's `src/main.rs`
//! through `git show`, parsed as Rust (syn): the struct deriving `Parser`,
//! its `#[command(subcommand)]` field's type, and that enum's variants with
//! their `#[command(...)]` attributes, read the way clap's derive reads them.
//! `surface` refuses when the parse of the working tree disagrees with the
//! binary, so the reading stays honest.

use std::path::Path;
use std::process::Command;

use syn::punctuated::Punctuated;
use syn::{Attribute, Fields, Item, Token};

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

/// `git show REFERENCE:PATH` in `root`, announced while it runs. A blob that
/// is not UTF-8 is refused by name: read lossily it would be parsed as text
/// it is not.
pub(super) fn git_show(root: &Path, reference: &str, path: &str) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(["show", &format!("{reference}:{path}")]);
    let output = stado_wait::output(&mut command).map_err(|error| format!("git show: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{reference}:{path} is not readable here: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("{reference}:{path} is not UTF-8 text: {error}"))
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

/// A revision's declared surface and version.
pub(super) fn of_revision(root: &Path, reference: &str) -> Result<(Vec<String>, String), String> {
    let surface = declared(&git_show(root, reference, MAIN)?)?;
    let version = manifest_version(&git_show(root, reference, MANIFEST)?)?;
    Ok((surface, version))
}

/// The working tree's declared surface.
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
    let mut rendered = String::new();
    let mut previous: Option<char> = None;
    let mut characters = variant.chars().peekable();
    while let Some(character) = characters.next() {
        let next = characters.peek().copied();
        let boundary = character.is_ascii_uppercase()
            && previous.is_some_and(|before| {
                before.is_ascii_lowercase()
                    || before.is_ascii_digit()
                    || (before.is_ascii_uppercase()
                        && next.is_some_and(|after| after.is_ascii_lowercase()))
            });
        if boundary {
            rendered.push('-');
        }
        rendered.push(if character == '_' {
            '-'
        } else {
            character.to_ascii_lowercase()
        });
        previous = Some(character);
    }
    rendered
}

/// What clap's derive reads from the `#[command(...)]` attributes on one
/// item: these keys are clap's attribute grammar, not a classification of
/// ours.
#[derive(Default)]
struct CommandAttributes {
    subcommand: bool,
    hidden: bool,
    skipped: bool,
    name: Option<String>,
    rename_all: Option<String>,
}

fn command_attributes(attributes: &[Attribute]) -> Result<CommandAttributes, String> {
    let mut read = CommandAttributes::default();
    for attribute in attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("command"))
    {
        attribute
            .parse_nested_meta(|meta| {
                if meta.path.is_ident("subcommand") {
                    read.subcommand = true;
                } else if meta.path.is_ident("hide") {
                    read.hidden = meta.value()?.parse::<syn::LitBool>()?.value;
                } else if meta.path.is_ident("skip") {
                    read.skipped = true;
                    if meta.input.peek(Token![=]) {
                        meta.value()?.parse::<syn::Expr>()?;
                    }
                } else if meta.path.is_ident("name") {
                    read.name = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                } else if meta.path.is_ident("rename_all") {
                    read.rename_all = Some(meta.value()?.parse::<syn::LitStr>()?.value());
                } else if meta.input.peek(Token![=]) {
                    meta.value()?.parse::<syn::Expr>()?;
                } else if meta.input.peek(syn::token::Paren) {
                    let content;
                    syn::parenthesized!(content in meta.input);
                    content.step(|cursor| {
                        let mut rest = *cursor;
                        while let Some((_, next)) = rest.token_tree() {
                            rest = next;
                        }
                        Ok(((), rest))
                    })?;
                }
                Ok(())
            })
            .map_err(|error| format!("a #[command] attribute in {MAIN} does not parse: {error}"))?;
    }
    Ok(read)
}

/// Whether `attributes` derive the trait `name`.
fn derives(attributes: &[Attribute], name: &str) -> Result<bool, String> {
    for attribute in attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("derive"))
    {
        let paths = attribute
            .parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
            .map_err(|error| format!("a #[derive] in {MAIN} does not parse: {error}"))?;
        if paths.iter().any(|path| {
            path.segments
                .last()
                .is_some_and(|segment| segment.ident == name)
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The type a field names: `Option<Commands>` and `Commands` both name
/// `Commands`.
fn field_type_name(kind: &syn::Type) -> Option<String> {
    let syn::Type::Path(path) = kind else {
        return None;
    };
    let last = path.path.segments.last()?;
    if last.ident == "Option" {
        if let syn::PathArguments::AngleBracketed(arguments) = &last.arguments {
            return arguments.args.iter().find_map(|argument| match argument {
                syn::GenericArgument::Type(inner) => field_type_name(inner),
                _ => None,
            });
        }
    }
    Some(last.ident.to_string())
}

/// Every command the clap declaration in a `src/main.rs` text advertises.
pub(super) fn declared(source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source)
        .map_err(|error| format!("{MAIN} does not parse as Rust: {error}"))?;
    let mut subcommand_type = None;
    for item in &file.items {
        let Item::Struct(parser) = item else {
            continue;
        };
        if !derives(&parser.attrs, "Parser")? {
            continue;
        }
        let Fields::Named(fields) = &parser.fields else {
            return Err(format!("the Parser struct in {MAIN} has no named fields"));
        };
        for field in &fields.named {
            if command_attributes(&field.attrs)?.subcommand {
                subcommand_type = Some(
                    field_type_name(&field.ty)
                        .ok_or_else(|| format!("the subcommand field in {MAIN} names no type"))?,
                );
            }
        }
    }
    let name = subcommand_type.ok_or_else(|| {
        format!("no #[derive(Parser)] struct with a #[command(subcommand)] field in {MAIN}")
    })?;
    let commands = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Enum(declared) if declared.ident == name => Some(declared),
            _ => None,
        })
        .ok_or_else(|| format!("no `enum {name}` in {MAIN}"))?;
    if let Some(rule) = command_attributes(&commands.attrs)?.rename_all {
        if rule != "kebab-case" {
            return Err(format!(
                "enum {name} renames its commands {rule}; only clap's default kebab-case is rendered"
            ));
        }
    }
    let mut names = std::collections::BTreeSet::new();
    for variant in &commands.variants {
        let read = command_attributes(&variant.attrs)?;
        if read.hidden || read.skipped {
            continue;
        }
        names.insert(match read.name {
            Some(named) => named,
            None => kebab(&variant.ident.to_string()),
        });
    }
    names.remove(FRAMEWORK_COMMAND);
    if names.is_empty() {
        return Err(format!("no commands found in {MAIN}"));
    }
    Ok(names.into_iter().collect())
}
