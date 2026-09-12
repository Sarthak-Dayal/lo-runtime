# Lexer Design — LO (LiveOak) P1

Scope: source text in, `Vec<Token>` out. Parsing is a separate document.

---

## Decisions

| Decision | Why |
|---|---|
| Tokenize the whole file up front into `Vec<Token>` | Parser needs 2 tokens of lookahead (LL(2)). A vector + cursor gives free `peek`/`peek2`. No streaming buffer to hand-build. |
| Hand-written scanner, no lexer-generator crate | LO's lexical grammar is small. Course requires the parser to be hand-built ("consult the grammar, not your parser generator's escape hatches"); same standard applied to the lexer. Every team member has to defend it at the whiteboard (viva voce). |
| Single-character lookahead (`Peekable<Chars>`) | LO has no multi-character operators. Nothing in the grammar needs more than 1 char of peek. |
| Decode string escapes during scanning, not later | `\u{...}` range validation (reject surrogates and codepoints above U+10FFFF) is a lex-phase compile error per spec. Has to happen while the string is being scanned. |
| `Token { kind, line }` — one `line` field on the wrapper, not one per `TokenKind` variant | Every token needs exactly one line number. No variant-specific variation to justify repeating the field. |
| Track line number only, no column | No document states a column policy for P1 diagnostics specifically. The "line, not column" statement in the source material is in the P4/DWARF chapter's discussion of debugger-stepping granularity: *"column tracking was considered and rejected as cost without P4 payoff."* That's good evidence for line-only being sufficient even beyond P1's own error messages — the course already checked whether the debugger project would need column info and decided no. |
| `in`, `out`, `err`, `Main`, `Input`, `Output` are NOT lexer keywords | They never appear as literal tokens in the LO-4 grammar. They lex as plain `Ident` and get flagged as reserved later, during name resolution. |

---

## Token shape

```rust
struct Token {
    kind: TokenKind,
    line: u32,
}

enum TokenKind {
    Ident(String),    // name text, not yet known to be a var/field/class/method
    Num(i32),          // integer literal, already parsed to a value
    Str(String),       // string literal, escapes already resolved, quotes stripped

    // keywords — unit variants, one per LO-4 keyword (see table below)
    KwInt, KwBool, KwString, KwVoid,
    KwClass, KwExtends, KwThis, KwSuper, KwNull, KwNew,
    KwReturn, KwIf, KwElse, KwWhile, KwBreak,
    KwTrue, KwFalse, KwInstanceof,

    // punctuation / operators — all single character
    LParen, RParen, LBrace, RBrace, LBracket, RBracket,
    Semicolon, Comma, Dot, Question, Colon, Equals,
    Plus, Minus, Star, Slash, Percent, Amp, Pipe, Lt, Gt, Tilde, Bang,

    Eof,
}
```

**What each part is:**

- `kind` — which of the categories below this token is.
- `line` — source line the token started on. Used for error messages only.
- `Ident(String)` — any name the programmer wrote that isn't a keyword: a variable, a
  field, a class, a method. The lexer does not know which. That gets decided later, by
  where the parser finds the name in the grammar, then by the type checker.
- `Num(i32)` — an integer literal. The text `"42"` becomes the value `42` here, already
  converted, not kept as digit characters.
- `Str(String)` — a string literal. Quotes are stripped, escapes are resolved, so the
  stored value is exactly what the LO program sees at runtime.
- Keyword variants — one fixed variant per LO-4 keyword. No payload; the source text
  that produces `KwIf` is always exactly `if`, so there's nothing to store.
- Punctuation/operator variants — one per symbol. No payload, same reason.
- `Eof` — end of input, emitted once at the end of the vector. Lets `peek`/`peek2` near
  the end of the file return a real token instead of `None`.

The `String` inside `Ident`/`Str` is Rust's built-in string type, unrelated to LO's own
`Type::String`.

---

## Keyword table

Recognized only after scanning the full identifier text (maximal munch), then checked
against this table. No match → emit `Ident` instead.

| Source text | Token |
|---|---|
| `int` | `KwInt` |
| `bool` | `KwBool` |
| `String` | `KwString` |
| `void` | `KwVoid` |
| `class` | `KwClass` |
| `extends` | `KwExtends` |
| `this` | `KwThis` |
| `super` | `KwSuper` |
| `null` | `KwNull` |
| `new` | `KwNew` |
| `return` | `KwReturn` |
| `if` | `KwIf` |
| `else` | `KwElse` |
| `while` | `KwWhile` |
| `break` | `KwBreak` |
| `true` | `KwTrue` |
| `false` | `KwFalse` |
| `instanceof` | `KwInstanceof` |

## Reserved names — NOT keywords

Lex as ordinary `Ident`. `in`/`out`/`err` are enforced as reserved at name resolution
via `E_RESERVED_VARIABLE_NAME`; `Input`/`Output` via `E_RESERVED_CLASS_NAME`. `Main` is
the odd one out: `error-codes.md` is explicit that *"`Main` is permitted but must
satisfy the entry-point shape"* — `Main` is exempt from `E_RESERVED_CLASS_NAME`
entirely and is checked only by its own separate entry-point codes (`E_NO_MAIN_CLASS`,
`E_MAIN_CLASS_EXTENDS`, `E_NO_MAIN_METHOD`, `E_MAIN_METHOD_SIGNATURE`,
`E_MAIN_NO_ZERO_ARG_CONSTRUCTOR`). None of this changes what the lexer does — `Main`
still just lexes as `Ident` — only which check code applies later, at name resolution.

| Name | Role | Checked via |
|---|---|---|
| `in` | preamble binding, type `Input` | `E_RESERVED_VARIABLE_NAME` |
| `out` | preamble binding, type `Output` | `E_RESERVED_VARIABLE_NAME` |
| `err` | preamble binding, type `Output` | `E_RESERVED_VARIABLE_NAME` |
| `Main` | required entry-point class name | entry-point codes, NOT `E_RESERVED_CLASS_NAME` |
| `Input` | synthesized preamble class | `E_RESERVED_CLASS_NAME` |
| `Output` | synthesized preamble class | `E_RESERVED_CLASS_NAME` |

Side effect of `String` being a keyword and not an identifier: `class C extends String`
fails to parse (`String` can't fill the `ClassName` slot), so "can a user extend String"
is answered by the lexer/parser, not a check the type checker has to write.

---

## Operator/literal facts that shape the token set

- No `==`. `=` is the only equals token. It means assignment in a `Stmt` and equality in
  a `Binop`; the parser tells these apart by grammar position, not the lexer.
- No `&&` / `||`. `&` and `|` are single characters (short-circuit AND / inclusive OR).
  LO has no doubled operators anywhere.
- Identifiers allow a literal apostrophe after the first character:
  `[a-zA-Z]([a-zA-Z0-9'_])*`.
- `\u{H...}` escapes: use `std::char::from_u32(u32) -> Option<char>` to validate.
  `None` on exactly the invalid cases the spec names (above U+10FFFF, or in the
  surrogate range U+D800–DFFF) — no need to hand-write the range check.
- `//` line comments confirmed (used throughout the conformance-suite examples).
  Block comments (`/* */`) not confirmed either way.
- Line counter must advance across whitespace and comments, not just token characters.

---

## Open items

- Block comments (`/* */`): confirm they exist or don't before assuming either way.
- Integer literal overflow: grammar allows unbounded digit strings, `int` is 32-bit.
  No clearly-named error code for this case (closest is the catch-all
  `E_PARSE_PHASE_OTHER`). Decide the behavior, write a test for it.
- Error recovery: assumed fail-fast (stop at first bad character). No document
  supports this for any phase (see `parser_design.md`'s Decisions table and Open
  Item 1) — a team assumption adopted for implementation simplicity across lexer,
  parser, and checker alike; worth a direct question to course staff before treating
  it as settled.

---

## Alternate designs considered, not chosen

| Option | Rejected because |
|---|---|
| Lazy/streaming lexer (produce tokens on demand from an iterator) | LL(2) still needs a 2-token lookahead buffer built on top of it. A pre-built `Vec<Token>` gives that for free with less code. File sizes here don't justify avoiding the upfront allocation. |
| Lexer-generator crate (e.g. `logos`) | Course's stated ethos for the parser ("consult the grammar, not your parser generator's escape hatches") reads as build-it-yourself for the whole front end. Harder to defend at the whiteboard for viva voce. LO's lexical grammar is small enough that a generator saves little. |
| `line: u32` on every `TokenKind` variant, matching the AST's per-node style | Every token needs exactly one line number — no variant-to-variant variation to justify the repetition. Wrapper struct is less code, same information. |
| Defer string-escape decoding to the parser or AST-construction step | `\u{...}` validation is a lex-phase compile error. Decoding has to happen while the string literal is being scanned, so there's nothing left to defer. |
| Treat `in`/`out`/`err`/`Main`/`Input`/`Output` as lexer keywords | They are not part of the grammar's token vocabulary — they never appear as literal tokens in any LO-4 production. Making them keywords would be lexing something the grammar doesn't define. Their reserved status belongs to name resolution. |
