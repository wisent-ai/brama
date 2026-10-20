//! A shell script split into tokens the way the Shell Command Language
//! recognises them (section 2.3): quotes, escapes, line continuations,
//! comments, operators, redirections, here-documents and command
//! substitutions, whose bodies are kept to be read as scripts of their own.
//! https://pubs.opengroup.org/onlinepubs/9799919799/utilities/V3_chap02.html

use std::iter::Peekable;
use std::str::Chars;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Token {
    /// A word as written, quotes included, so `"$X"` is told from `$X`.
    Word(String),
    /// `;`, `&`, `&&`, `||`, `(` or a newline: the next word starts a command.
    Separator,
    /// `|`: a pipe between commands, or an alternative inside a case pattern.
    Pipe,
    /// `;;`: the end of one case item.
    CaseBreak,
    /// `)`: the end of a case pattern or of a subshell.
    Close,
    /// A redirection operator: the next word is its target.
    Redirect,
}

struct HereDocument {
    delimiter: String,
    strip_tabs: bool,
}

/// One script's tokens, and the bodies of its `$( … )` and backquoted
/// command substitutions.
#[derive(Default)]
pub(super) struct Lexed {
    pub(super) tokens: Vec<Token>,
    pub(super) substitutions: Vec<String>,
    word: Option<String>,
    /// The next word is a here-document delimiter; `true` for `<<-`.
    delimiter_next: Option<bool>,
    here_documents: Vec<HereDocument>,
}

impl Lexed {
    fn word(&mut self) -> &mut String {
        self.word.get_or_insert_with(String::new)
    }

    fn finish(&mut self) {
        let Some(text) = self.word.take() else {
            return;
        };
        match self.delimiter_next.take() {
            Some(strip_tabs) => self.here_documents.push(HereDocument {
                delimiter: text
                    .chars()
                    .filter(|c| !matches!(c, '\'' | '"' | '\\'))
                    .collect(),
                strip_tabs,
            }),
            None => self.tokens.push(Token::Word(text)),
        }
    }

    fn operator(&mut self, token: Token) {
        self.finish();
        self.tokens.push(token);
    }
}

fn unterminated(what: &str) -> String {
    format!("the script ends inside {what}")
}

fn unclosed(document: &HereDocument) -> String {
    format!(
        "the here-document ending in {} is never closed",
        document.delimiter
    )
}

/// Up to the `close` that matches an already consumed `open`, quotes and
/// escapes honoured; the text between them.
fn enclosed(chars: &mut Peekable<Chars>, open: char, close: char) -> Result<String, String> {
    let mut body = String::new();
    let mut nested = Vec::new();
    while let Some(c) = chars.next() {
        if c == close && nested.pop().is_none() {
            return Ok(body);
        }
        body.push(c);
        match c {
            _ if c == open => nested.push(c),
            '\'' | '"' => quoted(chars, &mut body, c)?,
            '\\' => body.extend(chars.next()),
            _ => {}
        }
    }
    Err(unterminated(&format!("an unclosed `{open}`")))
}

/// The rest of a quoted string whose opening quote is already written.
fn quoted(chars: &mut Peekable<Chars>, into: &mut String, quote: char) -> Result<(), String> {
    while let Some(c) = chars.next() {
        into.push(c);
        if c == quote {
            return Ok(());
        }
        if c == '\\' && quote == '"' {
            into.extend(chars.next());
        }
    }
    Err(unterminated(&format!("a `{quote}` quotation")))
}

/// A `$(` substitution whose `$(` is consumed: written into the word and
/// kept as a script of its own.
fn substitution(chars: &mut Peekable<Chars>, lexed: &mut Lexed) -> Result<(), String> {
    let body = enclosed(chars, '(', ')')?;
    let word = lexed.word();
    word.push_str("$(");
    word.push_str(&body);
    word.push(')');
    lexed.substitutions.push(body);
    Ok(())
}

/// A backquoted substitution whose opening backquote is consumed.
fn backquoted(chars: &mut Peekable<Chars>, lexed: &mut Lexed) -> Result<(), String> {
    let mut body = String::new();
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                let word = lexed.word();
                word.push('`');
                word.push_str(&body);
                word.push('`');
                lexed.substitutions.push(body);
                return Ok(());
            }
            '\\' => body.extend(chars.next()),
            other => body.push(other),
        }
    }
    Err(unterminated("a backquoted command substitution"))
}

/// A double-quoted string whose opening quote is consumed; substitutions in
/// it are kept as scripts too.
fn double_quoted(chars: &mut Peekable<Chars>, lexed: &mut Lexed) -> Result<(), String> {
    lexed.word().push('"');
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                lexed.word().push('"');
                return Ok(());
            }
            '$' if chars.next_if_eq(&'(').is_some() => substitution(chars, lexed)?,
            '`' => backquoted(chars, lexed)?,
            '\\' => {
                let word = lexed.word();
                word.push('\\');
                word.extend(chars.next());
            }
            other => lexed.word().push(other),
        }
    }
    Err(unterminated("a `\"` quotation"))
}

/// The lines of one here-document, up to and including its delimiter line.
fn skip_here_document(chars: &mut Peekable<Chars>, document: &HereDocument) -> Result<(), String> {
    loop {
        let mut line = String::new();
        while let Some(c) = chars.next_if(|c| *c != '\n') {
            line.push(c);
        }
        let ended = chars.next().is_none();
        let line = if document.strip_tabs {
            line.trim_start_matches('\t')
        } else {
            &line
        };
        if line == document.delimiter {
            return Ok(());
        }
        if ended {
            return Err(unclosed(document));
        }
    }
}

/// `script` as tokens, or the construct it ends inside.
pub(super) fn lex(script: &str) -> Result<Lexed, String> {
    let mut chars = script.chars().peekable();
    let mut lexed = Lexed::default();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('\n') => {}
                Some(escaped) => lexed.word().extend(['\\', escaped]),
                None => return Err(unterminated("a backslash escape")),
            },
            ' ' | '\t' => lexed.finish(),
            '\n' => {
                lexed.operator(Token::Separator);
                for document in std::mem::take(&mut lexed.here_documents) {
                    skip_here_document(&mut chars, &document)?;
                }
            }
            '#' if lexed.word.is_none() => while chars.next_if(|c| *c != '\n').is_some() {},
            ';' if chars.next_if_eq(&';').is_some() => lexed.operator(Token::CaseBreak),
            ';' | '&' | '(' => {
                chars.next_if_eq(&'&');
                lexed.operator(Token::Separator);
            }
            '|' if chars.next_if_eq(&'|').is_some() => lexed.operator(Token::Separator),
            '|' => lexed.operator(Token::Pipe),
            ')' => lexed.operator(Token::Close),
            '<' | '>' => {
                // An IO number written right before the operator belongs to it.
                if lexed
                    .word
                    .as_deref()
                    .is_some_and(|word| word.chars().all(|d| d.is_ascii_digit()))
                {
                    lexed.word = None;
                }
                lexed.finish();
                if c == '<' && chars.next_if_eq(&'<').is_some() {
                    lexed.delimiter_next = Some(chars.next_if_eq(&'-').is_some());
                } else {
                    chars.next_if(|next| matches!(next, '<' | '>' | '&' | '|'));
                    lexed.tokens.push(Token::Redirect);
                }
            }
            '\'' => {
                let word = lexed.word();
                word.push('\'');
                quoted(&mut chars, word, '\'')?;
            }
            '"' => double_quoted(&mut chars, &mut lexed)?,
            '`' => backquoted(&mut chars, &mut lexed)?,
            '$' if chars.next_if_eq(&'(').is_some() => substitution(&mut chars, &mut lexed)?,
            '$' if chars.next_if_eq(&'{').is_some() => {
                let body = enclosed(&mut chars, '{', '}')?;
                let word = lexed.word();
                word.push_str("${");
                word.push_str(&body);
                word.push('}');
            }
            other => lexed.word().push(other),
        }
    }
    lexed.finish();
    if lexed.delimiter_next.is_some() {
        return Err(unterminated("a here-document operator without its delimiter"));
    }
    if let Some(document) = lexed.here_documents.first() {
        return Err(unclosed(document));
    }
    Ok(lexed)
}
