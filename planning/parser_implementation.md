# Parser Implementation Plan — LO (LiveOak) P1

Implements everything decided in `parser_design.md`. Assumes the same crate as
`lexer_implementation.md`: `/Users/jay/Documents/College/Fall2026/PL/P1/lo-compiler/`,
package `lo-compiler`, edition 2021, no external dependencies. `src/token.rs` and
`src/lexer.rs` already exist per that plan; this adds the AST and the parser.

---

## Files to create / modify

| File | Contents |
|---|---|
| `src/ast.rs` | new — every AST type from `parser_design.md`'s "AST shape" section |
| `src/parser.rs` | new — `ParseError`, `ErrorCode`, `parse_program`, and every `parse_*` function |
| `src/add_io_classes.rs` | new — `add_io_classes`, per `parser_design.md`'s "`add_io_classes`" section |
| `src/main.rs` | modified — add `mod ast; mod parser; mod add_io_classes;` |

---

## Decisions resolved for this implementation

These weren't fully pinned down at the design level — concrete choices made here so there's nothing left for an implementer to guess.

| Item | Decision |
|---|---|
| Threaded parsing state (enclosing class name, `BodyScope` locals handle, constructor call-keyword tracking) | One `ParseContext` struct (defined below), passed as `&mut ParseContext` through `parse_stmt`/`parse_nested_block`/`parse_var_decl`. Never a mutable field on `Parser` itself: all three pieces of state (class name, locals handle, in-constructor flag) are scoped per method/constructor body and must never leak across method boundaries — a `Parser`-level field would persist across the sibling `parse_class_decl`/`parse_method_decl` calls that reuse the same `Parser`, with nothing forcing a reset between them. Bundling them into one context type built fresh per body and threaded only through that body's recursive calls makes the scoping structurally enforced instead of reset-by-convention. |
| How nested `if`/`while` blocks get a "not the constructor's top level anymore" context without losing the shared `locals` handle | Rust doesn't let you move a `&mut` field out of a borrowed struct, so `parse_nested_block` builds a fresh `ParseContext` for the nested call using `locals: &mut *ctx.locals` (an explicit reborrow of the same underlying `Vec`, not a new one) and `at_constructor_top_level: false`. This is the one real Rust-ownership subtlety in this whole plan — get this specific line right or duplicate-local detection across sibling/nested blocks silently breaks. |
| `E_DUPLICATE_FIELD` — parser or checker? | Checker. `error-codes.md` files it under well-formedness, not parse-phase (same principle as `E_LOCAL_SHADOWS_FORMAL`/`E_BREAK_OUTSIDE_LOOP` in `parser_design.md`). `parse_field_list` flattens `VarDecl`s into `Vec<Param>` with **no** duplicate check — that happens later, in code that doesn't exist yet. |
| `E_SUPER_IN_ROOT_CLASS` — parser or checker? | Checker. Filed under inheritance-check in `error-codes.md`, not parse-phase, even though the parser has the info (`extends: Option<String>`) locally available. `parse_other_constructor_call` records whichever call keyword appears without judging whether it's legal given the class's inheritance status. |
| Error code for a single generic mis-expected token (missing `;`, missing `)`, etc.) | `ErrorCode::EParsePhaseOther` — the catch-all everywhere outside a `ClassDecl`'s own structure. `EMalformedConstructor`, `EDelegationBothSuperAndThis`, `EDelegationNotFirstStatement`, `EDuplicateLocal`, `EReservedKeywordAsIdentifier` are each used only at the exact point `parser_design.md` names for them. `EMalformedClassDecl` is broader: every `expect()` inside `parse_class_decl` for a `ClassDecl`'s own structural tokens (the field parens, the `{`/`}` around methods) uses it too, not just the two triggers `parser_design.md` names explicitly ("wrong order," "empty `[ ]` brackets") — any structural failure while parsing a `ClassDecl` is still a malformed class declaration. |
| How `parse_paren_expr` decides whether it might be looking at a cast | A dedicated lookahead-only check (`try_peek_type_in_parens`, three tokens of pure peeking, no consumption) confirming the exact shape `(Type)` — not the weaker "is the next token also `(`". The weaker check is insufficient: it's equally true when the outer paren just contains an ordinary nested expression that happens to start with `(`, e.g. a cast one level down (`((Animal)((Dog)obj))`) or a cast combined with `instanceof` (`(((Dog)x) instanceof Animal)`). With the precise gate, everything that isn't confirmed as `(Type)` falls through to one ordinary recursive `parse_expr()` call, which correctly resolves nested `(`-led expressions — including nested casts — entirely on its own, at any depth, before the outer level ever looks at what follows. |
| `expect_ident_name` on a non-identifier token | Splits into two cases: if the actual token is one of the 18 keywords, `EReservedKeywordAsIdentifier`; anything else (punctuation, a literal, `Eof`), `EParsePhaseOther`. Only the keyword case is what `E_RESERVED_KEYWORD_AS_IDENTIFIER`'s trigger text describes. |
| How a `(`-led receiver (`Receiver::Computed`) and a `(`-led `Expr` resolve their parenthesized content | One shared core, `parse_paren_value` — both a statement-position receiver (`((Cat) a).purr();`) and an expression-position value followed by a call (`(new Circle(5)).area()`) are, per decision point 4, the same `(`-led fragment in different positions. `parse_paren_expr` (`Expr` position) calls `parse_paren_value` and then checks for a trailing `.` to build a `Receiver::Computed`/`MethodCall`; `parse_receiver`'s `(` arm calls the same `parse_paren_value` and always wraps the result as `Receiver::Computed`, since `parse_call_stmt` already requires the trailing `.MethodName(...)` immediately after. A receiver-only reimplementation of "`(`, parse one `Expr`, `)`" would not run the unop/cast-vs-nested-paren logic at the receiver's own recursion depth, breaking exactly the cases (like `((Cat) a).purr();`) where the parenthesized content itself needs that disambiguation. |

---

## `src/ast.rs`

```rust
#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Int,
    Bool,
    String,
    Void,
    Class(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub classes: Vec<ClassDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub declared_type: Type,
    pub name: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassDecl {
    pub name: String,
    pub extends: Option<String>,
    pub fields: Vec<Param>,
    pub constructors: Vec<ConstructorDecl>,
    pub methods: Vec<MethodDecl>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstructorDecl {
    pub formals: Vec<Param>,
    pub other_constructor_call: Option<OtherConstructorCall>,
    pub body: BodyScope,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodDecl {
    pub ret: Type,
    pub name: String,
    pub formals: Vec<Param>,
    pub body: MethodBody,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OtherConstructorCall {
    ThisCall(Vec<Expr>, u32),
    SuperCall(Vec<Expr>, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MethodBody {
    User(BodyScope),
    Io(IoOp),
}

#[derive(Debug, Clone, PartialEq)]
pub enum IoOp {
    ReadInt, ReadBool, ReadString, Eof,
    PrintInt, PrintBool, PrintString, Println,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub declared_type: Type,
    pub names: Vec<String>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BodyScope {
    pub locals: Vec<VarDecl>,
    pub stmts: Vec<Stmt>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Assign(String, Expr, u32),
    Return(Expr, u32),
    If(Expr, Vec<Stmt>, Vec<Stmt>, u32),
    While(Expr, Vec<Stmt>, u32),
    Break(u32),
    Empty(u32),
    CallStmt(MethodCall),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodCall {
    pub receiver: Receiver,
    pub name: String,
    pub args: Vec<Expr>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(i32, u32),
    Bool(bool, u32),
    Str(String, u32),
    Var(String, u32),
    This(u32),
    Null(u32),
    New(String, Vec<Expr>, u32),
    Call(MethodCall),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>, u32),
    Bin(Box<Expr>, BinOp, Box<Expr>, u32),
    Un(UnOp, Box<Expr>, u32),
    Cast(Type, Box<Expr>, u32),
    InstanceOf(Box<Expr>, String, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Receiver {
    Var(String, u32),
    This(u32),
    Super(u32),
    Computed(Box<Expr>, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp { Add, Sub, Mul, Div, Mod, And, Or, Lt, Gt, Eq }

#[derive(Debug, Clone, PartialEq)]
pub enum UnOp { Not, Neg }
```

---

## `src/parser.rs`

```rust
use crate::ast::*;
use crate::token::{Token, TokenKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub code: ErrorCode,
    pub line: u32,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ErrorCode {
    EReservedKeywordAsIdentifier,
    EMalformedClassDecl,
    EMalformedConstructor,
    EDelegationBothSuperAndThis,
    EDelegationNotFirstStatement,
    EDuplicateLocal,
    EParsePhaseOther,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::EReservedKeywordAsIdentifier => "E_RESERVED_KEYWORD_AS_IDENTIFIER",
            ErrorCode::EMalformedClassDecl => "E_MALFORMED_CLASS_DECL",
            ErrorCode::EMalformedConstructor => "E_MALFORMED_CONSTRUCTOR",
            ErrorCode::EDelegationBothSuperAndThis => "E_DELEGATION_BOTH_SUPER_AND_THIS",
            ErrorCode::EDelegationNotFirstStatement => "E_DELEGATION_NOT_FIRST_STATEMENT",
            ErrorCode::EDuplicateLocal => "E_DUPLICATE_LOCAL",
            ErrorCode::EParsePhaseOther => "E_PARSE_PHASE_OTHER",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CallKeyword {
    This,
    Super,
}

/// Threaded through parse_stmt / parse_nested_block / parse_var_decl. Built fresh for
/// each method/constructor body by parse_method_body_scope / parse_constructor_body_scope.
struct ParseContext<'p> {
    class_name: &'p str,
    locals: &'p mut Vec<VarDecl>,
    in_constructor: bool,
    /// True only while parsing the constructor's own top-level Stmt list. Set to
    /// false when recursing into any if/while block, at any depth, and never set
    /// back to true again for that subtree — see parse_nested_block.
    at_constructor_top_level: bool,
    /// Whichever call keyword parse_other_constructor_call recorded at the
    /// very start of the constructor body, if any. Read-only from here on.
    recorded_call_keyword: Option<CallKeyword>,
}

pub fn parse_program(tokens: &[Token]) -> Result<Program, ParseError> {
    let mut parser = Parser { tokens, pos: 0 };
    let mut classes = Vec::new();
    while !parser.check(&TokenKind::Eof) {
        classes.push(parser.parse_class_decl()?);
    }
    Ok(Program { classes })
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    // ---- core primitives ----

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    /// Safe near the end of input: Eof is always the last token, and pos never
    /// advances past it, so this never actually indexes out of bounds in practice —
    /// the fallback only matters if that invariant is ever violated.
    fn peek2(&self) -> &Token {
        self.tokens.get(self.pos + 1).unwrap_or_else(|| self.tokens.last().unwrap())
    }

    fn check(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, kind: TokenKind, code: ErrorCode) -> Result<Token, ParseError> {
        if self.check(&kind) {
            Ok(self.advance())
        } else {
            Err(ParseError {
                code,
                line: self.peek().line,
                message: format!("expected {:?}, found {:?}", kind, self.peek().kind),
            })
        }
    }

    fn expect_ident_name(&mut self) -> Result<String, ParseError> {
        let line = self.peek().line;
        match self.peek().kind.clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Ok(name)
            }
            kind if is_keyword(&kind) => Err(ParseError {
                code: ErrorCode::EReservedKeywordAsIdentifier,
                line,
                message: format!("expected an identifier, found reserved keyword {:?}", kind),
            }),
            other => Err(ParseError {
                code: ErrorCode::EParsePhaseOther,
                line,
                message: format!("expected an identifier, found {:?}", other),
            }),
        }
    }

    fn binop_for(kind: &TokenKind) -> Option<BinOp> {
        match kind {
            TokenKind::Plus => Some(BinOp::Add),
            TokenKind::Minus => Some(BinOp::Sub),
            TokenKind::Star => Some(BinOp::Mul),
            TokenKind::Slash => Some(BinOp::Div),
            TokenKind::Percent => Some(BinOp::Mod),
            TokenKind::Amp => Some(BinOp::And),
            TokenKind::Pipe => Some(BinOp::Or),
            TokenKind::Lt => Some(BinOp::Lt),
            TokenKind::Gt => Some(BinOp::Gt),
            TokenKind::Equals => Some(BinOp::Eq),
            _ => None,
        }
    }

    fn try_parse_primitive_type(&mut self) -> Option<Type> {
        let ty = match &self.peek().kind {
            TokenKind::KwInt => Type::Int,
            TokenKind::KwBool => Type::Bool,
            TokenKind::KwString => Type::String,
            TokenKind::KwVoid => Type::Void,
            _ => return None,
        };
        self.advance();
        Some(ty)
    }

    fn parse_type(&mut self) -> Result<Type, ParseError> {
        if let Some(ty) = self.try_parse_primitive_type() {
            return Ok(ty);
        }
        let name = self.expect_ident_name()?;
        Ok(Type::Class(name))
    }

    /// Decision point 1: `Ident` then another `Ident` means a VarDecl continues.
    fn starts_var_decl(&self) -> bool {
        match &self.peek().kind {
            TokenKind::KwInt | TokenKind::KwBool | TokenKind::KwString | TokenKind::KwVoid => true,
            TokenKind::Ident(_) => matches!(self.peek2().kind, TokenKind::Ident(_)),
            _ => false,
        }
    }

    // ---- Program / ClassDecl ----

    fn parse_class_decl(&mut self) -> Result<ClassDecl, ParseError> {
        let line = self.peek().line;
        self.expect(TokenKind::KwClass, ErrorCode::EParsePhaseOther)?;
        let name = self.expect_ident_name()?;

        let extends = if self.check(&TokenKind::KwExtends) {
            self.advance();
            Some(self.expect_ident_name()?)
        } else {
            None
        };

        self.expect(TokenKind::LParen, ErrorCode::EMalformedClassDecl)?;
        let fields = self.parse_field_list()?;
        self.expect(TokenKind::RParen, ErrorCode::EMalformedClassDecl)?;

        let constructors = if self.check(&TokenKind::LBracket) {
            self.advance();
            if self.check(&TokenKind::RBracket) {
                return Err(ParseError {
                    code: ErrorCode::EMalformedClassDecl,
                    line: self.peek().line,
                    message: "empty [ ] constructor section".into(),
                });
            }
            let mut ctors = Vec::new();
            while !self.check(&TokenKind::RBracket) {
                ctors.push(self.parse_constructor_decl(&name)?);
            }
            self.advance(); // ]
            ctors
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LBrace, ErrorCode::EMalformedClassDecl)?;
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            methods.push(self.parse_method_decl(&name)?);
        }
        self.advance(); // }

        // A `[ ]` section here means it was written after `{ methods }` instead of
        // before it — the three sections are position-fixed (fields, then
        // constructors, then methods), so this is the "wrong order" case
        // E_MALFORMED_CLASS_DECL's trigger text names explicitly, not a fresh
        // top-level `class` to hand back to parse_program.
        if self.check(&TokenKind::LBracket) {
            return Err(ParseError {
                code: ErrorCode::EMalformedClassDecl,
                line: self.peek().line,
                message: "constructor [ ] section must come before the method body, not after".into(),
            });
        }

        Ok(ClassDecl { name, extends, fields, constructors, methods, line })
    }

    /// A class's field-parens section is VarDecl*, not Formals — grouped names
    /// (`int x, y;`) are legal and must be flattened one Param per name. No
    /// duplicate-name check here: E_DUPLICATE_FIELD is checker work.
    fn parse_field_list(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RParen) {
            let line = self.peek().line;
            let declared_type = self.parse_type()?;
            let mut names = vec![self.expect_ident_name()?];
            while self.check(&TokenKind::Comma) {
                self.advance();
                names.push(self.expect_ident_name()?);
            }
            self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?;
            for name in names {
                fields.push(Param { declared_type: declared_type.clone(), name, line });
            }
        }
        Ok(fields)
    }

    // ---- ConstructorDecl / MethodDecl ----

    fn parse_constructor_decl(&mut self, class_name: &str) -> Result<ConstructorDecl, ParseError> {
        let line = self.peek().line;
        let ctor_name = self.expect_ident_name()?;
        if ctor_name != class_name {
            return Err(ParseError {
                code: ErrorCode::EMalformedConstructor,
                line,
                message: format!(
                    "constructor name '{}' does not match class name '{}'",
                    ctor_name, class_name
                ),
            });
        }
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?;
        let formals = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_formals()?
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;

        let (other_constructor_call, body) = self.parse_constructor_body_scope(class_name)?;
        Ok(ConstructorDecl { formals, other_constructor_call, body, line })
    }

    fn parse_method_decl(&mut self, class_name: &str) -> Result<MethodDecl, ParseError> {
        let line = self.peek().line;
        let ret = self.parse_type()?;
        let name = self.expect_ident_name()?;
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?;
        let formals = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_formals()?
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        let body_scope = self.parse_method_body_scope(class_name)?;
        Ok(MethodDecl { ret, name, formals, body: MethodBody::User(body_scope), line })
    }

    fn parse_formals(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut formals = Vec::new();
        loop {
            let line = self.peek().line;
            let declared_type = self.parse_type()?;
            let name = self.expect_ident_name()?;
            formals.push(Param { declared_type, name, line });
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(formals)
    }

    fn parse_actuals(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut actuals = vec![self.parse_expr()?];
        while self.check(&TokenKind::Comma) {
            self.advance();
            actuals.push(self.parse_expr()?);
        }
        Ok(actuals)
    }

    // ---- bodies: method vs. constructor are genuinely different productions ----

    fn parse_method_body_scope(&mut self, class_name: &str) -> Result<BodyScope, ParseError> {
        let line = self.peek().line;
        self.expect(TokenKind::LBrace, ErrorCode::EParsePhaseOther)?;
        let mut locals = Vec::new();
        let mut ctx = ParseContext {
            class_name,
            locals: &mut locals,
            in_constructor: false,
            at_constructor_top_level: false,
            recorded_call_keyword: None,
        };
        let stmts = self.parse_locals_then_stmts(&mut ctx, 1)?; // Stmt+
        self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?;
        Ok(BodyScope { locals, stmts, line })
    }

    fn parse_constructor_body_scope(
        &mut self,
        class_name: &str,
    ) -> Result<(Option<OtherConstructorCall>, BodyScope), ParseError> {
        let line = self.peek().line;
        self.expect(TokenKind::LBrace, ErrorCode::EParsePhaseOther)?;
        let other_call = self.parse_other_constructor_call()?;
        let mut locals = Vec::new();
        let mut ctx = ParseContext {
            class_name,
            locals: &mut locals,
            in_constructor: true,
            at_constructor_top_level: true,
            recorded_call_keyword: other_call.as_ref().map(|d| match d {
                OtherConstructorCall::ThisCall(..) => CallKeyword::This,
                OtherConstructorCall::SuperCall(..) => CallKeyword::Super,
            }),
        };
        let stmts = self.parse_locals_then_stmts(&mut ctx, 0)?; // Stmt*
        self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?;
        Ok((other_call, BodyScope { locals, stmts, line }))
    }

    /// The one legitimate this()/super() slot, if present — the very first thing in a
    /// constructor body, before any VarDecl. `E_MALFORMED_CONSTRUCTOR`-adjacent
    /// legality (whether it's allowed given `extends`) is checker work, not this
    /// function's job — this only records which keyword appeared, if either did.
    fn parse_other_constructor_call(&mut self) -> Result<Option<OtherConstructorCall>, ParseError> {
        let line = self.peek().line;
        let is_this = self.check(&TokenKind::KwThis);
        let is_super = self.check(&TokenKind::KwSuper);
        if !is_this && !is_super {
            return Ok(None);
        }
        if self.peek2().kind != TokenKind::LParen {
            return Ok(None); // this./super. — not a this()/super() call, leave for ordinary Stmt parsing
        }
        self.advance(); // this/super
        self.advance(); // (
        let args = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_actuals()?
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?;
        Ok(Some(if is_this {
            OtherConstructorCall::ThisCall(args, line)
        } else {
            OtherConstructorCall::SuperCall(args, line)
        }))
    }

    /// Shared by method bodies (min_stmts=1), constructor bodies (min_stmts=0), and
    /// if/while bodies (min_stmts=1) — all four are "(VarDecl)* (Stmt)(*|+)".
    fn parse_locals_then_stmts(
        &mut self,
        ctx: &mut ParseContext,
        min_stmts: usize,
    ) -> Result<Vec<Stmt>, ParseError> {
        while self.starts_var_decl() {
            self.parse_var_decl(ctx)?;
        }
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            stmts.push(self.parse_stmt(ctx)?);
        }
        if stmts.len() < min_stmts {
            return Err(ParseError {
                code: ErrorCode::EParsePhaseOther,
                line: self.peek().line,
                message: "expected at least one statement".into(),
            });
        }
        Ok(stmts)
    }

    /// Parses one VarDecl, expands its (possibly several) names, and pushes each
    /// into ctx.locals — checking each new name against every name already in
    /// ctx.locals AND every name already collected earlier in this same
    /// declaration (per-name, not per-VarDecl-node, per the flattening note on
    /// VarDecl in the AST — `int x, x;` is a duplicate within one declaration, not
    /// just across two). This is the parser-level E_DUPLICATE_LOCAL check.
    fn parse_var_decl(&mut self, ctx: &mut ParseContext) -> Result<(), ParseError> {
        let line = self.peek().line;
        let declared_type = self.parse_type()?;
        let mut names: Vec<String> = Vec::new();
        loop {
            let name = self.expect_ident_name()?;
            let is_duplicate = names.contains(&name)
                || ctx.locals.iter().any(|existing| existing.names.contains(&name));
            if is_duplicate {
                return Err(ParseError {
                    code: ErrorCode::EDuplicateLocal,
                    line,
                    message: format!("duplicate local '{}'", name),
                });
            }
            names.push(name);
            if self.check(&TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?;
        ctx.locals.push(VarDecl { declared_type, names, line });
        Ok(())
    }

    // ---- Stmt ----

    fn parse_stmt(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let line = self.peek().line;

        // Decision point 5, step 2: this()/super()-shaped fragment check. Position is
        // checked before keyword — nesting always wins over a keyword-mismatch
        // classification, even when a fragment is technically both.
        if ctx.in_constructor {
            let is_this = self.check(&TokenKind::KwThis);
            let is_super = self.check(&TokenKind::KwSuper);
            if (is_this || is_super) && self.peek2().kind == TokenKind::LParen {
                let code = if !ctx.at_constructor_top_level {
                    ErrorCode::EDelegationNotFirstStatement
                } else {
                    match ctx.recorded_call_keyword {
                        Some(CallKeyword::This) if is_super => {
                            ErrorCode::EDelegationBothSuperAndThis
                        }
                        Some(CallKeyword::Super) if is_this => {
                            ErrorCode::EDelegationBothSuperAndThis
                        }
                        _ => ErrorCode::EDelegationNotFirstStatement,
                    }
                };
                return Err(ParseError {
                    code,
                    line,
                    message: "misplaced constructor this()/super() call".into(),
                });
            }
        }

        match self.peek().kind.clone() {
            TokenKind::KwReturn => {
                self.advance();
                let e = self.parse_expr()?;
                self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?;
                Ok(Stmt::Return(e, line))
            }
            TokenKind::KwIf => self.parse_if_stmt(ctx),
            TokenKind::KwWhile => self.parse_while_stmt(ctx),
            TokenKind::KwBreak => {
                self.advance();
                self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?;
                Ok(Stmt::Break(line))
            }
            TokenKind::Semicolon => {
                self.advance();
                Ok(Stmt::Empty(line))
            }
            TokenKind::Ident(name) => {
                if self.peek2().kind == TokenKind::Equals {
                    self.advance(); // ident
                    self.advance(); // =
                    let e = self.parse_expr()?;
                    self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?;
                    Ok(Stmt::Assign(name, e, line))
                } else {
                    self.parse_call_stmt()
                }
            }
            TokenKind::KwThis | TokenKind::KwSuper | TokenKind::LParen => self.parse_call_stmt(),
            other => Err(ParseError {
                code: ErrorCode::EParsePhaseOther,
                line,
                message: format!("unexpected token starting statement: {:?}", other),
            }),
        }
    }

    fn parse_if_stmt(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let line = self.peek().line;
        self.advance(); // if
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?;
        let cond = self.parse_expr()?;
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        let then_branch = self.parse_nested_block(ctx)?;
        self.expect(TokenKind::KwElse, ErrorCode::EParsePhaseOther)?;
        let else_branch = self.parse_nested_block(ctx)?;
        Ok(Stmt::If(cond, then_branch, else_branch, line))
    }

    fn parse_while_stmt(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let line = self.peek().line;
        self.advance(); // while
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?;
        let cond = self.parse_expr()?;
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        let body = self.parse_nested_block(ctx)?;
        Ok(Stmt::While(cond, body, line))
    }

    /// Grammar's `Block` (if/while body): { (VarDecl)* (Stmt)+ }, no BodyScope of
    /// its own — every VarDecl still goes into the SAME locals collection as the
    /// enclosing method/constructor, via the reborrowed handle below.
    fn parse_nested_block(&mut self, ctx: &mut ParseContext) -> Result<Vec<Stmt>, ParseError> {
        self.expect(TokenKind::LBrace, ErrorCode::EParsePhaseOther)?;
        let mut nested_ctx = ParseContext {
            class_name: ctx.class_name,
            locals: &mut *ctx.locals, // reborrow — same Vec, not a new one
            in_constructor: ctx.in_constructor,
            at_constructor_top_level: false, // always false once nested, permanently for this subtree
            recorded_call_keyword: ctx.recorded_call_keyword,
        };
        let stmts = self.parse_locals_then_stmts(&mut nested_ctx, 1)?; // Stmt+
        self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?;
        Ok(stmts)
    }

    fn parse_call_stmt(&mut self) -> Result<Stmt, ParseError> {
        let line = self.peek().line;
        let receiver = self.parse_receiver()?;
        self.expect(TokenKind::Dot, ErrorCode::EParsePhaseOther)?;
        let name = self.expect_ident_name()?;
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?;
        let args = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_actuals()?
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?;
        Ok(Stmt::CallStmt(MethodCall { receiver, name, args, line }))
    }

    fn parse_receiver(&mut self) -> Result<Receiver, ParseError> {
        let line = self.peek().line;
        match &self.peek().kind {
            TokenKind::Ident(_) => {
                let name = self.expect_ident_name()?;
                Ok(Receiver::Var(name, line))
            }
            TokenKind::KwThis => {
                self.advance();
                Ok(Receiver::This(line))
            }
            TokenKind::KwSuper => {
                self.advance();
                Ok(Receiver::Super(line))
            }
            TokenKind::LParen => {
                let inner = self.parse_paren_value()?;
                Ok(Receiver::Computed(Box::new(inner), line))
            }
            other => Err(ParseError {
                code: ErrorCode::EParsePhaseOther,
                line,
                message: format!(
                    "expected a receiver (identifier, this, super, or parenthesized expression), found {:?}",
                    other
                ),
            }),
        }
    }

    // ---- Expr ----

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        let line = self.peek().line;
        match self.peek().kind.clone() {
            TokenKind::Num(n) => {
                self.advance();
                Ok(Expr::Num(n, line))
            }
            TokenKind::KwTrue => {
                self.advance();
                Ok(Expr::Bool(true, line))
            }
            TokenKind::KwFalse => {
                self.advance();
                Ok(Expr::Bool(false, line))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr::Str(s, line))
            }
            TokenKind::KwNull => {
                self.advance();
                Ok(Expr::Null(line))
            }
            TokenKind::KwNew => self.parse_new_expr(),
            TokenKind::Ident(_) => self.parse_var_or_call_expr(),
            TokenKind::KwThis => self.parse_this_or_call_expr(),
            // Bare `super` is never a legal Expr on its own (no such production) —
            // it only ever appears as a receiver, always followed by `.`.
            TokenKind::KwSuper => {
                self.advance();
                self.finish_call_tail(Receiver::Super(line), line)
            }
            TokenKind::LParen => self.parse_paren_expr(),
            other => Err(ParseError {
                code: ErrorCode::EParsePhaseOther,
                line,
                message: format!("unexpected token starting expression: {:?}", other),
            }),
        }
    }

    fn parse_new_expr(&mut self) -> Result<Expr, ParseError> {
        let line = self.peek().line;
        self.advance(); // new
        let name = self.expect_ident_name()?;
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?;
        let args = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_actuals()?
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        Ok(Expr::New(name, args, line))
    }

    fn parse_var_or_call_expr(&mut self) -> Result<Expr, ParseError> {
        let line = self.peek().line;
        let name = self.expect_ident_name()?;
        if self.check(&TokenKind::Dot) {
            self.finish_call_tail(Receiver::Var(name, line), line)
        } else {
            Ok(Expr::Var(name, line))
        }
    }

    fn parse_this_or_call_expr(&mut self) -> Result<Expr, ParseError> {
        let line = self.peek().line;
        self.advance(); // this
        if self.check(&TokenKind::Dot) {
            self.finish_call_tail(Receiver::This(line), line)
        } else {
            Ok(Expr::This(line))
        }
    }

    /// Assumes the `.` has NOT been consumed yet — consumes it here.
    fn finish_call_tail(&mut self, receiver: Receiver, line: u32) -> Result<Expr, ParseError> {
        self.expect(TokenKind::Dot, ErrorCode::EParsePhaseOther)?;
        let name = self.expect_ident_name()?;
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?;
        let args = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_actuals()?
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        Ok(Expr::Call(MethodCall { receiver, name, args, line }))
    }

    /// `(`-led Expr, including the trailing-call continuation P23/P49 require: once
    /// `parse_paren_value` resolves the parenthesized value, a `.` immediately after
    /// means this was actually a call receiver (`(new Circle(5)).area()`,
    /// `((Cat) a).purr()` used as a value, not a bare statement) — same shape as
    /// decision point 2's `Ident`/`this`/`super` receivers, one level of parens up.
    fn parse_paren_expr(&mut self) -> Result<Expr, ParseError> {
        let line = self.peek().line;
        let value = self.parse_paren_value()?;
        if self.check(&TokenKind::Dot) {
            self.finish_call_tail(Receiver::Computed(Box::new(value), line), line)
        } else {
            Ok(value)
        }
    }

    /// Decision point 4's core: resolves exactly one fully-parenthesized value —
    /// unop, cast, ternary, binop, instanceof, or plain unwrap — consuming the
    /// opening `(` through its matching close and nothing past it. Shared by
    /// `parse_paren_expr` (Expr position) and `parse_receiver`'s `(` arm (statement
    /// position, e.g. `((Cat) a).purr();`) so both resolve `(`-led content
    /// identically, per decision point 4's general dispatch — a receiver is just an
    /// Expr in a position that requires a trailing `.MethodName(...)`.
    fn parse_paren_value(&mut self) -> Result<Expr, ParseError> {
        let line = self.peek().line;
        self.advance(); // consume outer '('

        // Unop: ~ or !
        if self.check(&TokenKind::Tilde) || self.check(&TokenKind::Bang) {
            let op = if self.check(&TokenKind::Tilde) { UnOp::Neg } else { UnOp::Not };
            self.advance();
            let operand = self.parse_expr()?;
            self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
            return Ok(Expr::Un(op, Box::new(operand), line));
        }

        // Double-paren AND the content is exactly `(Type)` — a single keyword or a
        // single bare identifier immediately closed. This is a lookahead-only check
        // (try_peek_type_in_parens), deliberately NOT just "is the next token '('":
        // that weaker condition is also true when the outer paren simply contains an
        // ordinary nested expression that happens to start with '(' (e.g. a cast one
        // level down, as in `((Animal)((Dog)obj))` or `(((Dog)x) instanceof Animal)`)
        // — committing to cast-parsing on that weaker signal alone mishandles those
        // cases. Requiring the full `(Type)` shape up front, via pure lookahead, is
        // what makes the general recursive-descent case below (which handles nested
        // '('-led expressions on its own, correctly, at whatever depth) the fallback
        // for everything that isn't genuinely a type-in-parens.
        if self.try_peek_type_in_parens() {
            return self.parse_cast_or_nested_paren(line);
        }

        // General case: parse ONE full Expr via ordinary recursive dispatch — if it
        // starts with '(', it recurses through this same function and correctly
        // resolves whatever it is (including nested casts) entirely on its own,
        // returning only once it's genuinely complete. Then branch on what follows.
        let first = self.parse_expr()?;
        if self.check(&TokenKind::RParen) {
            self.advance();
            return Ok(first); // plain paren-wrap (P28), unwrapped
        }
        self.finish_paren_after(first, line)
    }

    /// Lookahead-only (consumes nothing): true iff the current token is '(' AND what
    /// immediately follows is exactly one Type token (a primitive keyword, or a bare
    /// identifier immediately followed by ')') — i.e. the current position opens
    /// precisely a `(Type)` fragment. False for anything else, including when the
    /// current token is '(' but what follows is itself another '(' (an ordinary
    /// nested expression, not a type position at all).
    ///
    /// The `)`-immediately-follows check only applies to the identifier case: a
    /// primitive keyword alone is enough to return true, with no check that `)`
    /// follows it too. That's safe only because `parse_expr` has no dispatch arm for
    /// any primitive-keyword token, so a primitive keyword can never legally start an
    /// `Expr` — any malformed shape past it (e.g. no closing `)`) still fails cleanly,
    /// just one level down, via `expect(RParen)` in `parse_cast_or_nested_paren`
    /// rather than here. A bare identifier has no such guarantee (`Ident` legally
    /// starts an ordinary `Expr`), which is why that branch needs the explicit
    /// closure check to avoid false-positiving on something like `(x + 1)`.
    fn try_peek_type_in_parens(&self) -> bool {
        if !self.check(&TokenKind::LParen) {
            return false;
        }
        match &self.peek2().kind {
            TokenKind::KwInt | TokenKind::KwBool | TokenKind::KwString | TokenKind::KwVoid => true,
            TokenKind::Ident(_) => self
                .tokens
                .get(self.pos + 2)
                .map(|t| t.kind == TokenKind::RParen)
                .unwrap_or(false),
            _ => false,
        }
    }

    /// Shared tail for "parsed one Expr inside parens, now decide ternary vs. binop
    /// vs. instanceof" — used by both parse_paren_expr's general case and
    /// parse_cast_or_nested_paren's "was a genuine value" branches.
    fn finish_paren_after(&mut self, first: Expr, line: u32) -> Result<Expr, ParseError> {
        match self.peek().kind.clone() {
            TokenKind::Question => {
                self.advance();
                let then_e = self.parse_expr()?;
                self.expect(TokenKind::Colon, ErrorCode::EParsePhaseOther)?;
                let else_e = self.parse_expr()?;
                self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
                Ok(Expr::Ternary(Box::new(first), Box::new(then_e), Box::new(else_e), line))
            }
            TokenKind::KwInstanceof => {
                self.advance();
                let class_name = self.expect_ident_name()?;
                self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
                Ok(Expr::InstanceOf(Box::new(first), class_name, line))
            }
            kind if Self::binop_for(&kind).is_some() => {
                let op = Self::binop_for(&kind).unwrap();
                self.advance();
                let right = self.parse_expr()?;
                self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
                Ok(Expr::Bin(Box::new(first), op, Box::new(right), line))
            }
            other => Err(ParseError {
                code: ErrorCode::EParsePhaseOther,
                line: self.peek().line,
                message: format!("expected ?, instanceof, an operator, or ) here, found {:?}", other),
            }),
        }
    }

    /// Precondition: try_peek_type_in_parens() was just true, so the current token
    /// is '(' and it opens EXACTLY `(Type)` — a single keyword or a single bare
    /// identifier, immediately closed. Safe to consume directly; no speculative
    /// parsing needed, and no recursive parse_expr() call that could accidentally
    /// swallow more or less than the type-in-parens fragment.
    fn parse_cast_or_nested_paren(&mut self, outer_line: u32) -> Result<Expr, ParseError> {
        self.advance(); // consume the second '('
        let ty = if let Some(ty) = self.try_parse_primitive_type() {
            ty
        } else {
            Type::Class(self.expect_ident_name()?)
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // closes (Type)

        // Now decide: was this genuinely a cast, or just `((Ident))` / `((Ident) op ...)`?
        match self.peek().kind.clone() {
            TokenKind::RParen => {
                // Nothing followed: not a cast. Unwrap both layers.
                self.advance(); // closes outer (
                match ty {
                    Type::Class(name) => Ok(Expr::Var(name, outer_line)),
                    _ => Err(ParseError {
                        code: ErrorCode::EParsePhaseOther,
                        line: outer_line,
                        message: "a primitive type in parens with nothing following is not a valid expression".into(),
                    }),
                }
            }
            TokenKind::Question | TokenKind::KwInstanceof => {
                let as_expr = reinterpret_as_expr(ty, outer_line)?;
                self.finish_paren_after(as_expr, outer_line)
            }
            kind if Self::binop_for(&kind).is_some() => {
                let as_expr = reinterpret_as_expr(ty, outer_line)?;
                self.finish_paren_after(as_expr, outer_line)
            }
            _ => {
                // A new expression starts here with no operator bridging it to the
                // type-shaped fragment — the only grammar shape that fits is a cast.
                let operand = self.parse_expr()?;
                self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
                Ok(Expr::Cast(ty, Box::new(operand), outer_line))
            }
        }
    }
}

/// Used only inside parse_cast_or_nested_paren's "not a cast after all" branches —
/// a primitive-keyword type reaching here (e.g. `((int) + y)`) is never valid,
/// since a value position can't be a bare primitive-type keyword.
fn reinterpret_as_expr(ty: Type, line: u32) -> Result<Expr, ParseError> {
    match ty {
        Type::Class(name) => Ok(Expr::Var(name, line)),
        _ => Err(ParseError {
            code: ErrorCode::EParsePhaseOther,
            line,
            message: "unexpected token following a primitive type in parens".into(),
        }),
    }
}

fn is_keyword(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::KwInt
            | TokenKind::KwBool
            | TokenKind::KwString
            | TokenKind::KwVoid
            | TokenKind::KwClass
            | TokenKind::KwExtends
            | TokenKind::KwThis
            | TokenKind::KwSuper
            | TokenKind::KwNull
            | TokenKind::KwNew
            | TokenKind::KwReturn
            | TokenKind::KwIf
            | TokenKind::KwElse
            | TokenKind::KwWhile
            | TokenKind::KwBreak
            | TokenKind::KwTrue
            | TokenKind::KwFalse
            | TokenKind::KwInstanceof
    )
}
```

---

## `src/add_io_classes.rs`

```rust
use crate::ast::{ClassDecl, IoOp, MethodBody, MethodDecl, Param, Program, Type};

const SYNTHETIC_LINE: u32 = 0;

pub fn add_io_classes(mut program: Program) -> Program {
    let mut classes = vec![input_class(), output_class()];
    classes.append(&mut program.classes);
    program.classes = classes;
    program
}

fn input_class() -> ClassDecl {
    ClassDecl {
        name: "Input".to_string(),
        extends: None,
        fields: vec![],
        constructors: vec![],
        methods: vec![
            io_method(Type::Int, "read_int", IoOp::ReadInt, vec![]),
            io_method(Type::Bool, "read_bool", IoOp::ReadBool, vec![]),
            io_method(Type::String, "read_string", IoOp::ReadString, vec![]),
            io_method(Type::Bool, "eof", IoOp::Eof, vec![]),
        ],
        line: SYNTHETIC_LINE,
    }
}

fn output_class() -> ClassDecl {
    ClassDecl {
        name: "Output".to_string(),
        extends: None,
        fields: vec![],
        constructors: vec![],
        methods: vec![
            io_method(Type::Void, "print_int", IoOp::PrintInt, vec![io_param(Type::Int, "n")]),
            io_method(Type::Void, "print_bool", IoOp::PrintBool, vec![io_param(Type::Bool, "b")]),
            io_method(Type::Void, "print_string", IoOp::PrintString, vec![io_param(Type::String, "s")]),
            io_method(Type::Void, "println", IoOp::Println, vec![]),
        ],
        line: SYNTHETIC_LINE,
    }
}

fn io_method(ret: Type, name: &str, op: IoOp, formals: Vec<Param>) -> MethodDecl {
    MethodDecl {
        ret,
        name: name.to_string(),
        formals,
        body: MethodBody::Io(op),
        line: SYNTHETIC_LINE,
    }
}

fn io_param(declared_type: Type, name: &str) -> Param {
    Param { declared_type, name: name.to_string(), line: SYNTHETIC_LINE }
}
```

Called once, between `parse_program` and the (not-yet-built) type checker: `add_io_classes(parse_program(tokens)?)`. Prepends so `Input`/`Output` sit ahead of every user class, matching the reference's "treated as if it preceded the first user class." Takes no `&Parser`, no tokens — operates purely on the finished `Program`, which is why it's its own file rather than a function in `parser.rs`.

---

## `src/main.rs` (add to the existing file)

```rust
mod ast;
mod parser;
mod add_io_classes;
```

(alongside the existing `mod token; mod lexer;` — `main()` itself doesn't need to change yet, this is still not the CLI.)

---

## Acceptance tests

Hand-traced against the actual control flow above, same discipline as `lexer_implementation.md`. All inputs assume they've already been through `tokenize` — shown here as source text for readability.

Rows that show a full `class ... { }` are direct `parse_program` calls. Rows that show a bare `Expr`/`Stmt`/body fragment (e.g. `((Circle) obj)`, `a.foo().bar();`) trace the relevant internal function (`parse_expr`, `parse_stmt`, etc.) starting mid-token-stream — `parse_program` is the only `pub` entry point, so exercising these as actual unit tests means either wrapping the fragment in a minimal class/method scaffold and asserting on the relevant sub-node of the resulting `Program`, or marking the specific `Parser` methods under test `pub(crate)` for direct construction (`Parser { tokens: &tokens, pos: 0 }`) and invocation in `#[cfg(test)]` code. Either is fine; pick one and apply it consistently. Either way, `tokens` must end with an `Eof` token, same as `tokenize`'s own output — `peek`/`peek2` assume it's always there and never check for an empty slice; a hand-built token vector for a `pub(crate)` test that omits the trailing `Eof` will panic instead of producing a `ParseError`.

| Input | Expected result |
|---|---|
| `class Empty () { }` | `Program { classes: [ClassDecl { name: "Empty", extends: None, fields: [], constructors: [], methods: [] }] }` |
| `class Foo (int x, y;) { }` | one class, `fields: [Param{Int,"x"}, Param{Int,"y"}]` — the field-list flattening path |
| `class Foo (int x;) [ Foo(int n) { x = n; } ] { }` | one constructor, `other_constructor_call: None`, `body.stmts = [Assign("x", Var("n"), _)]` |
| `class Foo () [ ] { }` | `Err(EMalformedClassDecl)` — empty `[ ]` |
| `class Dog extends Animal (int n;) [ Dog(int n) { super(n); } ] { }` | `other_constructor_call: Some(SuperCall([Var("n")], _))` |
| `class Dog extends Animal () [ Dog() { this(5); super(1); } ] { }` | parses the first `this(5);` as the recorded call (`ThisCall`), then hits `super(1);` at constructor top level — opposite keyword, same nesting depth → `Err(EDelegationBothSuperAndThis)` |
| `class Dog extends Animal () [ Dog() { this(5); this(6); } ] { }` | second `this(6);` — same keyword repeated → `Err(EDelegationNotFirstStatement)` |
| `class Dog extends Animal () [ Dog() { x = 1; super(1); } ] { }` | no call recorded at start; `super(1);` appears later → `Err(EDelegationNotFirstStatement)` |
| `class Dog extends Animal () [ Dog() { super(1); if (true) { this(5); } else { ; } } ] { }` | `this(5);` is nested inside the `if` — position checked before keyword, so this is `Err(EDelegationNotFirstStatement)`, **not** `EDelegationBothSuperAndThis`, even though it's also an opposite-keyword case |
| `int x; if (c) { int x; } else { ; }` (inside one method body) | second `int x;` inside the `if` shares `ctx.locals` with the first via the reborrowed handle → `Err(EDuplicateLocal)` |
| `while (a) { int x; } while (b) { int x; }` (two sibling loops, one method body) | same → `Err(EDuplicateLocal)` — confirms the shared-reference threading works across siblings, not just nesting |
| `((Circle) obj)` | `Cast(Class("Circle"), Var("obj"))` |
| `((x))` | `Var("x")` — both paren layers unwrapped, not a cast |
| `((x) + y)` | `Bin(Var("x"), Add, Var("y"))` |
| `((int) n)` | `Cast(Int, Var("n"))` — parses fine; `Int` as a cast target is a checker-level rejection, not a parser one |
| `((x) instanceof Circle)` | `InstanceOf(Var("x"), "Circle")` |
| `((Animal)((Dog)obj))` | `Cast(Class("Animal"), Cast(Class("Dog"), Var("obj")))` |
| `(((Dog) x) instanceof Animal)` | `InstanceOf(Cast(Class("Dog"), Var("x")), "Animal")` — three parens, the only legal way to combine a cast with `instanceof` |
| `a.foo().bar();` as a statement | `Err(EParsePhaseOther)` — `parse_receiver` accepts `a` as `Receiver::Var`, `finish_call_tail`-equivalent path for `Stmt` consumes `.foo(...)`, then expects `;` but finds `.` — no direct call-chaining without parens |
| `(a.foo()).bar();` as a statement | legal — `parse_receiver`'s `LParen` arm calls `parse_paren_value`, which resolves `a.foo()` to `Expr::Call` and returns it → wrapped as `Receiver::Computed`, then `.bar()` completes the call |
| `((Cat) a).purr();` as a statement | legal — `parse_receiver`'s `LParen` arm calls `parse_paren_value`, which sees the `((Cat) a)` shape, resolves it via `parse_cast_or_nested_paren` to `Expr::Cast(Class("Cat"), Var("a"))` → wrapped as `Receiver::Computed`, then `.purr()` completes the call |
| `x = (new Circle(5)).area();` (assignment RHS, expression position) | legal — `parse_expr` dispatches `(` to `parse_paren_expr`, which resolves `(new Circle(5))` via `parse_paren_value` to `Expr::New(...)`, sees the trailing `.`, and calls `finish_call_tail(Receiver::Computed(...))` → `Assign("x", Call(MethodCall{receiver: Computed(New(...)), name: "area", args: []}), _)` |
| `int x, x;` (inside one method body) | second `x` collides with the first within the same declaration, before either is pushed to `ctx.locals` → `Err(EDuplicateLocal)` |
| `class Foo (int x;) { int m() { return x; } } [ Foo(int n) { x = n; } ]` | `[ ]` section appears after `{ }` instead of before → `Err(EMalformedClassDecl)` — the "wrong order" trigger, not a fresh top-level `class` parse attempt |
| `class C extends String () { }` | `Err(EReservedKeywordAsIdentifier)` — `String` lexes as `KwString`, `expect_ident_name` rejects it in the `extends` clause |
| `class Foo (int x;) [ Bar(int x) { } ] { }` | `Err(EMalformedConstructor)` — constructor name `Bar` ≠ class name `Foo` |
| `add_io_classes(Program { classes: vec![] })` | `Program { classes: [Input, Output] }` — both prepended even with no user classes |
| `add_io_classes(Program { classes: [Main] })` | `Program { classes: [Input, Output, Main] }` — order is `Input`, `Output`, then whatever the parser produced, unchanged |
| `Input`'s methods | `read_int: ret Int, formals [], Io(ReadInt)`; `read_bool: ret Bool, Io(ReadBool)`; `read_string: ret String, Io(ReadString)`; `eof: ret Bool, Io(Eof)` |
| `Output`'s methods | `print_int: ret Void, formals [Param{Int,"n"}], Io(PrintInt)`; `print_bool: ret Void, formals [Param{Bool,"b"}], Io(PrintBool)`; `print_string: ret Void, formals [Param{String,"s"}], Io(PrintString)`; `println: ret Void, formals [], Io(Println)` |
| every field/method/param on `Input`/`Output` | `line: 0` |
