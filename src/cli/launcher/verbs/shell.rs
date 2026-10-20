//! The router calls a POSIX shell script makes, read with the shell's own
//! grammar rather than by scanning its text.
//!
//! A call is a simple command (section 2.9.1) whose command word is the
//! router literal: its words after that literal. An operand
//! (`--router "$…"`), a test operand (`[ -x "$…" ]`), an assignment
//! (`NAME="$…"`), a redirection target or a case pattern holding the literal
//! is no call. Command substitutions are read as scripts of their own.
//! https://pubs.opengroup.org/onlinepubs/9799919799/utilities/V3_chap02.html

use super::lexer::{lex, Token};

/// The `exec` special built-in: its operands are the command it runs
/// (https://pubs.opengroup.org/onlinepubs/9799919799/utilities/V3_chap02.html#exec).
const EXEC: &str = "exec";

/// A command word: a lowercase letter, then lowercase letters and dashes.
fn is_word(token: &str) -> bool {
    let mut chars = token.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c == '-')
}

/// `NAME=value` (section 2.9.1): an environment prefix of the command after it.
fn is_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// A reserved word (section 2.4) that opens or closes a compound command, so
/// the word after it is again a command word; or one that starts a loop or a
/// case, whose next words are not commands. These spellings are the shell
/// language's own grammar, not a classification of ours.
enum Reserved {
    Compound,
    Loop,
    Case,
}

fn reserved(word: &str) -> Option<Reserved> {
    match word {
        "!" | "{" | "}" | "if" | "then" | "else" | "elif" | "fi" | "while" | "until" | "do"
        | "done" | "esac" => Some(Reserved::Compound),
        "for" => Some(Reserved::Loop),
        "case" => Some(Reserved::Case),
        _ => None,
    }
}

/// Where the walk is in the grammar.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    /// Commands: the first word of a simple command is its command word.
    Command,
    /// After `for`: the loop variable and its word list, up to a separator or `do`.
    LoopWords,
    /// After `case`: its subject word, then `in`.
    CaseSubject,
    /// Inside `case … in`: a pattern up to its `)`, or `esac`.
    Pattern,
}

/// End the simple command `command`, keeping its verbs when its command word
/// is `router`.
fn flush(command: &mut Vec<&str>, router: &str, calls: &mut Vec<Vec<String>>) {
    if let [word, operands @ ..] = command.as_slice() {
        if *word == router {
            let verbs: Vec<String> = operands
                .iter()
                .take_while(|operand| is_word(operand))
                .map(|operand| operand.to_string())
                .collect();
            if !verbs.is_empty() {
                calls.push(verbs);
            }
        }
    }
    command.clear();
}

/// The words after `router` of every simple command in `script` whose
/// command word is `router` as written, substitutions included; a script
/// the grammar cannot read is refused with the construct it ends inside.
pub(super) fn router_calls(script: &str, router: &str) -> Result<Vec<Vec<String>>, String> {
    let lexed = lex(script)?;
    let mut calls = Vec::new();
    let mut position = Position::Command;
    let mut command: Vec<&str> = Vec::new();
    let mut target_next = false;
    for token in &lexed.tokens {
        match token {
            Token::Word(_) if target_next => target_next = false,
            Token::Word(word) => match position {
                Position::Command if command.is_empty() => match reserved(word) {
                    Some(Reserved::Compound) => {}
                    Some(Reserved::Loop) => position = Position::LoopWords,
                    Some(Reserved::Case) => position = Position::CaseSubject,
                    None if is_assignment(word) || word == EXEC => {}
                    None => command.push(word),
                },
                Position::Command => command.push(word),
                Position::LoopWords if word == "do" => position = Position::Command,
                Position::CaseSubject if word == "in" => position = Position::Pattern,
                Position::Pattern if word == "esac" => position = Position::Command,
                Position::LoopWords | Position::CaseSubject | Position::Pattern => {}
            },
            Token::Redirect => target_next = true,
            Token::Separator => {
                flush(&mut command, router, &mut calls);
                if !matches!(position, Position::Pattern | Position::CaseSubject) {
                    position = Position::Command;
                }
            }
            Token::Pipe if position == Position::Pattern => {}
            Token::Pipe => flush(&mut command, router, &mut calls),
            Token::CaseBreak => {
                flush(&mut command, router, &mut calls);
                position = Position::Pattern;
            }
            Token::Close => {
                flush(&mut command, router, &mut calls);
                position = Position::Command;
            }
        }
    }
    flush(&mut command, router, &mut calls);
    for body in &lexed.substitutions {
        calls.extend(router_calls(body, router)?);
    }
    Ok(calls)
}
