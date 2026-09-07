use crate::ast::*;
use crate::token::{Token, TokenKind};

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub code: ErrorCode,
    pub line: u32,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(clippy::enum_variant_names)] // shared `E` prefix mirrors the course's own error-code naming convention
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
    /// The constructor's recorded delegation call, if any: Some(true) for a
    /// this(...) delegation, Some(false) for super(...), None if no
    /// delegation appeared. Read-only from here on.
    recorded_call_is_this: Option<bool>,
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

    /// Every call site only reaches `peek2`/`peek3` after already confirming
    /// the current token isn't `Eof` (which is always the last token), so
    /// `pos + 1`/`pos + 2` are always in bounds — an out-of-bounds panic here
    /// means that invariant broke, which should fail loudly, not be masked.
    fn peek2(&self) -> &Token {
        &self.tokens[self.pos + 1]
    }

    fn peek3(&self) -> &Token {
        &self.tokens[self.pos + 2]
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

    /// Parses a comma-separated list via `parse_list`, or returns an empty
    /// list without calling it if the current token is already `)`.
    fn parse_list_or_empty<T>(
        &mut self,
        parse_list: fn(&mut Self) -> Result<Vec<T>, ParseError>,
    ) -> Result<Vec<T>, ParseError> {
        if self.check(&TokenKind::RParen) {
            Ok(Vec::new())
        } else {
            parse_list(self)
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
                message: "constructor [ ] section must come before the method body, not after"
                    .into(),
            });
        }

        Ok(ClassDecl {
            name,
            extends,
            fields,
            constructors,
            methods,
            line,
        })
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
                fields.push(Param {
                    declared_type: declared_type.clone(),
                    name,
                    line,
                });
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
        let formals = self.parse_list_or_empty(Self::parse_formals)?;
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;

        let (other_constructor_call, body) = self.parse_constructor_body_scope(class_name)?;
        Ok(ConstructorDecl {
            formals,
            other_constructor_call,
            body,
            line,
        })
    }

    fn parse_method_decl(&mut self, class_name: &str) -> Result<MethodDecl, ParseError> {
        let line = self.peek().line;
        let ret = self.parse_type()?;
        let name = self.expect_ident_name()?;
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?;
        let formals = self.parse_list_or_empty(Self::parse_formals)?;
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        let body_scope = self.parse_method_body_scope(class_name)?;
        Ok(MethodDecl {
            ret,
            name,
            formals,
            body: MethodBody::User(body_scope),
            line,
        })
    }

    fn parse_formals(&mut self) -> Result<Vec<Param>, ParseError> {
        let mut formals = Vec::new();
        loop {
            let line = self.peek().line;
            let declared_type = self.parse_type()?;
            let name = self.expect_ident_name()?;
            formals.push(Param {
                declared_type,
                name,
                line,
            });
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
            recorded_call_is_this: None,
        };
        let stmts = self.parse_locals_then_stmts(&mut ctx, 1)?; // Stmt+
        self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?;
        Ok(BodyScope {
            locals,
            stmts,
            line,
        })
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
            recorded_call_is_this: other_call
                .as_ref()
                .map(|d| matches!(d, OtherConstructorCall::ThisCall(..))),
        };
        let stmts = self.parse_locals_then_stmts(&mut ctx, 0)?; // Stmt*
        self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?;
        Ok((
            other_call,
            BodyScope {
                locals,
                stmts,
                line,
            },
        ))
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
        let args = self.parse_list_or_empty(Self::parse_actuals)?;
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
                || ctx
                    .locals
                    .iter()
                    .any(|existing| existing.names.contains(&name));
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
        ctx.locals.push(VarDecl {
            declared_type,
            names,
            line,
        });
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
                    match ctx.recorded_call_is_this {
                        Some(true) if is_super => ErrorCode::EDelegationBothSuperAndThis,
                        Some(false) if is_this => ErrorCode::EDelegationBothSuperAndThis,
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
            recorded_call_is_this: ctx.recorded_call_is_this,
        };
        let stmts = self.parse_locals_then_stmts(&mut nested_ctx, 1)?; // Stmt+
        self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?;
        Ok(stmts)
    }

    fn parse_call_stmt(&mut self) -> Result<Stmt, ParseError> {
        let line = self.peek().line;
        let receiver = self.parse_receiver()?;
        let call = self.finish_call_tail(receiver, line)?;
        self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?;
        Ok(Stmt::CallStmt(call))
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
            TokenKind::Ident(_) => {
                let name = self.expect_ident_name()?;
                if self.check(&TokenKind::Dot) {
                    Ok(Expr::Call(
                        self.finish_call_tail(Receiver::Var(name, line), line)?,
                    ))
                } else {
                    Ok(Expr::Var(name, line))
                }
            }
            TokenKind::KwThis => {
                self.advance();
                if self.check(&TokenKind::Dot) {
                    Ok(Expr::Call(
                        self.finish_call_tail(Receiver::This(line), line)?,
                    ))
                } else {
                    Ok(Expr::This(line))
                }
            }
            // Bare `super` is never a legal Expr on its own (no such production) —
            // it only ever appears as a receiver, always followed by `.`.
            TokenKind::KwSuper => {
                self.advance();
                Ok(Expr::Call(
                    self.finish_call_tail(Receiver::Super(line), line)?,
                ))
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
        let args = self.parse_list_or_empty(Self::parse_actuals)?;
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        Ok(Expr::New(name, args, line))
    }

    /// Assumes the `.` has NOT been consumed yet — consumes it here.
    fn finish_call_tail(
        &mut self,
        receiver: Receiver,
        line: u32,
    ) -> Result<MethodCall, ParseError> {
        self.expect(TokenKind::Dot, ErrorCode::EParsePhaseOther)?;
        let name = self.expect_ident_name()?;
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?;
        let args = self.parse_list_or_empty(Self::parse_actuals)?;
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        Ok(MethodCall {
            receiver,
            name,
            args,
            line,
        })
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
            Ok(Expr::Call(self.finish_call_tail(
                Receiver::Computed(Box::new(value), line),
                line,
            )?))
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
            let op = if self.check(&TokenKind::Tilde) {
                UnOp::Neg
            } else {
                UnOp::Not
            };
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
        self.parse_paren_operator_tail(first, line)
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
            TokenKind::Ident(_) => self.peek3().kind == TokenKind::RParen,
            _ => false,
        }
    }

    /// Shared tail for "parsed one Expr inside parens, now decide ternary vs. binop
    /// vs. instanceof" — used by both parse_paren_expr's general case and
    /// parse_cast_or_nested_paren's "was a genuine value" branches.
    fn parse_paren_operator_tail(&mut self, first: Expr, line: u32) -> Result<Expr, ParseError> {
        match self.peek().kind.clone() {
            TokenKind::Question => {
                self.advance();
                let if_e = self.parse_expr()?;
                self.expect(TokenKind::Colon, ErrorCode::EParsePhaseOther)?;
                let else_e = self.parse_expr()?;
                self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
                Ok(Expr::Ternary(
                    Box::new(first),
                    Box::new(if_e),
                    Box::new(else_e),
                    line,
                ))
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
                message: format!(
                    "expected ?, instanceof, an operator, or ) here, found {:?}",
                    other
                ),
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
                self.parse_paren_operator_tail(as_expr, outer_line)
            }
            kind if Self::binop_for(&kind).is_some() => {
                let as_expr = reinterpret_as_expr(ty, outer_line)?;
                self.parse_paren_operator_tail(as_expr, outer_line)
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

#[cfg(test)]
impl<'a> Parser<'a> {
    fn for_test(tokens: &'a [Token]) -> Self {
        Parser { tokens, pos: 0 }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn toks(source: &str) -> Vec<Token> {
        tokenize(source).unwrap_or_else(|e| panic!("unexpected lex error on {source:?}: {e:?}"))
    }

    fn program(source: &str) -> Program {
        parse_program(&toks(source))
            .unwrap_or_else(|e| panic!("unexpected parse error on {source:?}: {e:?}"))
    }

    fn program_err(source: &str) -> ParseError {
        parse_program(&toks(source)).expect_err(&format!("expected parse error on {source:?}"))
    }

    fn method_stmts(p: &Program, class_idx: usize, method_idx: usize) -> &[Stmt] {
        match &p.classes[class_idx].methods[method_idx].body {
            MethodBody::User(scope) => &scope.stmts,
            MethodBody::Io(_) => panic!("expected a user-defined method body"),
        }
    }

    fn expr(source: &str) -> Expr {
        let tokens = toks(source);
        let mut parser = Parser::for_test(&tokens);
        parser
            .parse_expr()
            .unwrap_or_else(|e| panic!("unexpected parse error on {source:?}: {e:?}"))
    }

    #[test]
    fn empty_class() {
        let p = program("class Empty () { }");
        assert_eq!(
            p,
            Program {
                classes: vec![ClassDecl {
                    name: "Empty".into(),
                    extends: None,
                    fields: vec![],
                    constructors: vec![],
                    methods: vec![],
                    line: 1,
                }]
            }
        );
    }

    #[test]
    fn grouped_field_names_flatten_to_one_param_each() {
        let p = program("class Foo (int x, y;) { }");
        assert_eq!(
            p.classes[0].fields,
            vec![
                Param {
                    declared_type: Type::Int,
                    name: "x".into(),
                    line: 1
                },
                Param {
                    declared_type: Type::Int,
                    name: "y".into(),
                    line: 1
                },
            ]
        );
    }

    #[test]
    fn constructor_with_no_delegation() {
        let p = program("class Foo (int x;) [ Foo(int n) { x = n; } ] { }");
        let ctor = &p.classes[0].constructors[0];
        assert_eq!(ctor.other_constructor_call, None);
        assert_eq!(
            ctor.body.stmts,
            vec![Stmt::Assign("x".into(), Expr::Var("n".into(), 1), 1)]
        );
    }

    #[test]
    fn empty_bracket_section_is_malformed() {
        let e = program_err("class Foo () [ ] { }");
        assert_eq!(e.code, ErrorCode::EMalformedClassDecl);
    }

    #[test]
    fn super_call_recorded() {
        let p = program("class Dog extends Animal (int n;) [ Dog(int n) { super(n); } ] { }");
        let ctor = &p.classes[0].constructors[0];
        assert_eq!(
            ctor.other_constructor_call,
            Some(OtherConstructorCall::SuperCall(
                vec![Expr::Var("n".into(), 1)],
                1
            ))
        );
    }

    #[test]
    fn delegation_both_super_and_this() {
        let e = program_err("class Dog extends Animal () [ Dog() { this(5); super(1); } ] { }");
        assert_eq!(e.code, ErrorCode::EDelegationBothSuperAndThis);
    }

    #[test]
    fn delegation_repeated_same_keyword() {
        let e = program_err("class Dog extends Animal () [ Dog() { this(5); this(6); } ] { }");
        assert_eq!(e.code, ErrorCode::EDelegationNotFirstStatement);
    }

    #[test]
    fn delegation_not_first_statement_no_prior_call() {
        let e = program_err("class Dog extends Animal () [ Dog() { x = 1; super(1); } ] { }");
        assert_eq!(e.code, ErrorCode::EDelegationNotFirstStatement);
    }

    #[test]
    fn nested_delegation_beats_keyword_mismatch() {
        // this(5) is nested inside the if, so position wins: NotFirstStatement,
        // not BothSuperAndThis, even though it's also an opposite-keyword case.
        let e = program_err(
            "class Dog extends Animal () [ Dog() { super(1); if (true) { this(5); } else { ; } } ] { }",
        );
        assert_eq!(e.code, ErrorCode::EDelegationNotFirstStatement);
    }

    #[test]
    fn duplicate_local_across_if_else_arms() {
        let e = program_err("class Foo () { void m() { int x; if (c) { int x; } else { ; } } }");
        assert_eq!(e.code, ErrorCode::EDuplicateLocal);
    }

    #[test]
    fn duplicate_local_across_sibling_loops() {
        let e = program_err(
            "class Foo () { void m() { while (a) { int x; x = 1; } while (b) { int x; x = 2; } } }",
        );
        assert_eq!(e.code, ErrorCode::EDuplicateLocal);
    }

    #[test]
    fn duplicate_local_within_one_declaration() {
        let e = program_err("class Foo () { void m() { int x, x; } }");
        assert_eq!(e.code, ErrorCode::EDuplicateLocal);
    }

    #[test]
    fn cast_expression() {
        assert_eq!(
            expr("((Circle) obj)"),
            Expr::Cast(
                Type::Class("Circle".into()),
                Box::new(Expr::Var("obj".into(), 1)),
                1
            )
        );
    }

    #[test]
    fn double_paren_identifier_is_not_a_cast() {
        assert_eq!(expr("((x))"), Expr::Var("x".into(), 1));
    }

    #[test]
    fn double_paren_identifier_then_binop() {
        assert_eq!(
            expr("((x) + y)"),
            Expr::Bin(
                Box::new(Expr::Var("x".into(), 1)),
                BinOp::Add,
                Box::new(Expr::Var("y".into(), 1)),
                1
            )
        );
    }

    #[test]
    fn primitive_cast_parses_fine_checker_rejects_later() {
        assert_eq!(
            expr("((int) n)"),
            Expr::Cast(Type::Int, Box::new(Expr::Var("n".into(), 1)), 1)
        );
    }

    #[test]
    fn double_paren_identifier_then_instanceof() {
        assert_eq!(
            expr("((x) instanceof Circle)"),
            Expr::InstanceOf(Box::new(Expr::Var("x".into(), 1)), "Circle".into(), 1)
        );
    }

    #[test]
    fn nested_casts() {
        assert_eq!(
            expr("((Animal)((Dog)obj))"),
            Expr::Cast(
                Type::Class("Animal".into()),
                Box::new(Expr::Cast(
                    Type::Class("Dog".into()),
                    Box::new(Expr::Var("obj".into(), 1)),
                    1
                )),
                1
            )
        );
    }

    #[test]
    fn cast_combined_with_instanceof_needs_three_parens() {
        assert_eq!(
            expr("(((Dog) x) instanceof Animal)"),
            Expr::InstanceOf(
                Box::new(Expr::Cast(
                    Type::Class("Dog".into()),
                    Box::new(Expr::Var("x".into(), 1)),
                    1
                )),
                "Animal".into(),
                1
            )
        );
    }

    #[test]
    fn direct_call_chaining_without_parens_is_illegal() {
        let e = program_err("class Foo () { void m() { a.foo().bar(); } }");
        assert_eq!(e.code, ErrorCode::EParsePhaseOther);
    }

    #[test]
    fn call_chaining_with_parens_is_legal() {
        let p = program("class Foo () { void m() { (a.foo()).bar(); } }");
        let Stmt::CallStmt(call) = &method_stmts(&p, 0, 0)[0] else {
            panic!("expected a call statement");
        };
        assert_eq!(call.name, "bar");
        assert!(
            matches!(&call.receiver, Receiver::Computed(inner, _) if matches!(**inner, Expr::Call(_)))
        );
    }

    #[test]
    fn cast_receiver_then_call() {
        let p = program("class Foo () { void m() { ((Cat) a).purr(); } }");
        let Stmt::CallStmt(call) = &method_stmts(&p, 0, 0)[0] else {
            panic!("expected a call statement");
        };
        assert_eq!(call.name, "purr");
        assert!(matches!(
            &call.receiver,
            Receiver::Computed(inner, _) if matches!(**inner, Expr::Cast(Type::Class(ref c), _, _) if c == "Cat")
        ));
    }

    #[test]
    fn computed_receiver_from_new_in_assignment() {
        let p = program("class Foo () { void m() { x = (new Circle(5)).area(); } }");
        let Stmt::Assign(name, Expr::Call(call), _) = &method_stmts(&p, 0, 0)[0] else {
            panic!("expected an assignment to a call expression");
        };
        assert_eq!(name, "x");
        assert_eq!(call.name, "area");
        assert!(
            matches!(&call.receiver, Receiver::Computed(inner, _) if matches!(**inner, Expr::New(ref n, _, _) if n == "Circle"))
        );
    }

    #[test]
    fn wrong_order_bracket_section_after_methods() {
        let e =
            program_err("class Foo (int x;) { int m() { return x; } } [ Foo(int n) { x = n; } ]");
        assert_eq!(e.code, ErrorCode::EMalformedClassDecl);
    }

    #[test]
    fn string_keyword_rejected_as_identifier() {
        let e = program_err("class C extends String () { }");
        assert_eq!(e.code, ErrorCode::EReservedKeywordAsIdentifier);
    }

    #[test]
    fn constructor_name_must_match_class_name() {
        let e = program_err("class Foo (int x;) [ Bar(int x) { } ] { }");
        assert_eq!(e.code, ErrorCode::EMalformedConstructor);
    }
}
