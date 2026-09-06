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

impl LexError {
    fn new(kind: LexErrorKind, line: u32) -> Self {
        LexError { kind, line }
    }
}

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

                '"' => self.scan_string(line)?,
                '0'..='9' => self.scan_number(c, line)?,
                c if c.is_ascii_alphabetic() => self.scan_ident_or_keyword(c),

                other => {
                    return Err(LexError::new(LexErrorKind::UnexpectedChar(other), line));
                }
            };

            tokens.push(Token { kind, line });
        }
    }

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
                    // Distinguish a `//` comment from a division operator by checking the next char.
                    let mut ahead = self.chars.clone();
                    ahead.next();
                    if ahead.peek() == Some(&'/') {
                        self.chars.next();
                        self.chars.next();
                        while let Some(&next) = self.chars.peek() {
                            if next == '\n' {
                                break;
                            }
                            self.chars.next();
                        }
                    } else {
                        return;
                    }
                }
                _ => return,
            }
        }
    }

    fn scan_number(&mut self, first: char, line: u32) -> Result<TokenKind, LexError> {
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
        text.parse::<i32>()
            .map(TokenKind::Num)
            .map_err(|_| LexError::new(LexErrorKind::IntegerLiteralOverflow(text), line))
    }

    fn scan_ident_or_keyword(&mut self, first: char) -> TokenKind {
        let mut text = String::new();
        text.push(first);
        while let Some(&c) = self.chars.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                text.push(c);
                self.chars.next();
            } else {
                break;
            }
        }

        match text.as_str() {
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
        }
    }

    // Called with the opening `"` already consumed.
    fn scan_string(&mut self, line: u32) -> Result<TokenKind, LexError> {
        let mut value = String::new();
        loop {
            match self.chars.next() {
                None | Some('\n') => {
                    return Err(LexError::new(LexErrorKind::UnterminatedString, line))
                }
                Some('"') => return Ok(TokenKind::Str(value)),
                Some('\\') => {
                    let resolved = self.scan_escape(line)?;
                    value.push(resolved);
                }
                Some(c) => value.push(c),
            }
        }
    }

    // Called with the backslash already consumed.
    fn scan_escape(&mut self, line: u32) -> Result<char, LexError> {
        match self.chars.next() {
            Some('"') => Ok('"'),
            Some('\\') => Ok('\\'),
            Some('n') => Ok('\n'),
            Some('t') => Ok('\t'),
            Some('r') => Ok('\r'),
            Some('u') => self.scan_unicode_escape(line),
            Some(other) => Err(LexError::new(LexErrorKind::InvalidEscape(other), line)),
            None => Err(LexError::new(LexErrorKind::UnterminatedString, line)),
        }
    }

    // Called with `\u` already consumed. Expects `{`, one or more hex digits, `}`.
    fn scan_unicode_escape(&mut self, line: u32) -> Result<char, LexError> {
        if self.chars.next() != Some('{') {
            return Err(LexError::new(LexErrorKind::InvalidUnicodeEscape, line));
        }

        let mut hex = String::new();
        loop {
            match self.chars.next() {
                Some('}') => break,
                Some(c) if c.is_ascii_hexdigit() => hex.push(c),
                _ => return Err(LexError::new(LexErrorKind::InvalidUnicodeEscape, line)),
            }
        }

        if hex.is_empty() {
            return Err(LexError::new(LexErrorKind::InvalidUnicodeEscape, line));
        }

        let value = u32::from_str_radix(&hex, 16)
            .map_err(|_| LexError::new(LexErrorKind::InvalidUnicodeEscape, line))?;

        char::from_u32(value).ok_or(LexError::new(LexErrorKind::InvalidUnicodeEscape, line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::TokenKind::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        tokenize(source)
            .unwrap_or_else(|e| panic!("unexpected lex error on {source:?}: {e:?}"))
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    fn lines(source: &str) -> Vec<u32> {
        tokenize(source)
            .unwrap_or_else(|e| panic!("unexpected lex error on {source:?}: {e:?}"))
            .into_iter()
            .map(|t| t.line)
            .collect()
    }

    #[test]
    fn empty_input_is_just_eof() {
        assert_eq!(kinds(""), vec![Eof]);
        assert_eq!(lines(""), vec![1]);
    }

    #[test]
    fn assignment_statement() {
        assert_eq!(
            kinds("radius = data;"),
            vec![
                Ident("radius".into()),
                Equals,
                Ident("data".into()),
                Semicolon,
                Eof
            ]
        );
    }

    #[test]
    fn declaration_statement() {
        assert_eq!(
            kinds("int radius;"),
            vec![KwInt, Ident("radius".into()), Semicolon, Eof]
        );
    }

    #[test]
    fn integer_literal() {
        assert_eq!(kinds("42"), vec![Num(42), Eof]);
    }

    #[test]
    fn string_literal() {
        assert_eq!(kinds("\"hello\""), vec![Str("hello".into()), Eof]);
    }

    #[test]
    fn string_with_newline_escape_stays_on_one_line() {
        let toks = tokenize("\"a\\nb\"").unwrap();
        assert_eq!(toks[0].kind, Str("a\nb".into()));
        assert_eq!(toks[0].line, 1);
        assert_eq!(toks[1].kind, Eof);
    }

    #[test]
    fn line_comment_then_next_line() {
        let src = "// comment\nint x;";
        assert_eq!(kinds(src), vec![KwInt, Ident("x".into()), Semicolon, Eof]);
        assert_eq!(lines(src), vec![2, 2, 2, 2]);
    }

    #[test]
    fn apostrophe_is_not_part_of_an_identifier() {
        let err = tokenize("x'").unwrap_err();
        assert_eq!(
            err,
            LexError {
                kind: LexErrorKind::UnexpectedChar('\''),
                line: 1
            }
        );
    }

    #[test]
    fn single_char_bitwise_and() {
        assert_eq!(
            kinds("(1 & 0)"),
            vec![LParen, Num(1), Amp, Num(0), RParen, Eof]
        );
    }

    #[test]
    fn unicode_escape_hello() {
        assert_eq!(
            kinds("\"\\u{48}\\u{65}\\u{6C}\\u{6C}\\u{6F}\""),
            vec![Str("Hello".into()), Eof]
        );
    }

    #[test]
    fn unicode_escape_surrogate_is_rejected() {
        let err = tokenize("\"\\u{D800}\"").unwrap_err();
        assert_eq!(
            err,
            LexError {
                kind: LexErrorKind::InvalidUnicodeEscape,
                line: 1
            }
        );
    }

    #[test]
    fn integer_literal_overflow() {
        let err = tokenize("99999999999").unwrap_err();
        assert_eq!(
            err,
            LexError {
                kind: LexErrorKind::IntegerLiteralOverflow("99999999999".into()),
                line: 1
            }
        );
    }

    #[test]
    fn unterminated_string() {
        let err = tokenize("\"abc").unwrap_err();
        assert_eq!(
            err,
            LexError {
                kind: LexErrorKind::UnterminatedString,
                line: 1
            }
        );
    }

    #[test]
    fn unexpected_char() {
        let err = tokenize("@").unwrap_err();
        assert_eq!(
            err,
            LexError {
                kind: LexErrorKind::UnexpectedChar('@'),
                line: 1
            }
        );
    }

    #[test]
    fn all_keywords_recognized() {
        let src = "int bool String void class extends this super null new \
                    return if else while break true false instanceof";
        assert_eq!(
            kinds(src),
            vec![
                KwInt,
                KwBool,
                KwString,
                KwVoid,
                KwClass,
                KwExtends,
                KwThis,
                KwSuper,
                KwNull,
                KwNew,
                KwReturn,
                KwIf,
                KwElse,
                KwWhile,
                KwBreak,
                KwTrue,
                KwFalse,
                KwInstanceof,
                Eof,
            ]
        );
    }

    #[test]
    fn reserved_names_lex_as_plain_idents() {
        for name in ["in", "out", "err", "Main", "Input", "Output"] {
            assert_eq!(kinds(name), vec![Ident(name.into()), Eof], "for {name}");
        }
    }

    #[test]
    fn all_single_char_punctuation() {
        assert_eq!(
            kinds("(){}[];,.?:=+-*/%&|<>~!"),
            vec![
                LParen, RParen, LBrace, RBrace, LBracket, RBracket, Semicolon, Comma, Dot,
                Question, Colon, Equals, Plus, Minus, Star, Slash, Percent, Amp, Pipe, Lt, Gt,
                Tilde, Bang, Eof,
            ]
        );
    }

    #[test]
    fn no_double_char_operators_exist() {
        assert_eq!(kinds("=="), vec![Equals, Equals, Eof]);
        assert_eq!(kinds("&&"), vec![Amp, Amp, Eof]);
        assert_eq!(kinds("||"), vec![Pipe, Pipe, Eof]);
    }

    #[test]
    fn division_operator_not_confused_with_comment() {
        assert_eq!(
            kinds("a / b"),
            vec![Ident("a".into()), Slash, Ident("b".into()), Eof]
        );
    }

    #[test]
    fn line_comment_at_end_of_file_with_no_trailing_newline() {
        assert_eq!(
            kinds("int x; // trailing"),
            vec![KwInt, Ident("x".into()), Semicolon, Eof]
        );
    }

    #[test]
    fn slash_star_is_not_a_block_comment() {
        assert_eq!(
            kinds("/* not a comment */"),
            vec![
                Slash,
                Star,
                Ident("not".into()),
                Ident("a".into()),
                Ident("comment".into()),
                Star,
                Slash,
                Eof
            ]
        );
    }

    #[test]
    fn line_numbers_advance_across_blank_lines_and_comments() {
        let src = "int a;\n\n// comment\nint b;";
        assert_eq!(lines(src), vec![1, 1, 1, 4, 4, 4, 4]);
    }

    #[test]
    fn raw_newline_inside_string_is_unterminated() {
        let err = tokenize("\"line1\nline2\"").unwrap_err();
        assert_eq!(
            err,
            LexError {
                kind: LexErrorKind::UnterminatedString,
                line: 1
            }
        );
    }

    #[test]
    fn all_basic_escapes() {
        assert_eq!(
            kinds(r#""\"\\\n\t\r""#),
            vec![Str("\"\\\n\t\r".into()), Eof]
        );
    }

    #[test]
    fn invalid_escape_char() {
        let err = tokenize("\"\\q\"").unwrap_err();
        assert_eq!(
            err,
            LexError {
                kind: LexErrorKind::InvalidEscape('q'),
                line: 1
            }
        );
    }

    #[test]
    fn unicode_escape_above_max_scalar_is_rejected() {
        let err = tokenize("\"\\u{110000}\"").unwrap_err();
        assert_eq!(
            err,
            LexError {
                kind: LexErrorKind::InvalidUnicodeEscape,
                line: 1
            }
        );
    }

    #[test]
    fn unicode_escape_missing_brace_forms_rejected() {
        assert_eq!(
            tokenize("\"\\u48\"").unwrap_err().kind,
            LexErrorKind::InvalidUnicodeEscape
        );
        assert_eq!(
            tokenize("\"\\u{48\"").unwrap_err().kind,
            LexErrorKind::InvalidUnicodeEscape
        );
        assert_eq!(
            tokenize("\"\\u{}\"").unwrap_err().kind,
            LexErrorKind::InvalidUnicodeEscape
        );
    }

    #[test]
    fn unterminated_string_via_escape_at_eof() {
        let err = tokenize("\"abc\\").unwrap_err();
        assert_eq!(
            err,
            LexError {
                kind: LexErrorKind::UnterminatedString,
                line: 1
            }
        );
    }

    #[test]
    fn max_i32_literal_accepted_but_one_more_overflows() {
        assert_eq!(kinds("2147483647"), vec![Num(2147483647), Eof]);
        assert!(tokenize("2147483648").is_err());
    }

    #[test]
    fn identifier_with_digits_and_underscore() {
        assert_eq!(kinds("a1_b2"), vec![Ident("a1_b2".into()), Eof]);
    }

    #[test]
    fn string_keyword_vs_string_ident_case_sensitive() {
        assert_eq!(kinds("String"), vec![KwString, Eof]);
        assert_eq!(kinds("string"), vec![Ident("string".into()), Eof]);
    }

    #[test]
    fn full_class_snippet() {
        let src = "class Foo extends Bar {\n    int x;\n}\n";
        assert_eq!(
            kinds(src),
            vec![
                KwClass,
                Ident("Foo".into()),
                KwExtends,
                Ident("Bar".into()),
                LBrace,
                KwInt,
                Ident("x".into()),
                Semicolon,
                RBrace,
                Eof,
            ]
        );
    }
}
