# Lexer Implementation Plan — LO (LiveOak) P1

Implements everything decided in `lexer_design.md`. No project exists yet, so this
plan assumes a new Cargo binary crate. Adjust the crate name/path if you want something
different — nothing below depends on the name.

**Assumption:** new crate at `/Users/jay/Documents/College/Fall2026/PL/P1/lo-compiler/`,
package name `lo-compiler`, edition 2021, no external dependencies.

---

## Files to create

| File | Contents |
|---|---|
| `Cargo.toml` | package manifest, no deps |
| `src/main.rs` | wires the modules, no CLI logic yet |
| `src/token.rs` | `Token`, `TokenKind` |
| `src/lexer.rs` | `LexError`, `LexErrorKind`, `tokenize`, and the private `Lexer` struct that does the work |

---

## Decisions resolved for this implementation

These were open items in `lexer_design.md`. Picking a concrete behavior now so there's
nothing left for an implementer to guess.

| Item | Decision |
|---|---|
| Block comments `/* */` | Not implemented. `/` always produces a `Slash` token unless immediately followed by a second `/`. If a test program uses `/* */`, it will fail to parse later — that failure is the signal to revisit this. |
| Integer literal overflow | Lex-time error: `LexErrorKind::IntegerLiteralOverflow(String)`, carrying the offending literal text. |
| Escape character that isn't `" \ n t r u` | Lex-time error: `LexErrorKind::InvalidEscape(char)`. |
| Raw (unescaped) newline inside a string literal | Allowed. Pushed into the string value as-is, and the line counter advances (the string token's own `line` stays pinned to where the opening `"` was). |
| `\u{}` with zero hex digits, missing `{`, missing `}`, or a value `char::from_u32` rejects | All map to `LexErrorKind::InvalidUnicodeEscape`. |

---

## `Cargo.toml`

```toml
[package]
name = "lo-compiler"
version = "0.1.0"
edition = "2021"
```

## `src/main.rs`

```rust
mod token;
mod lexer;

fn main() {
    // CLI entry point comes later. Nothing to do yet.
}
```

## `src/token.rs`

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Num(i32),
    Str(String),

    KwInt, KwBool, KwString, KwVoid,
    KwClass, KwExtends, KwThis, KwSuper, KwNull, KwNew,
    KwReturn, KwIf, KwElse, KwWhile, KwBreak,
    KwTrue, KwFalse, KwInstanceof,

    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    Semicolon, Comma, Dot, Question, Colon, Equals,
    Plus, Minus, Star, Slash, Percent, Amp, Pipe, Lt, Gt, Tilde, Bang,

    Eof,
}
```

## `src/lexer.rs`

```rust
use crate::token::{Token, TokenKind};
use std::iter::Peekable;
use std::str::Chars;

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub kind: LexErrorKind,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LexErrorKind {
    UnexpectedChar(char),
    UnterminatedString,
    InvalidEscape(char),
    InvalidUnicodeEscape,
    IntegerLiteralOverflow(String),
}

/// Entry point. Tokenizes the whole source string. Always ends the returned
/// vector with exactly one `Eof` token. Fails on the first error (no recovery).
pub fn tokenize(source: &str) -> Result<Vec<Token>, LexError> {
    Lexer::new(source).run()
}

struct Lexer<'a> {
    chars: Peekable<Chars<'a>>,
    line: u32,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Lexer {
            chars: source.chars().peekable(),
            line: 1,
        }
    }

    fn run(mut self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();

        loop {
            self.skip_whitespace_and_comments();
            let line = self.line;

            let c = match self.chars.next() {
                None => {
                    tokens.push(Token { kind: TokenKind::Eof, line });
                    return Ok(tokens);
                }
                Some(c) => c,
            };

            let kind = match c {
                '(' => TokenKind::LParen,
                ')' => TokenKind::RParen,
                '{' => TokenKind::LBrace,
                '}' => TokenKind::RBrace,
                '[' => TokenKind::LBracket,
                ']' => TokenKind::RBracket,
                ';' => TokenKind::Semicolon,
                ',' => TokenKind::Comma,
                '.' => TokenKind::Dot,
                '?' => TokenKind::Question,
                ':' => TokenKind::Colon,
                '=' => TokenKind::Equals,
                '+' => TokenKind::Plus,
                '-' => TokenKind::Minus,
                '*' => TokenKind::Star,
                '/' => TokenKind::Slash,
                '%' => TokenKind::Percent,
                '&' => TokenKind::Amp,
                '|' => TokenKind::Pipe,
                '<' => TokenKind::Lt,
                '>' => TokenKind::Gt,
                '~' => TokenKind::Tilde,
                '!' => TokenKind::Bang,

                '"' => {
                    tokens.push(self.scan_string(line)?);
                    continue;
                }
                '0'..='9' => {
                    tokens.push(self.scan_number(c, line)?);
                    continue;
                }
                c if c.is_ascii_alphabetic() => {
                    tokens.push(self.scan_ident_or_keyword(c, line));
                    continue;
                }

                other => {
                    return Err(LexError { kind: LexErrorKind::UnexpectedChar(other), line });
                }
            };

            tokens.push(Token { kind, line });
        }
    }

    /// Consumes whitespace and `//` line comments. Stops the instant it sees a
    /// character that starts a real token (including a lone `/`).
    fn skip_whitespace_and_comments(&mut self) {
        loop {
            match self.chars.peek() {
                Some(' ') | Some('\t') | Some('\r') => {
                    self.chars.next();
                }
                Some('\n') => {
                    self.chars.next();
                    self.line += 1;
                }
                Some('/') => {
                    // Peek a second character ahead without consuming from the
                    // real iterator, since Peekable only exposes one char of
                    // lookahead. A cloned iterator is free to advance and discard.
                    let mut ahead = self.chars.clone();
                    ahead.next();
                    if ahead.peek() == Some(&'/') {
                        self.chars.next(); // first '/'
                        self.chars.next(); // second '/'
                        while let Some(&next) = self.chars.peek() {
                            if next == '\n' {
                                break;
                            }
                            self.chars.next();
                        }
                        // leave the '\n' itself for this same loop to consume
                        // next iteration, so the line counter increments exactly once
                    } else {
                        return; // a real division-operator token starts here
                    }
                }
                _ => return,
            }
        }
    }

    fn scan_number(&mut self, first: char, line: u32) -> Result<Token, LexError> {
        let mut text = String::new();
        text.push(first);
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_digit() {
                text.push(c);
                self.chars.next();
            } else {
                break;
            }
        }
        match text.parse::<i32>() {
            Ok(n) => Ok(Token { kind: TokenKind::Num(n), line }),
            Err(_) => Err(LexError {
                kind: LexErrorKind::IntegerLiteralOverflow(text),
                line,
            }),
        }
    }

    fn scan_ident_or_keyword(&mut self, first: char, line: u32) -> Token {
        let mut text = String::new();
        text.push(first);
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '\'' {
                text.push(c);
                self.chars.next();
            } else {
                break;
            }
        }

        let kind = match text.as_str() {
            "int" => TokenKind::KwInt,
            "bool" => TokenKind::KwBool,
            "String" => TokenKind::KwString,
            "void" => TokenKind::KwVoid,
            "class" => TokenKind::KwClass,
            "extends" => TokenKind::KwExtends,
            "this" => TokenKind::KwThis,
            "super" => TokenKind::KwSuper,
            "null" => TokenKind::KwNull,
            "new" => TokenKind::KwNew,
            "return" => TokenKind::KwReturn,
            "if" => TokenKind::KwIf,
            "else" => TokenKind::KwElse,
            "while" => TokenKind::KwWhile,
            "break" => TokenKind::KwBreak,
            "true" => TokenKind::KwTrue,
            "false" => TokenKind::KwFalse,
            "instanceof" => TokenKind::KwInstanceof,
            _ => TokenKind::Ident(text),
        };

        Token { kind, line }
    }

    /// Called with the opening `"` already consumed.
    fn scan_string(&mut self, line: u32) -> Result<Token, LexError> {
        let mut value = String::new();
        loop {
            match self.chars.next() {
                None => return Err(LexError { kind: LexErrorKind::UnterminatedString, line }),
                Some('"') => return Ok(Token { kind: TokenKind::Str(value), line }),
                Some('\n') => {
                    self.line += 1;
                    value.push('\n');
                }
                Some('\\') => {
                    let resolved = self.scan_escape(line)?;
                    value.push(resolved);
                }
                Some(c) => value.push(c),
            }
        }
    }

    /// Called with the backslash already consumed.
    fn scan_escape(&mut self, line: u32) -> Result<char, LexError> {
        match self.chars.next() {
            Some('"') => Ok('"'),
            Some('\\') => Ok('\\'),
            Some('n') => Ok('\n'),
            Some('t') => Ok('\t'),
            Some('r') => Ok('\r'),
            Some('u') => self.scan_unicode_escape(line),
            Some(other) => Err(LexError { kind: LexErrorKind::InvalidEscape(other), line }),
            None => Err(LexError { kind: LexErrorKind::UnterminatedString, line }),
        }
    }

    /// Called with `\u` already consumed. Expects `{`, one or more hex digits, `}`.
    fn scan_unicode_escape(&mut self, line: u32) -> Result<char, LexError> {
        if self.chars.next() != Some('{') {
            return Err(LexError { kind: LexErrorKind::InvalidUnicodeEscape, line });
        }

        let mut hex = String::new();
        loop {
            match self.chars.next() {
                Some('}') => break,
                Some(c) if c.is_ascii_hexdigit() => hex.push(c),
                _ => return Err(LexError { kind: LexErrorKind::InvalidUnicodeEscape, line }),
            }
        }

        if hex.is_empty() {
            return Err(LexError { kind: LexErrorKind::InvalidUnicodeEscape, line });
        }

        let value = u32::from_str_radix(&hex, 16)
            .map_err(|_| LexError { kind: LexErrorKind::InvalidUnicodeEscape, line })?;

        char::from_u32(value).ok_or(LexError { kind: LexErrorKind::InvalidUnicodeEscape, line })
    }
}
```

---

## Acceptance tests

Run these through `tokenize` and check the exact result. `@N` marks the expected `line`
value on each token.

| Input | Expected tokens |
|---|---|
| `` (empty) | `Eof@1` |
| `radius = data;` | `Ident("radius")@1, Equals@1, Ident("data")@1, Semicolon@1, Eof@1` |
| `int radius;` | `KwInt@1, Ident("radius")@1, Semicolon@1, Eof@1` |
| `42` | `Num(42)@1, Eof@1` |
| `"hello"` | `Str("hello")@1, Eof@1` |
| `"a\nb"` (backslash-n escape, one source line) | `Str("a\nb" with a real newline byte inside)@1, Eof@1` |
| `// comment`, newline, `int x;` | `KwInt@2, Ident("x")@2, Semicolon@2, Eof@2` |
| `x'` | `Ident("x'")@1, Eof@1` |
| `(1 & 0)` | `LParen@1, Num(1)@1, Amp@1, Num(0)@1, RParen@1, Eof@1` |
| `"\u{48}\u{65}\u{6C}\u{6C}\u{6F}"` | `Str("Hello")@1, Eof@1` |
| `"\u{D800}"` | `Err(InvalidUnicodeEscape @ 1)` — surrogate range rejected |
| `99999999999` | `Err(IntegerLiteralOverflow("99999999999") @ 1)` |
| `"abc` (no closing quote) | `Err(UnterminatedString @ 1)` |
| `@` | `Err(UnexpectedChar('@') @ 1)` |

Every input/output pair above was traced through the algorithm by hand while writing
this plan — this isn't a "should probably work" list, each row was checked against the
actual control flow.

---

## Explicitly out of scope for this step

- The parser (consumes `Vec<Token>`, produces the AST) — separate implementation step.
- Wiring `LexError` into the course's actual error-code vocabulary (`E_...` codes) —
  that belongs to the shared error-architecture design, not the lexer in isolation.
- A CLI (`main.rs` is a stub).
