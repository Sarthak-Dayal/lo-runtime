# Parser Implementation — LO (LiveOak) P1

Implements `parser_design.md`. No decisions of its own — every choice (AST
shape, function structure, error-code placement, `ParseContext` threading,
the cast-vs-nested-paren resolution) is already settled there; this is the
literal code. Comments here cite grammar productions (`P<n>: <rhs>`) and note
what a token-consuming line actually consumes; they do not explain why the
code is structured the way it is — that reasoning lives in `parser_design.md`.

---

## Files to create / modify

| File | Contents |
|---|---|
| `compiler/src/ast.rs` | new — every AST type below |
| `compiler/src/parser.rs` | new — `ParseError`, `ErrorCode`, `ParseContext` and friends, `parse_program`, and every `parse_*`/support function |
| `compiler/src/main.rs` | already has `mod ast; mod parser;` — no change needed |

---

## `compiler/src/ast.rs`

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
pub struct Formal {
    pub declared_type: Type,
    pub identifier: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarDecl {
    pub declared_type: Type,
    pub identifiers: Vec<String>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassDecl {
    pub class_name: String,
    pub extends: Option<String>,
    pub fields: Vec<VarDecl>,
    pub constructors: Vec<ConstructorDecl>,
    pub methods: Vec<MethodDecl>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstructorDecl {
    pub formals: Vec<Formal>,
    pub delegation: Option<ConstructorDelegation>,
    pub body: BodyScope,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodDecl {
    pub return_type: Type,
    pub method_name: String,
    pub formals: Vec<Formal>,
    pub body: MethodBody,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstructorDelegation {
    ThisCall(Vec<Expr>, u32),
    SuperCall(Vec<Expr>, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum MethodBody {
    UserDefined(BodyScope),
    Io(IoOp),
}

#[derive(Debug, Clone, PartialEq)]
pub enum IoOp {
    ReadInt,
    ReadBool,
    ReadString,
    Eof,
    PrintInt,
    PrintBool,
    PrintString,
    Println,
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
    pub obj_name: ObjName,
    pub method_name: String,
    pub actuals: Vec<Expr>,
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
    Binop(Box<Expr>, Binop, Box<Expr>, u32),
    Unop(Unop, Box<Expr>, u32),
    Cast(Type, Box<Expr>, u32),
    InstanceOf(Box<Expr>, String, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ObjName {
    Var(String, u32),
    This(u32),
    Super(u32),
    Computed(Box<Expr>, u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Binop {
    Add, Sub, Mul, Div, Mod, And, Or, Lt, Gt, Eq,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Unop {
    Not, Neg,
}
```

---

## `compiler/src/parser.rs`

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
#[allow(clippy::enum_variant_names)]
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

fn new_parse_error(code: ErrorCode, line: u32, message: impl Into<String>) -> ParseError {
    ParseError {
        code,
        line,
        message: message.into(),
    }
}

struct ParseContext<'p> {
    locals: &'p mut Vec<VarDecl>,
    constructor: Option<ConstructorContext>,
}

#[derive(Clone, Copy)]
struct ConstructorContext {
    at_top_level: bool,
    delegation: Option<DelegationKeyword>,
}

#[derive(Clone, Copy, PartialEq)]
enum DelegationKeyword {
    This,
    Super,
}

#[derive(PartialEq)]
enum StmtArity {
    ZeroOrMore,
    OneOrMore,
}

// P1: Program -> (ClassDecl)*
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
    // ================================
    // LO grammar production parsing
    // ================================

    // P4: ClassDecl -> class ClassName (extends ClassName)? ( (VarDecl)* ) ( [ (ConstructorDecl)+ ] )? { (MethodDecl)* }
    fn parse_class_decl(&mut self) -> Result<ClassDecl, ParseError> {
        let line = self.peek1().line;
        self.expect(TokenKind::KwClass, ErrorCode::EMalformedClassDecl)?; // "class"
        let class_name = self.parse_class_name()?; // P44: ClassName -> Identifier

        let extends = if self.check(&TokenKind::KwExtends) {
            self.advance(); // "extends"
            Some(self.parse_class_name()?) // P44: ClassName -> Identifier
        } else {
            None
        };

        self.expect(TokenKind::LParen, ErrorCode::EMalformedClassDecl)?; // "("
        let mut fields = Vec::new();
        while !self.check(&TokenKind::RParen) {
            fields.push(self.parse_var_decl()?); // P11: VarDecl -> Type Identifier ( , Identifier)* ;
        }
        self.expect(TokenKind::RParen, ErrorCode::EMalformedClassDecl)?; // ")"

        let constructors = if self.check(&TokenKind::LBracket) {
            self.advance(); // "["
            if self.check(&TokenKind::RBracket) {
                return Err(new_parse_error(
                    ErrorCode::EMalformedClassDecl,
                    self.peek1().line,
                    "empty [ ] constructor section",
                ));
            }
            let mut parsed_constructors = Vec::new();
            while !self.check(&TokenKind::RBracket) {
                // P5/P6: ConstructorDecl -> ClassName ( (Formals)? ) { ( this/super ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }
                parsed_constructors.push(self.parse_constructor_decl(&class_name)?);
            }
            self.advance(); // "]"
            parsed_constructors
        } else {
            Vec::new()
        };

        self.expect(TokenKind::LBrace, ErrorCode::EMalformedClassDecl)?; // "{"
        let mut methods = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            methods.push(self.parse_method_decl()?); // P7: MethodDecl -> Type MethodName ( (Formals)? ) { (VarDecl)* (Stmt)+ }
        }
        self.advance(); // "}"

        Ok(ClassDecl {
            class_name,
            extends,
            fields,
            constructors,
            methods,
            line,
        })
    }

    // P5: ConstructorDecl -> ClassName ( (Formals)? ) { ( this ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }
    // P6: ConstructorDecl -> ClassName ( (Formals)? ) { ( super ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }
    fn parse_constructor_decl(&mut self, class_name: &str) -> Result<ConstructorDecl, ParseError> {
        let line = self.peek1().line;
        let constructor_name = self.parse_class_name()?; // P44: ClassName -> Identifier
        if constructor_name != class_name {
            return Err(new_parse_error(
                ErrorCode::EMalformedConstructor,
                line,
                format!(
                    "constructor name '{}' does not match class name '{}'",
                    constructor_name, class_name
                ),
            ));
        }
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
        let formals = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_formals()? // P9: Formals -> Type Identifier ( , Type Identifier)*
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"

        let body_line = self.peek1().line;
        self.expect(TokenKind::LBrace, ErrorCode::EParsePhaseOther)?; // "{"
        let delegation = self.parse_constructor_delegation()?;
        let mut locals = Vec::new();
        let mut ctx = ParseContext {
            locals: &mut locals,
            constructor: Some(ConstructorContext {
                at_top_level: true,
                delegation: delegation.as_ref().map(|d| match d {
                    ConstructorDelegation::ThisCall(..) => DelegationKeyword::This,
                    ConstructorDelegation::SuperCall(..) => DelegationKeyword::Super,
                }),
            }),
        };
        let stmts = self.parse_var_decls_and_stmts(&mut ctx, StmtArity::ZeroOrMore)?;
        self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?; // "}"
        let body = BodyScope {
            locals,
            stmts,
            line: body_line,
        };
        Ok(ConstructorDecl {
            formals,
            delegation,
            body,
            line,
        })
    }

    // P5: ConstructorDecl -> ClassName ( (Formals)? ) { ( this ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }
    // P6: ConstructorDecl -> ClassName ( (Formals)? ) { ( super ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }
    fn parse_constructor_delegation(
        &mut self,
    ) -> Result<Option<ConstructorDelegation>, ParseError> {
        let line = self.peek1().line;
        let is_this = self.check(&TokenKind::KwThis);
        let is_super = self.check(&TokenKind::KwSuper);
        if !is_this && !is_super {
            return Ok(None);
        }
        if self.peek2().kind != TokenKind::LParen {
            return Ok(None);
        }
        self.advance(); // "this" or "super"
        self.advance(); // "("
        let args = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_actuals()? // P10: Actuals -> Expr ( , Expr)*
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
        self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?; // ";"
        Ok(Some(if is_this {
            ConstructorDelegation::ThisCall(args, line) // P5: ConstructorDecl -> ClassName ( (Formals)? ) { ( this ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }
        } else {
            ConstructorDelegation::SuperCall(args, line) // P6: ConstructorDecl -> ClassName ( (Formals)? ) { ( super ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }
        }))
    }

    // P7: MethodDecl -> Type MethodName ( (Formals)? ) { (VarDecl)* (Stmt)+ }
    fn parse_method_decl(&mut self) -> Result<MethodDecl, ParseError> {
        let line = self.peek1().line;
        let return_type = self.parse_type()?; // P36: Type -> ClassName
        let method_name = self.parse_method_name()?; // P45: MethodName -> Identifier
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
        let formals = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_formals()? // P9: Formals -> Type Identifier ( , Type Identifier)*
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
        let body_line = self.peek1().line;
        self.expect(TokenKind::LBrace, ErrorCode::EParsePhaseOther)?; // "{"
        let mut locals = Vec::new();
        let mut ctx = ParseContext {
            locals: &mut locals,
            constructor: None,
        };
        let stmts = self.parse_var_decls_and_stmts(&mut ctx, StmtArity::OneOrMore)?;
        self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?; // "}"
        let body_scope = BodyScope {
            locals,
            stmts,
            line: body_line,
        };
        Ok(MethodDecl {
            return_type,
            method_name,
            formals,
            body: MethodBody::UserDefined(body_scope),
            line,
        })
    }

    // P9: Formals -> Type Identifier ( , Type Identifier)*
    fn parse_formals(&mut self) -> Result<Vec<Formal>, ParseError> {
        let mut formals = Vec::new();
        loop {
            let line = self.peek1().line;
            let declared_type = self.parse_type()?; // P36: Type -> ClassName
            let identifier = self.parse_identifier()?; // Identifier
            formals.push(Formal {
                declared_type,
                identifier,
                line,
            });
            if self.check(&TokenKind::Comma) {
                self.advance(); // ","
            } else {
                break;
            }
        }
        Ok(formals)
    }

    // P10: Actuals -> Expr ( , Expr)*
    fn parse_actuals(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut actuals = vec![self.parse_expr()?];
        while self.check(&TokenKind::Comma) {
            self.advance(); // ","
            actuals.push(self.parse_expr()?);
        }
        Ok(actuals)
    }

    // P11: VarDecl -> Type Identifier ( , Identifier)* ;
    fn parse_var_decl(&mut self) -> Result<VarDecl, ParseError> {
        let line = self.peek1().line;
        let declared_type = self.parse_type()?; // P36: Type -> ClassName
        let mut identifiers = vec![self.parse_identifier()?]; // Identifier
        while self.check(&TokenKind::Comma) {
            self.advance(); // ","
            identifiers.push(self.parse_identifier()?); // Identifier
        }
        self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?; // ";"
        Ok(VarDecl {
            declared_type,
            identifiers,
            line,
        })
    }

    // P12: Block -> { (VarDecl)* (Stmt)+ }
    fn parse_block(&mut self, ctx: &mut ParseContext) -> Result<Vec<Stmt>, ParseError> {
        self.expect(TokenKind::LBrace, ErrorCode::EParsePhaseOther)?; // "{"
        let mut nested_ctx = ParseContext {
            locals: &mut *ctx.locals, // reborrow -- same Vec as the enclosing body
            constructor: ctx.constructor.map(|_| ConstructorContext {
                at_top_level: false,
                delegation: None,
            }),
        };
        let stmts = self.parse_var_decls_and_stmts(&mut nested_ctx, StmtArity::OneOrMore)?;
        self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?; // "}"
        Ok(stmts)
    }

    // "(VarDecl)* (Stmt)+" (P7, P12) or "(VarDecl)* (Stmt)*" (P5, P6), per `arity`.
    fn parse_var_decls_and_stmts(
        &mut self,
        ctx: &mut ParseContext,
        arity: StmtArity,
    ) -> Result<Vec<Stmt>, ParseError> {
        while self.is_start_of_var_decl() {
            let decl = self.parse_var_decl()?; // P11: VarDecl -> Type Identifier ( , Identifier)* ;
            self.hoist(decl, ctx)?;
        }
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) {
            stmts.push(self.parse_stmt(ctx)?);
        }
        if arity == StmtArity::OneOrMore && stmts.is_empty() {
            return Err(new_parse_error(
                ErrorCode::EParsePhaseOther,
                self.peek1().line,
                "expected at least one statement",
            ));
        }
        Ok(stmts)
    }

    // P13-P19: Stmt -> ...
    fn parse_stmt(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        if let Some(delegation_err) = self.misplaced_delegation_error(ctx) {
            return Err(delegation_err);
        }

        let line = self.peek1().line;
        match self.peek1().kind.clone() {
            // P13: Stmt -> return Expr ;
            TokenKind::KwReturn => {
                self.advance(); // "return"
                let expr = self.parse_expr()?;
                self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?; // ";"
                Ok(Stmt::Return(expr, line))
            }
            // P14: Stmt -> if ( Expr ) Block else Block
            TokenKind::KwIf => self.parse_if_else_stmt(ctx),
            // P15: Stmt -> while ( Expr ) Block
            TokenKind::KwWhile => self.parse_while_stmt(ctx),
            // P16: Stmt -> break ;
            TokenKind::KwBreak => {
                self.advance(); // "break"
                self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?; // ";"
                Ok(Stmt::Break(line))
            }
            // P18: Stmt -> ;
            TokenKind::Semicolon => {
                self.advance(); // ";"
                Ok(Stmt::Empty(line))
            }
            TokenKind::Ident(_) => {
                if self.peek2().kind == TokenKind::Equals {
                    // P17: Stmt -> Var = Expr ;
                    let name = self.parse_var()?; // P50: Var -> Identifier
                    self.advance(); // "="
                    let expr = self.parse_expr()?;
                    self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?; // ";"
                    Ok(Stmt::Assign(name, expr, line))
                } else {
                    // P19: Stmt -> ObjName . MethodName ( (Actuals)? ) ;, via P46: ObjName -> Var
                    self.parse_call_stmt()
                }
            }
            // P19, via P47: ObjName -> this | P48: ObjName -> super | P49: ObjName -> ( Expr )
            TokenKind::KwThis | TokenKind::KwSuper | TokenKind::LParen => self.parse_call_stmt(),
            other => Err(new_parse_error(
                ErrorCode::EParsePhaseOther,
                line,
                format!("unexpected token starting statement: {:?}", other),
            )),
        }
    }

    // P14: Stmt -> if ( Expr ) Block else Block
    fn parse_if_else_stmt(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let line = self.peek1().line;
        self.advance(); // "if"
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
        let cond = self.parse_expr()?;
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
        let if_body = self.parse_block(ctx)?; // P12: Block -> { (VarDecl)* (Stmt)+ }
        self.expect(TokenKind::KwElse, ErrorCode::EParsePhaseOther)?; // "else"
        let else_body = self.parse_block(ctx)?; // P12: Block -> { (VarDecl)* (Stmt)+ }
        Ok(Stmt::If(cond, if_body, else_body, line))
    }

    // P15: Stmt -> while ( Expr ) Block
    fn parse_while_stmt(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
        let line = self.peek1().line;
        self.advance(); // "while"
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
        let cond = self.parse_expr()?;
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
        let body = self.parse_block(ctx)?; // P12: Block -> { (VarDecl)* (Stmt)+ }
        Ok(Stmt::While(cond, body, line))
    }

    // P19: Stmt -> ObjName . MethodName ( (Actuals)? ) ;
    fn parse_call_stmt(&mut self) -> Result<Stmt, ParseError> {
        let line = self.peek1().line;
        let obj_name = self.parse_obj_name()?;
        let call = self.parse_method_call_suffix(obj_name, line)?;
        self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?; // ";"
        Ok(Stmt::CallStmt(call))
    }

    // P46-P49: ObjName -> ...
    fn parse_obj_name(&mut self) -> Result<ObjName, ParseError> {
        let line = self.peek1().line;
        match &self.peek1().kind {
            TokenKind::Ident(_) => {
                let identifier = self.parse_var()?; // P50: Var -> Identifier
                Ok(ObjName::Var(identifier, line)) // P46: ObjName -> Var
            }
            TokenKind::KwThis => {
                self.advance(); // "this"
                Ok(ObjName::This(line)) // P47: ObjName -> this
            }
            TokenKind::KwSuper => {
                self.advance(); // "super"
                Ok(ObjName::Super(line)) // P48: ObjName -> super
            }
            TokenKind::LParen => {
                let inner = self.parse_paren_expr()?;
                Ok(ObjName::Computed(Box::new(inner), line)) // P49: ObjName -> ( Expr )
            }
            other => Err(new_parse_error(
                ErrorCode::EParsePhaseOther,
                line,
                format!(
                    "expected an ObjName (identifier, this, super, or parenthesized expression), found {:?}",
                    other
                ),
            )),
        }
    }

    // The ". MethodName ( (Actuals)? )" fragment of P19 and P23.
    fn parse_method_call_suffix(
        &mut self,
        obj_name: ObjName,
        line: u32,
    ) -> Result<MethodCall, ParseError> {
        self.expect(TokenKind::Dot, ErrorCode::EParsePhaseOther)?; // "."
        let method_name = self.parse_method_name()?; // P45: MethodName -> Identifier
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
        let actuals = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_actuals()? // P10: Actuals -> Expr ( , Expr)*
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
        Ok(MethodCall {
            obj_name,
            method_name,
            actuals,
            line,
        })
    }

    // P20-P23, P31, P32: Expr -> ...
    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        let line = self.peek1().line;
        match self.peek1().kind.clone() {
            // P32: Expr -> Literal
            TokenKind::Num(_) | TokenKind::KwTrue | TokenKind::KwFalse | TokenKind::Str(_) => {
                self.parse_literal()
            }
            // P21: Expr -> null
            TokenKind::KwNull => {
                self.advance(); // "null"
                Ok(Expr::Null(line))
            }
            // P22: Expr -> new ClassName ( (Actuals)? )
            TokenKind::KwNew => self.parse_new_expr(),
            TokenKind::Ident(_) => {
                let identifier = self.parse_var()?; // P50: Var -> Identifier
                if self.check(&TokenKind::Dot) {
                    // P23: Expr -> ObjName . MethodName ( (Actuals)? ), via P46: ObjName -> Var
                    let obj_name = ObjName::Var(identifier, line);
                    Ok(Expr::Call(self.parse_method_call_suffix(obj_name, line)?))
                } else {
                    // P31: Expr -> Var
                    Ok(Expr::Var(identifier, line))
                }
            }
            TokenKind::KwThis => {
                self.advance(); // "this"
                if self.check(&TokenKind::Dot) {
                    // P23: Expr -> ObjName . MethodName ( (Actuals)? ), via P47: ObjName -> this
                    Ok(Expr::Call(
                        self.parse_method_call_suffix(ObjName::This(line), line)?,
                    ))
                } else {
                    // P20: Expr -> this
                    Ok(Expr::This(line))
                }
            }
            // P23: Expr -> ObjName . MethodName ( (Actuals)? ), via P48: ObjName -> super
            TokenKind::KwSuper => {
                self.advance(); // "super"
                Ok(Expr::Call(
                    self.parse_method_call_suffix(ObjName::Super(line), line)?,
                ))
            }
            TokenKind::LParen => {
                let value = self.parse_paren_expr()?; // P25-P30: Expr -> ...
                if self.check(&TokenKind::Dot) {
                    // P23: Expr -> ObjName . MethodName ( (Actuals)? ), via P49: ObjName -> ( Expr )
                    let obj_name = ObjName::Computed(Box::new(value), line);
                    Ok(Expr::Call(self.parse_method_call_suffix(obj_name, line)?))
                } else {
                    Ok(value)
                }
            }
            other => Err(new_parse_error(
                ErrorCode::EParsePhaseOther,
                line,
                format!("unexpected token starting expression: {:?}", other),
            )),
        }
    }

    // P22: Expr -> new ClassName ( (Actuals)? )
    fn parse_new_expr(&mut self) -> Result<Expr, ParseError> {
        let line = self.peek1().line;
        self.advance(); // "new"
        let class_name = self.parse_class_name()?; // P44: ClassName -> Identifier
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
        let actuals = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_actuals()? // P10: Actuals -> Expr ( , Expr)*
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
        Ok(Expr::New(class_name, actuals, line))
    }

    // P25-P30: Expr -> ...
    fn parse_paren_expr(&mut self) -> Result<Expr, ParseError> {
        let line = self.peek1().line;
        self.advance(); // outer "("

        if let Some(op) = Self::to_unop(&self.peek1().kind) {
            // P27: Expr -> ( Unop Expr )
            self.advance(); // "~" or "!"
            let operand = self.parse_expr()?;
            self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
            return Ok(Expr::Unop(op, Box::new(operand), line));
        }

        if let Some(ty) = self.primitive_type_ahead() {
            // P29: Expr -> ( ( Type ) Expr ), Type -> int | bool | String | void
            self.advance(); // second "("
            self.advance(); // primitive-type keyword
            self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
            let value = self.parse_expr()?;
            self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
            return Ok(Expr::Cast(ty, Box::new(value), line));
        }

        // P25/P26/P28/P29(ClassName)/P30's operand: an ordinary Expr
        let primary_expr = self.parse_expr()?;
        self.parse_paren_suffix(primary_expr, line)
    }

    fn parse_paren_suffix(&mut self, primary_expr: Expr, line: u32) -> Result<Expr, ParseError> {
        // P29: Expr -> ( ( Type ) Expr ), Type -> ClassName
        if let Expr::Var(identifier, _) = &primary_expr {
            if self.is_start_of_expr() {
                let value = self.parse_expr()?;
                self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
                return Ok(Expr::Cast(
                    Type::Class(identifier.clone()),
                    Box::new(value),
                    line,
                ));
            }
        }

        // P23: Expr -> ObjName . MethodName ( (Actuals)? ), via P49: ObjName -> ( Expr )
        if self.check(&TokenKind::Dot) {
            let obj_name = ObjName::Computed(Box::new(primary_expr), line);
            let call = Expr::Call(self.parse_method_call_suffix(obj_name, line)?);
            return self.parse_paren_close_or_operator(call, line);
        }

        self.parse_paren_close_or_operator(primary_expr, line)
    }

    // P28: Expr -> ( Expr ), if it closes here; else P25/P26/P30 continue it.
    fn parse_paren_close_or_operator(
        &mut self,
        value: Expr,
        line: u32,
    ) -> Result<Expr, ParseError> {
        if self.check(&TokenKind::RParen) {
            self.advance(); // ")"
            Ok(value)
        } else {
            self.parse_operator_suffix(value, line)
        }
    }

    // P25/P26/P30: Expr -> ... (continuation after the caller's own first Expr)
    fn parse_operator_suffix(&mut self, primary_expr: Expr, line: u32) -> Result<Expr, ParseError> {
        match self.peek1().kind.clone() {
            TokenKind::Question => {
                // P25: Expr -> ( Expr ? Expr : Expr )
                self.advance(); // "?"
                let if_expr = self.parse_expr()?;
                self.expect(TokenKind::Colon, ErrorCode::EParsePhaseOther)?; // ":"
                let else_expr = self.parse_expr()?;
                self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
                Ok(Expr::Ternary(
                    Box::new(primary_expr),
                    Box::new(if_expr),
                    Box::new(else_expr),
                    line,
                ))
            }
            TokenKind::KwInstanceof => {
                // P30: Expr -> ( Expr instanceof ClassName )
                self.advance(); // "instanceof"
                let class_name = self.parse_class_name()?; // P44: ClassName -> Identifier
                self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
                Ok(Expr::InstanceOf(Box::new(primary_expr), class_name, line))
            }
            other => {
                if let Some(op) = Self::to_binop(&other) {
                    // P26: Expr -> ( Expr Binop Expr )
                    self.advance(); // Binop token
                    let right = self.parse_expr()?;
                    self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
                    Ok(Expr::Binop(
                        Box::new(primary_expr),
                        op,
                        Box::new(right),
                        line,
                    ))
                } else {
                    Err(new_parse_error(
                        ErrorCode::EParsePhaseOther,
                        self.peek1().line,
                        format!(
                            "expected ?, instanceof, an operator, or ) here, found {:?}",
                            other
                        ),
                    ))
                }
            }
        }
    }

    // P33: Binop -> [+-*/%&|<>=]
    fn to_binop(kind: &TokenKind) -> Option<Binop> {
        match kind {
            TokenKind::Plus => Some(Binop::Add),
            TokenKind::Minus => Some(Binop::Sub),
            TokenKind::Star => Some(Binop::Mul),
            TokenKind::Slash => Some(Binop::Div),
            TokenKind::Percent => Some(Binop::Mod),
            TokenKind::Amp => Some(Binop::And),
            TokenKind::Pipe => Some(Binop::Or),
            TokenKind::Lt => Some(Binop::Lt),
            TokenKind::Gt => Some(Binop::Gt),
            TokenKind::Equals => Some(Binop::Eq),
            _ => None,
        }
    }

    // P35/P37/P38/P39: Type -> void | int | bool | String
    fn try_parse_primitive_type(&mut self) -> Option<Type> {
        let ty = match &self.peek1().kind {
            TokenKind::KwInt => Type::Int,       // P37: Type -> int
            TokenKind::KwBool => Type::Bool,     // P38: Type -> bool
            TokenKind::KwString => Type::String, // P39: Type -> String
            TokenKind::KwVoid => Type::Void,     // P35: Type -> void
            _ => return None,
        };
        self.advance(); // primitive-type keyword
        Some(ty)
    }

    // P35-P39: Type -> ...
    fn parse_type(&mut self) -> Result<Type, ParseError> {
        if let Some(ty) = self.try_parse_primitive_type() {
            return Ok(ty);
        }
        // P36: Type -> ClassName
        let class_name = self.parse_class_name()?; // P44: ClassName -> Identifier
        Ok(Type::Class(class_name))
    }

    // P44: ClassName -> Identifier
    fn parse_class_name(&mut self) -> Result<String, ParseError> {
        self.parse_identifier()
    }

    // P45: MethodName -> Identifier
    fn parse_method_name(&mut self) -> Result<String, ParseError> {
        self.parse_identifier()
    }

    // P50: Var -> Identifier
    fn parse_var(&mut self) -> Result<String, ParseError> {
        self.parse_identifier()
    }

    // P32: Expr -> Literal; P40-P43: Literal -> ...
    fn parse_literal(&mut self) -> Result<Expr, ParseError> {
        let line = self.peek1().line;
        match self.peek1().kind.clone() {
            TokenKind::Num(n) => {
                self.advance(); // P40: Literal -> Num
                Ok(Expr::Num(n, line))
            }
            TokenKind::KwTrue => {
                self.advance(); // P41: Literal -> true
                Ok(Expr::Bool(true, line))
            }
            TokenKind::KwFalse => {
                self.advance(); // P42: Literal -> false
                Ok(Expr::Bool(false, line))
            }
            TokenKind::Str(s) => {
                self.advance(); // P43: Literal -> String
                Ok(Expr::Str(s, line))
            }
            other => Err(new_parse_error(
                ErrorCode::EParsePhaseOther,
                line,
                format!("expected a literal, found {:?}", other),
            )),
        }
    }

    // P34: Unop -> [~!]
    fn to_unop(kind: &TokenKind) -> Option<Unop> {
        match kind {
            TokenKind::Tilde => Some(Unop::Neg),
            TokenKind::Bang => Some(Unop::Not),
            _ => None,
        }
    }

    // Consumes the lexer's Identifier token (P53).
    fn parse_identifier(&mut self) -> Result<String, ParseError> {
        let line = self.peek1().line;
        match self.peek1().kind.clone() {
            TokenKind::Ident(identifier) => {
                self.advance(); // identifier
                Ok(identifier)
            }
            kind if Self::is_keyword(&kind) => Err(new_parse_error(
                ErrorCode::EReservedKeywordAsIdentifier,
                line,
                format!("expected an identifier, found reserved keyword {:?}", kind),
            )),
            other => Err(new_parse_error(
                ErrorCode::EParsePhaseOther,
                line,
                format!("expected an identifier, found {:?}", other),
            )),
        }
    }

    // ================================
    // Parse-time semantic checks
    // ================================

    fn hoist(&self, decl: VarDecl, ctx: &mut ParseContext) -> Result<(), ParseError> {
        for (i, identifier) in decl.identifiers.iter().enumerate() {
            let is_duplicate = decl.identifiers[..i].contains(identifier)
                || ctx
                    .locals
                    .iter()
                    .any(|existing| existing.identifiers.contains(identifier));
            if is_duplicate {
                return Err(new_parse_error(
                    ErrorCode::EDuplicateLocal,
                    decl.line,
                    format!("duplicate local '{}'", identifier),
                ));
            }
        }
        ctx.locals.push(decl);
        Ok(())
    }

    fn misplaced_delegation_error(&self, ctx: &ParseContext) -> Option<ParseError> {
        let constructor = ctx.constructor?;
        let is_this = self.check(&TokenKind::KwThis);
        let is_super = self.check(&TokenKind::KwSuper);
        if !(is_this || is_super) || self.peek2().kind != TokenKind::LParen {
            return None;
        }
        let code = if !constructor.at_top_level {
            ErrorCode::EDelegationNotFirstStatement
        } else {
            match constructor.delegation {
                Some(DelegationKeyword::This) if is_super => ErrorCode::EDelegationBothSuperAndThis,
                Some(DelegationKeyword::Super) if is_this => ErrorCode::EDelegationBothSuperAndThis,
                _ => ErrorCode::EDelegationNotFirstStatement,
            }
        };
        Some(new_parse_error(
            code,
            self.peek1().line,
            "misplaced constructor this()/super() call",
        ))
    }

    // ================================
    // Token-level helpers
    // ================================

    fn peek1(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek2(&self) -> &Token {
        debug_assert!(self.peek1().kind != TokenKind::Eof);
        &self.tokens[self.pos + 1]
    }

    fn check(&self, kind: &TokenKind) -> bool {
        &self.peek1().kind == kind
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
            Err(new_parse_error(
                code,
                self.peek1().line,
                format!("expected {:?}, found {:?}", kind, self.peek1().kind),
            ))
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

    // distinguishes P11 (VarDecl) from P17/P19 at the top of a body
    fn is_start_of_var_decl(&self) -> bool {
        match &self.peek1().kind {
            TokenKind::KwInt | TokenKind::KwBool | TokenKind::KwString | TokenKind::KwVoid => true,
            TokenKind::Ident(_) => matches!(self.peek2().kind, TokenKind::Ident(_)),
            _ => false,
        }
    }

    // first(Expr).
    fn is_start_of_expr(&self) -> bool {
        matches!(
            self.peek1().kind,
            TokenKind::Num(_)
                | TokenKind::KwTrue
                | TokenKind::KwFalse
                | TokenKind::Str(_)
                | TokenKind::KwNull
                | TokenKind::KwNew
                | TokenKind::Ident(_)
                | TokenKind::KwThis
                | TokenKind::KwSuper
                | TokenKind::LParen
        )
    }

    // LL(2) lookahead disambiguating a primitive-Type P29 from P25/P26/P28/P30.
    fn primitive_type_ahead(&self) -> Option<Type> {
        if !self.check(&TokenKind::LParen) {
            return None;
        }
        match &self.peek2().kind {
            TokenKind::KwInt => Some(Type::Int),
            TokenKind::KwBool => Some(Type::Bool),
            TokenKind::KwString => Some(Type::String),
            TokenKind::KwVoid => Some(Type::Void),
            _ => None,
        }
    }
}
```

---

## Acceptance tests

Hand-traced against the control flow above. Rows showing a full `class ... { }`
are direct `parse_program` calls. Rows showing a bare `Expr`/`Stmt` fragment
trace the relevant internal function starting mid-token-stream; `parse_program`
is the only `pub` entry point, so exercising these directly means either
wrapping the fragment in a minimal class/method scaffold and asserting on the
relevant sub-node of the resulting `Program`, or marking the specific `Parser`
methods under test `pub(crate)` for direct construction and invocation in
`#[cfg(test)]` code. Either way, `tokens` must end with an `Eof` token, same as
`tokenize`'s own output — `peek1`/`peek2` assume it's always
there. Test function names should say what they test (e.g.
`duplicate_local_across_if_else_arms`); no comments above them.

| Input | Expected result |
|---|---|
| `class Empty () { }` | `Program { classes: [ClassDecl { class_name: "Empty", extends: None, fields: [], constructors: [], methods: [] }] }` |
| `class Foo (int x, y;) { }` | one class, `fields: [VarDecl { declared_type: Int, identifiers: ["x", "y"] }]` — one grouped `VarDecl`, not two |
| `class Foo (int x;) [ Foo(int n) { x = n; } ] { }` | one constructor, `formals: [Formal { declared_type: Int, identifier: "n" }]`, `delegation: None`, `body.stmts = [Assign("x", Var("n"), _)]` |
| `class Foo () [ ] { }` | `Err(EMalformedClassDecl)` — empty `[ ]` |
| `class Dog extends Animal (int n;) [ Dog(int n) { super(n); } ] { }` | `delegation: Some(SuperCall([Var("n")], _))` |
| `class Dog extends Animal () [ Dog() { this(5); super(1); } ] { }` | `this(5);` recorded as the delegation; `super(1);` at constructor top level, opposite keyword → `Err(EDelegationBothSuperAndThis)` |
| `class Dog extends Animal () [ Dog() { this(5); this(6); } ] { }` | second `this(6);` repeats the same keyword → `Err(EDelegationNotFirstStatement)` |
| `class Dog extends Animal () [ Dog() { x = 1; super(1); } ] { }` | no delegation recorded at the start; `super(1);` appears later → `Err(EDelegationNotFirstStatement)` |
| `class Dog extends Animal () [ Dog() { super(1); if (true) { this(5); } else { ; } } ] { }` | `this(5);` is nested inside the `if` → `Err(EDelegationNotFirstStatement)`, not `EDelegationBothSuperAndThis`, even though it's also an opposite-keyword case |
| `int x; if (c) { int x; } else { ; }` (inside one method body) | second `int x;` inside the `if` shares `ctx.locals` with the first via the reborrow in `parse_block` → `Err(EDuplicateLocal)` |
| `while (a) { int x; } while (b) { int x; }` (two sibling loops, one method body) | same → `Err(EDuplicateLocal)` across siblings, not just nesting |
| `int x, x;` (inside one method body) | second `x` collides with the first within the same `VarDecl` → `Err(EDuplicateLocal)` |
| `((Circle) obj)` | `Cast(Class("Circle"), Var("obj"))` |
| `((x))` | `Var("x")` — both paren layers unwrapped, not a cast |
| `((x) + y)` | `Binop(Var("x"), Add, Var("y"))` |
| `((int) n)` | `Cast(Int, Var("n"))` — parses fine; a primitive cast target is a checker-level concern, not a parser one |
| `((x) instanceof Circle)` | `InstanceOf(Var("x"), "Circle")` |
| `((Animal)((Dog)obj))` | `Cast(Class("Animal"), Cast(Class("Dog"), Var("obj")))` |
| `(((Dog) x) instanceof Animal)` | `InstanceOf(Cast(Class("Dog"), Var("x")), "Animal")` |
| `a.foo().bar();` as a statement | `Err(EParsePhaseOther)` — `parse_obj_name` accepts `a` as `ObjName::Var`, `parse_method_call_suffix` consumes `.foo(...)`, then expects `;` but finds `.` |
| `(a.foo()).bar();` as a statement | legal — `parse_obj_name`'s `LParen` arm calls `parse_paren_expr`, which resolves `a.foo()` to `Expr::Call` and returns it, wrapped as `ObjName::Computed`, then `.bar()` completes the call |
| `((Cat) a).purr();` as a statement | legal — `parse_obj_name`'s `LParen` arm calls `parse_paren_expr`, which resolves `((Cat) a)` to `Expr::Cast(Class("Cat"), Var("a"))`, wrapped as `ObjName::Computed`, then `.purr()` completes the call |
| `((x).m())` | `Expr::Call(MethodCall { obj_name: Computed(Var("x")), method_name: "m", actuals: [] })` — `parse_expr` parses `(x)` as `Var("x")`, then `parse_paren_suffix` sees `.` (not a cast, since `is_start_of_expr()` is false for `.`) and resolves it as a computed-receiver method call |
| `((x).m() + y)` | `Binop(Call(...), Add, Var("y"))` — same path through `parse_paren_suffix`, which then continues into `parse_operator_suffix` instead of closing the outer paren |
| `x = (new Circle(5)).area();` (assignment RHS) | `parse_expr`'s `LParen` arm resolves `(new Circle(5))` via `parse_paren_expr` to `Expr::New(...)`, sees the trailing `.`, calls `parse_method_call_suffix` with `ObjName::Computed(New(...))` → `Assign("x", Call(...), _)` |
| `class Foo (int x;) { int m() { return x; } } [ Foo(int n) { x = n; } ]` | `[ ]` appears after `{ }` instead of before → `Err(EMalformedClassDecl)` |
| `class C extends String () { }` | `Err(EReservedKeywordAsIdentifier)` — `String` lexes as `KwString`, rejected by `parse_identifier` in the `extends` clause |
| `class Foo (int x;) [ Bar(int x) { } ] { }` | `Err(EMalformedConstructor)` — constructor name `Bar` ≠ class name `Foo` |
