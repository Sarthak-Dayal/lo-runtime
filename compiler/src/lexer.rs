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
                    tokens.push(Token {
                        kind: TokenKind::Eof,
                        line,
                    });
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
                    return Err(LexError {
                        kind: LexErrorKind::UnexpectedChar(other),
                        line,
                    });
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
            Ok(n) => Ok(Token {
                kind: TokenKind::Num(n),
                line,
            }),
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
                None => {
                    return Err(LexError {
                        kind: LexErrorKind::UnterminatedString,
                        line,
                    })
                }
                Some('"') => {
                    return Ok(Token {
                        kind: TokenKind::Str(value),
                        line,
                    })
                }
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
            Some(other) => Err(LexError {
                kind: LexErrorKind::InvalidEscape(other),
                line,
            }),
            None => Err(LexError {
                kind: LexErrorKind::UnterminatedString,
                line,
            }),
        }
    }

    /// Called with `\u` already consumed. Expects `{`, one or more hex digits, `}`.
    fn scan_unicode_escape(&mut self, line: u32) -> Result<char, LexError> {
        if self.chars.next() != Some('{') {
            return Err(LexError {
                kind: LexErrorKind::InvalidUnicodeEscape,
                line,
            });
        }

        let mut hex = String::new();
        loop {
            match self.chars.next() {
                Some('}') => break,
                Some(c) if c.is_ascii_hexdigit() => hex.push(c),
                _ => {
                    return Err(LexError {
                        kind: LexErrorKind::InvalidUnicodeEscape,
                        line,
                    })
                }
            }
        }

        if hex.is_empty() {
            return Err(LexError {
                kind: LexErrorKind::InvalidUnicodeEscape,
                line,
            });
        }

        let value = u32::from_str_radix(&hex, 16).map_err(|_| LexError {
            kind: LexErrorKind::InvalidUnicodeEscape,
            line,
        })?;

        char::from_u32(value).ok_or(LexError {
            kind: LexErrorKind::InvalidUnicodeEscape,
            line,
        })
    }
}
