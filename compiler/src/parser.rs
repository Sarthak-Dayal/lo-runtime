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
        let line = self.peek().line;
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
                    self.peek().line,
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
        let line = self.peek().line;
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

        let body_line = self.peek().line;
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
        let line = self.peek().line;
        let is_this = self.check(&TokenKind::KwThis);
        let is_super = self.check(&TokenKind::KwSuper);
        if !is_this && !is_super {
            return Ok(None);
        }
        if self.peek_ahead1().kind != TokenKind::LParen {
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
        let line = self.peek().line;
        let return_type = self.parse_type()?; // P36: Type -> ClassName
        let method_name = self.parse_method_name()?; // P45: MethodName -> Identifier
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
        let formals = if self.check(&TokenKind::RParen) {
            Vec::new()
        } else {
            self.parse_formals()? // P9: Formals -> Type Identifier ( , Type Identifier)*
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
        let body_line = self.peek().line;
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
            let line = self.peek().line;
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
        let line = self.peek().line;
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
                self.peek().line,
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

        let line = self.peek().line;
        match self.peek().kind.clone() {
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
                if self.peek_ahead1().kind == TokenKind::Equals {
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
        let line = self.peek().line;
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
        let line = self.peek().line;
        self.advance(); // "while"
        self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
        let cond = self.parse_expr()?;
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
        let body = self.parse_block(ctx)?; // P12: Block -> { (VarDecl)* (Stmt)+ }
        Ok(Stmt::While(cond, body, line))
    }

    // P19: Stmt -> ObjName . MethodName ( (Actuals)? ) ;
    fn parse_call_stmt(&mut self) -> Result<Stmt, ParseError> {
        let line = self.peek().line;
        let obj_name = self.parse_obj_name()?;
        let call = self.parse_method_call_suffix(obj_name, line)?;
        self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?; // ";"
        Ok(Stmt::CallStmt(call))
    }

    // P46-P49: ObjName -> ...
    fn parse_obj_name(&mut self) -> Result<ObjName, ParseError> {
        let line = self.peek().line;
        match &self.peek().kind {
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
        let line = self.peek().line;
        match self.peek().kind.clone() {
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
        let line = self.peek().line;
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
        let line = self.peek().line;
        self.advance(); // outer "("

        if let Some(op) = Self::to_unop(&self.peek().kind) {
            // P27: Expr -> ( Unop Expr )
            self.advance(); // "~" or "!"
            let operand = self.parse_expr()?;
            self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
            return Ok(Expr::Unop(op, Box::new(operand), line));
        }

        if self.is_cast_type_ahead() {
            let ty = self.parse_paren_type()?;

            return match ty {
                Type::Class(identifier) if !self.is_start_of_expr() => {
                    // P28: Expr -> ( Expr ), via P31: Expr -> Var
                    self.parse_paren_suffix(Expr::Var(identifier, line), line)
                }
                _ => {
                    // P29: Expr -> ( ( Type ) Expr )
                    let value = self.parse_expr()?;
                    self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
                    Ok(Expr::Cast(ty, Box::new(value), line))
                }
            };
        }

        // P25/P26/P28/P30's operand: an ordinary Expr
        let primary_expr = self.parse_expr()?;
        self.parse_paren_suffix(primary_expr, line)
    }

    // The "( Type )" fragment of P29.
    fn parse_paren_type(&mut self) -> Result<Type, ParseError> {
        self.advance(); // second "("
        let ty = if let Some(ty) = self.try_parse_primitive_type() {
            ty
        } else {
            Type::Class(self.parse_class_name()?) // P44: ClassName -> Identifier
        };
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
        Ok(ty)
    }

    fn parse_paren_suffix(&mut self, primary_expr: Expr, line: u32) -> Result<Expr, ParseError> {
        let value = if self.check(&TokenKind::Dot) {
            let obj_name = ObjName::Computed(Box::new(primary_expr), line);
            // P23: Expr -> ObjName . MethodName ( (Actuals)? ), via P49: ObjName -> ( Expr )
            Expr::Call(self.parse_method_call_suffix(obj_name, line)?)
        } else {
            primary_expr
        };
        if self.check(&TokenKind::RParen) {
            // P28: Expr -> ( Expr )
            self.advance(); // ")"
            Ok(value)
        } else {
            self.parse_operator_suffix(value, line)
        }
    }

    // P25/P26/P30: Expr -> ... (continuation after the caller's own first Expr)
    fn parse_operator_suffix(&mut self, primary_expr: Expr, line: u32) -> Result<Expr, ParseError> {
        match self.peek().kind.clone() {
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
                        self.peek().line,
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
        let ty = match &self.peek().kind {
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
        let line = self.peek().line;
        match self.peek().kind.clone() {
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
        let line = self.peek().line;
        match self.peek().kind.clone() {
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
        if !(is_this || is_super) || self.peek_ahead1().kind != TokenKind::LParen {
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
            self.peek().line,
            "misplaced constructor this()/super() call",
        ))
    }

    // ================================
    // Token-level helpers
    // ================================

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_ahead1(&self) -> &Token {
        debug_assert!(self.peek().kind != TokenKind::Eof);
        &self.tokens[self.pos + 1]
    }

    fn peek_ahead2(&self) -> &Token {
        debug_assert!(self.peek().kind != TokenKind::Eof);
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
            Err(new_parse_error(
                code,
                self.peek().line,
                format!("expected {:?}, found {:?}", kind, self.peek().kind),
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
        match &self.peek().kind {
            TokenKind::KwInt | TokenKind::KwBool | TokenKind::KwString | TokenKind::KwVoid => true,
            TokenKind::Ident(_) => matches!(self.peek_ahead1().kind, TokenKind::Ident(_)),
            _ => false,
        }
    }

    // first(Expr).
    fn is_start_of_expr(&self) -> bool {
        matches!(
            self.peek().kind,
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

    // LL(2) lookahead disambiguating P28 vs. P29.
    fn is_cast_type_ahead(&self) -> bool {
        if !self.check(&TokenKind::LParen) {
            return false;
        }
        match &self.peek_ahead1().kind {
            TokenKind::KwInt | TokenKind::KwBool | TokenKind::KwString | TokenKind::KwVoid => true,
            TokenKind::Ident(_) => self.peek_ahead2().kind == TokenKind::RParen,
            _ => false,
        }
    }
}

#[cfg(test)]
impl<'a> Parser<'a> {
    fn for_test(tokens: &'a [Token]) -> Self {
        Parser { tokens, pos: 0 }
    }
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
            MethodBody::UserDefined(scope) => &scope.stmts,
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

    fn expr_err(source: &str) -> ParseError {
        let tokens = toks(source);
        let mut parser = Parser::for_test(&tokens);
        parser
            .parse_expr()
            .expect_err(&format!("expected parse error on {source:?}"))
    }

    #[test]
    fn empty_class() {
        let p = program("class Empty () { }");
        assert_eq!(
            p,
            Program {
                classes: vec![ClassDecl {
                    class_name: "Empty".into(),
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
    fn grouped_field_names_stay_in_one_var_decl() {
        let p = program("class Foo (int x, y;) { }");
        assert_eq!(
            p.classes[0].fields,
            vec![VarDecl {
                declared_type: Type::Int,
                identifiers: vec!["x".into(), "y".into()],
                line: 1,
            }]
        );
    }

    #[test]
    fn constructor_with_no_delegation() {
        let p = program("class Foo (int x;) [ Foo(int n) { x = n; } ] { }");
        let ctor = &p.classes[0].constructors[0];
        assert_eq!(
            ctor.formals,
            vec![Formal {
                declared_type: Type::Int,
                identifier: "n".into(),
                line: 1,
            }]
        );
        assert_eq!(ctor.delegation, None);
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
            ctor.delegation,
            Some(ConstructorDelegation::SuperCall(
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
    fn duplicate_local_between_sibling_if_else_arms() {
        let e = program_err(
            "class Foo () { void m() { if (c) { int x; x = 1; } else { int x; x = 2; } } }",
        );
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
            Expr::Binop(
                Box::new(Expr::Var("x".into(), 1)),
                Binop::Add,
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
    fn casts_nest_to_arbitrary_depth() {
        assert_eq!(
            expr("((A)((B)((C)obj)))"),
            Expr::Cast(
                Type::Class("A".into()),
                Box::new(Expr::Cast(
                    Type::Class("B".into()),
                    Box::new(Expr::Cast(
                        Type::Class("C".into()),
                        Box::new(Expr::Var("obj".into(), 1)),
                        1
                    )),
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
    fn computed_receiver_call_in_double_parens() {
        assert_eq!(
            expr("((x).m())"),
            Expr::Call(MethodCall {
                obj_name: ObjName::Computed(Box::new(Expr::Var("x".into(), 1)), 1),
                method_name: "m".into(),
                actuals: vec![],
                line: 1,
            })
        );
    }

    #[test]
    fn computed_receiver_call_then_binop() {
        assert_eq!(
            expr("((x).m() + y)"),
            Expr::Binop(
                Box::new(Expr::Call(MethodCall {
                    obj_name: ObjName::Computed(Box::new(Expr::Var("x".into(), 1)), 1),
                    method_name: "m".into(),
                    actuals: vec![],
                    line: 1,
                })),
                Binop::Add,
                Box::new(Expr::Var("y".into(), 1)),
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
        assert_eq!(call.method_name, "bar");
        assert!(
            matches!(&call.obj_name, ObjName::Computed(inner, _) if matches!(**inner, Expr::Call(_)))
        );
    }

    #[test]
    fn cast_receiver_then_call() {
        let p = program("class Foo () { void m() { ((Cat) a).purr(); } }");
        let Stmt::CallStmt(call) = &method_stmts(&p, 0, 0)[0] else {
            panic!("expected a call statement");
        };
        assert_eq!(call.method_name, "purr");
        assert!(matches!(
            &call.obj_name,
            ObjName::Computed(inner, _) if matches!(**inner, Expr::Cast(Type::Class(ref c), _, _) if c == "Cat")
        ));
    }

    #[test]
    fn computed_receiver_from_new_in_assignment() {
        let p = program("class Foo () { void m() { x = (new Circle(5)).area(); } }");
        let Stmt::Assign(name, Expr::Call(call), _) = &method_stmts(&p, 0, 0)[0] else {
            panic!("expected an assignment to a call expression");
        };
        assert_eq!(name, "x");
        assert_eq!(call.method_name, "area");
        assert!(
            matches!(&call.obj_name, ObjName::Computed(inner, _) if matches!(**inner, Expr::New(ref n, _, _) if n == "Circle"))
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

    #[test]
    fn second_methods_own_closing_brace_does_not_leak_to_class_end() {
        let p = program("class Foo () { void a() { ; } void b() { ; } }");
        assert_eq!(p.classes[0].methods.len(), 2);
        assert_eq!(p.classes[0].methods[0].method_name, "a");
        assert_eq!(p.classes[0].methods[1].method_name, "b");
    }

    #[test]
    fn locals_hoisted_flat_regardless_of_nesting() {
        let p = program(
            "class Foo () { void m() { int x; if (c) { int y; y = 1; } else { ; } x = 1; } }",
        );
        let MethodBody::UserDefined(scope) = &p.classes[0].methods[0].body else {
            panic!("expected a user-defined method body");
        };
        assert_eq!(
            scope.locals,
            vec![
                VarDecl {
                    declared_type: Type::Int,
                    identifiers: vec!["x".into()],
                    line: 1
                },
                VarDecl {
                    declared_type: Type::Int,
                    identifiers: vec!["y".into()],
                    line: 1
                },
            ]
        );
        assert_eq!(scope.stmts.len(), 2);
    }

    #[test]
    fn empty_constructor_body_is_legal() {
        let p = program("class Foo () [ Foo() { } ] { }");
        let ctor = &p.classes[0].constructors[0];
        assert_eq!(ctor.delegation, None);
        assert_eq!(ctor.body.stmts, vec![]);
    }

    #[test]
    fn empty_method_body_is_illegal() {
        let e = program_err("class Foo () { void m() { } }");
        assert_eq!(e.code, ErrorCode::EParsePhaseOther);
    }

    #[test]
    fn ternary_expression() {
        assert_eq!(
            expr("(c ? 1 : 2)"),
            Expr::Ternary(
                Box::new(Expr::Var("c".into(), 1)),
                Box::new(Expr::Num(1, 1)),
                Box::new(Expr::Num(2, 1)),
                1
            )
        );
    }

    #[test]
    fn unary_operators() {
        assert_eq!(
            expr("(~x)"),
            Expr::Unop(Unop::Neg, Box::new(Expr::Var("x".into(), 1)), 1)
        );
        assert_eq!(
            expr("(!x)"),
            Expr::Unop(Unop::Not, Box::new(Expr::Var("x".into(), 1)), 1)
        );
    }

    #[test]
    fn delegation_followed_by_ordinary_call_is_legal() {
        let p = program("class Dog extends Animal () [ Dog() { super(1); this.setup(); } ] { }");
        let ctor = &p.classes[0].constructors[0];
        assert_eq!(
            ctor.delegation,
            Some(ConstructorDelegation::SuperCall(vec![Expr::Num(1, 1)], 1))
        );
        let Stmt::CallStmt(call) = &ctor.body.stmts[0] else {
            panic!("expected a call statement");
        };
        assert_eq!(call.method_name, "setup");
        assert!(matches!(call.obj_name, ObjName::This(_)));
    }

    #[test]
    fn delegation_both_super_and_this_reverse_order() {
        let e = program_err("class Dog extends Animal () [ Dog() { super(1); this(5); } ] { }");
        assert_eq!(e.code, ErrorCode::EDelegationBothSuperAndThis);
    }

    #[test]
    fn this_call_recorded() {
        let p = program("class Foo (int x;) [ Foo() { this(5); } ] { }");
        let ctor = &p.classes[0].constructors[0];
        assert_eq!(
            ctor.delegation,
            Some(ConstructorDelegation::ThisCall(vec![Expr::Num(5, 1)], 1))
        );
    }

    #[test]
    fn return_statement() {
        let p = program("class Foo () { int m() { return 1; } }");
        assert_eq!(
            method_stmts(&p, 0, 0).to_vec(),
            vec![Stmt::Return(Expr::Num(1, 1), 1)]
        );
    }

    #[test]
    fn break_statement_inside_while() {
        let p = program("class Foo () { void m() { while (true) { break; } } }");
        assert_eq!(
            method_stmts(&p, 0, 0).to_vec(),
            vec![Stmt::While(Expr::Bool(true, 1), vec![Stmt::Break(1)], 1)]
        );
    }

    #[test]
    fn if_statement_shape() {
        let p = program("class Foo () { void m() { if (c) { x = 1; } else { y = 2; } } }");
        assert_eq!(
            method_stmts(&p, 0, 0).to_vec(),
            vec![Stmt::If(
                Expr::Var("c".into(), 1),
                vec![Stmt::Assign("x".into(), Expr::Num(1, 1), 1)],
                vec![Stmt::Assign("y".into(), Expr::Num(2, 1), 1)],
                1
            )]
        );
    }

    #[test]
    fn program_with_multiple_classes() {
        let p = program("class A () { } class B () { }");
        assert_eq!(p.classes.len(), 2);
        assert_eq!(p.classes[0].class_name, "A");
        assert_eq!(p.classes[1].class_name, "B");
    }

    #[test]
    fn missing_field_parens_is_malformed() {
        let e = program_err("class Foo { }");
        assert_eq!(e.code, ErrorCode::EMalformedClassDecl);
    }

    #[test]
    fn bare_literals() {
        assert_eq!(expr("5"), Expr::Num(5, 1));
        assert_eq!(expr("true"), Expr::Bool(true, 1));
        assert_eq!(expr("false"), Expr::Bool(false, 1));
        assert_eq!(expr("\"hi\""), Expr::Str("hi".into(), 1));
        assert_eq!(expr("null"), Expr::Null(1));
    }

    #[test]
    fn new_expression_with_multiple_args() {
        assert_eq!(
            expr("new Circle(5, 6)"),
            Expr::New("Circle".into(), vec![Expr::Num(5, 1), Expr::Num(6, 1)], 1)
        );
    }

    #[test]
    fn primitive_type_alone_in_double_parens_is_invalid() {
        assert_eq!(expr_err("((int))").code, ErrorCode::EParsePhaseOther);
    }

    #[test]
    fn primitive_type_before_instanceof_is_invalid() {
        assert_eq!(
            expr_err("((int) instanceof Foo)").code,
            ErrorCode::EParsePhaseOther
        );
    }

    #[test]
    fn multiple_var_decls_in_field_list() {
        let p = program("class Fiver (int a, b; bool x, y; String str;) { }");
        assert_eq!(
            p.classes[0].fields,
            vec![
                VarDecl {
                    declared_type: Type::Int,
                    identifiers: vec!["a".into(), "b".into()],
                    line: 1
                },
                VarDecl {
                    declared_type: Type::Bool,
                    identifiers: vec!["x".into(), "y".into()],
                    line: 1
                },
                VarDecl {
                    declared_type: Type::String,
                    identifiers: vec!["str".into()],
                    line: 1
                },
            ]
        );
    }

    #[test]
    fn reserved_keyword_rejected_as_local_name() {
        let e = program_err("class Foo () { void m() { int this; } }");
        assert_eq!(e.code, ErrorCode::EReservedKeywordAsIdentifier);
    }

    #[test]
    fn reserved_keyword_rejected_as_method_name() {
        let e = program_err("class Foo () { bool break() { ; } }");
        assert_eq!(e.code, ErrorCode::EReservedKeywordAsIdentifier);
    }

    #[test]
    fn multiple_constructors_in_one_class() {
        let p = program(
            "class Cake (int layers;) [ Cake() { this(5); } Cake(int n) { layers = n; } ] { }",
        );
        assert_eq!(p.classes[0].constructors.len(), 2);
        assert_eq!(p.classes[0].constructors[0].formals.len(), 0);
        assert_eq!(p.classes[0].constructors[1].formals.len(), 1);
    }

    #[test]
    fn plain_instanceof_without_double_parens() {
        assert_eq!(
            expr("(a instanceof Cat)"),
            Expr::InstanceOf(Box::new(Expr::Var("a".into(), 1)), "Cat".into(), 1)
        );
    }
}
