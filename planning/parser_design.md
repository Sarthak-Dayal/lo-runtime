# Parser Design — LO (LiveOak) P1

Scope: `Vec<Token>` in, `Program` (the AST) out.

---

## AST

```rust
pub enum Type {
    Int,
    Bool,
    String,
    Void,
    Class(String),
}

pub struct Program {
    pub classes: Vec<ClassDecl>,
}

// One entry of a Formals list (P9) -- each entry restates its own type
// ("int x, bool y"), never grouped, unlike VarDecl below.
pub struct Formal {
    pub declared_type: Type,
    pub identifier: String,
    pub line: u32,
}

// One VarDecl (P11) -- kept grouped, matching the grammar exactly: one
// shared type, one or more identifiers ("int x, y, z;" is a single VarDecl
// with three identifiers, not three VarDecls). Not flattened at parse time --
// no parser-side consumer needs a flat per-name view (E_DUPLICATE_LOCAL
// checks name-by-name against the still-grouped list just fine; the
// checker-phase E_DUPLICATE_FIELD has no parser-side need at all), so
// there's nothing to buy by discarding the grouping here. Whoever needs a
// flat (type, name, line) view later -- the checker, codegen -- expands
// `identifiers` at the point of use.
pub struct VarDecl {
    pub declared_type: Type,
    pub identifiers: Vec<String>,
    pub line: u32,
}

pub struct ClassDecl {
    pub class_name: String,
    pub extends: Option<String>,
    pub fields: Vec<VarDecl>,
    pub constructors: Vec<ConstructorDecl>,
    pub methods: Vec<MethodDecl>,
    pub line: u32,
}

pub struct ConstructorDecl {
    pub formals: Vec<Formal>,
    pub delegation: Option<ConstructorDelegation>,
    pub body: BodyScope,
    pub line: u32,
}

pub struct MethodDecl {
    pub return_type: Type,
    pub method_name: String,
    pub formals: Vec<Formal>,
    pub body: MethodBody,
    pub line: u32,
}

pub enum ConstructorDelegation {
    ThisCall(Vec<Expr>, u32),
    SuperCall(Vec<Expr>, u32),
}

pub enum MethodBody {
    UserDefined(BodyScope),
    Io(IoOp), // never produced by the parser
}

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

pub struct BodyScope {
    pub locals: Vec<VarDecl>,
    pub stmts: Vec<Stmt>,
    pub line: u32,
}

pub enum Stmt {
    Assign(String, Expr, u32),
    Return(Expr, u32),
    If(Expr, Vec<Stmt>, Vec<Stmt>, u32),
    While(Expr, Vec<Stmt>, u32),
    Break(u32),
    Empty(u32),
    CallStmt(MethodCall),
}

pub struct MethodCall {
    pub obj_name: ObjName,
    pub method_name: String,
    pub actuals: Vec<Expr>,
    pub line: u32,
}

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

pub enum ObjName {
    Var(String, u32),
    This(u32),
    Super(u32),
    Computed(Box<Expr>, u32),
}

pub enum Binop {
    Add, Sub, Mul, Div, Mod, And, Or, Lt, Gt, Eq,
}

pub enum Unop {
    Not, Neg,
}
```

- Every node carries its own `line`.
- `Type::String` is its own variant, not `Class("String")`.
- `if`/`while` bodies are bare `Vec<Stmt>` — no wrapper type.
- `BodyScope` holds every hoisted local for one method/constructor body, flattened regardless of nesting depth.
- Field and type names match the grammar's own vocabulary: `identifier` (Identifier), `class_name` (ClassName), `method_name` (MethodName), `formals` (Formals), `actuals` (Actuals), `obj_name` (ObjName), `Binop`/`Unop`.

**`Formal` vs. `VarDecl`:**
- Two distinct types; neither is flattened at parse time.
- `Formal` mirrors P9 (`Formals`) exactly: one type, one identifier, per entry — the grammar never groups names here.
- `VarDecl` mirrors P11 (`VarDecl`) exactly: one type, `Vec<String>` of identifiers — the grammar does group names here (`int x, y, z;`).
- No per-name flattening: no parser-side consumer needs a flat per-name view badly enough to justify discarding the grammar's own grouped shape.
- `ClassDecl.fields` and `BodyScope.locals` are both `Vec<VarDecl>` — P4's `( (VarDecl)* )` and P7/P12/P5/P6's `(VarDecl)*` are the same production (P11), reused verbatim at both call sites, not two different productions that happen to look alike.

---

## Grammar-citation convention

- A function implementing exactly one production gets a one-line header directly above `fn`: `// P<n>: <exact RHS>`, copied verbatim from the grammar table, never paraphrased.
- A function dispatching over several productions of the same nonterminal gets a header listing every production it covers — `// P<n>-P<m>: <Nonterminal> -> ...` for a contiguous run, `// P<n>/P<m>/...: <Nonterminal> -> ...` when it isn't — and each match arm gets its own precise `// P<n>: <exact RHS>` directly above it.
- A function with no production of its own (LL(2) lookahead, a shared continuation tail, token-level machinery) gets no `P<n>` header at all — the absence is the signal; there's no separate "not a production" disclaimer to write.
- Where two or more productions share a prefix and a lookahead call is needed, the comment names the exact production pair (or triple) forcing it — never just "needs lookahead" without saying against what.
- See "LL(2) decision points" below for the full list; every `peek2` call traces to an entry in it.

---

## LL(2) decision points

- `peek1()` reads the current token.
- `peek2()` reads the next token. A fixed, zero-argument function, not one taking a numeric offset — a parameterized `peek_ahead(n)` could silently be called with `n > 1` anywhere, which would blow past the bound this whole section documents without anything visibly different at the call site. With only `peek2` existing, looking further ahead would require adding a new function, not just typing a different number — a deliberate, reviewable change instead of a silent one.
- Every decision point below resolves with `peek2()` alone. Nothing backtracks, and nothing ever needs a second token of lookahead — including decision point 3, which looks like it should at first glance. See that entry for why it doesn't.

### 1. Ident-led: `VarDecl` vs. assignment vs. call statement

- Shared prefix: one `Identifier`.
- P11: `VarDecl -> Type Identifier ( , Identifier)* ;`
- P17: `Stmt -> Var = Expr ;`
- P19, via P46 (`ObjName -> Var`): `Stmt -> ObjName . MethodName ( (Actuals)? ) ;`
- Separates at `peek2()`: another `Identifier` → P11; `=` → P17; anything else → P19.
- Code: `is_start_of_var_decl` picks P11 vs. not; `parse_stmt`'s own check on `=` then picks P17 vs. P19.

### 2. `this`-led: bare `this` vs. `this.method(...)`

- Shared prefix: `this`.
- P20: `Expr -> this`
- P23, via P47 (`ObjName -> this`): `Expr -> ObjName . MethodName ( (Actuals)? )`
- Separates at `peek2()`: `.` → P23; anything else → P20.
- Code: each of `parse_expr`'s `Ident`/`KwThis`/`LParen` arms checks for `Dot` directly.
- `super` has no P20-equivalent bare-`Expr` production — only P48 (`ObjName -> super`) exists — so `super` in `Expr` position always goes straight to P23, no lookahead needed.

### 3. `( (`-led: cast vs. an ordinary nested `(`-led `Expr`

- Shared prefix: `( (`.
- P28: `Expr -> ( Expr )`, where the inner `Expr` itself starts with `(` (recursing into any of P25-P30).
- P29: `Expr -> ( ( Type ) Expr )`.
- A primitive-type keyword (P35/P37/P38/P39) at `peek2()` is unambiguous — resolves immediately, since a primitive keyword can never start an ordinary `Expr`. Code: `is_primitive_type_ahead`.
- An `Identifier` at `peek2()` is where it gets interesting. `((Dog)obj)` (a cast) and `((x))` (a redundantly-parenthesized `Var`) are indistinguishable for the first three tokens — `( Identifier )` — no matter how far you look with fixed lookahead: whatever's inside that inner `( )` can itself be an arbitrarily deep, further-nested `Expr` (e.g. `((((Dog)))obj)`), and each extra layer pushes the one token that actually reveals the answer further to the right. So this is **not** "peek one token further" (LL(3)) or even "peek one more after that" (LL(4)) — no fixed *k* tokens of raw lookahead from the leading `(` resolves it in general.
  - Resolution: don't try to look past the ambiguity — parse through it. Consume `(Identifier)` as an ordinary `Var` via the normal recursive-descent `parse_expr()` call (valid syntax either way, so nothing is lost by committing to it, and recursion handles however deep the nesting actually is, since depth is a stack property, not a lookahead property). Once that call returns, check the *current* token with a single fresh `peek1()`: does another expression immediately follow with no connector? Only a cast produces that shape, so if so, reinterpret the `Var`'s name as the cast's `Type` and parse the operand; otherwise it really was just a `Var`, and whatever follows (`.`, an operator, `)`, `instanceof`) is handled the same as for any other parsed value. Code: the `if let Expr::Var(identifier, _) = &primary_expr { if self.is_start_of_expr() { ... } }` guard at the top of `parse_paren_suffix`.
  - This keeps every actual decision point at `peek1()` + `peek2()` — no exception, unlike an earlier version of this parser which used a `peek_ahead2()` to check "does `)` follow the identifier immediately" as a pre-filter before committing to a tentative parse. That check only ever answered "is this worth attempting as a type," not "is it a cast" (`((x))` passes it too), so it didn't actually buy a one-shot decision — full resolution still needed a further token after that, checked separately. Parsing through the shared material and deciding once, afterward, does the same job with a strictly smaller lookahead budget and no leftover exception to document.
  - The two sub-cases (primitive vs. `ClassName`) are decided at genuinely different times relative to consumption — primitive keywords structurally cannot be parsed as an ordinary `Expr` at all, so that case must be decided *before* parsing anything; `ClassName` shares its `Identifier` token with `Var`, so it can only be decided *after* parsing. This is why the two cases live in different functions (`parse_paren_expr`'s upfront check vs. `parse_paren_suffix`'s post-parse check) rather than one — it isn't an arbitrary split, it reflects when each case's answer actually becomes knowable.
- On why this isn't really an LL(2)-vs-LL(k) tradeoff: LL(2) doesn't promise that two tokens from the *start* of a construct determine its whole parse — it promises that at each point where the parser must choose a production, two tokens of lookahead from the parser's *current position* suffice. The apparent need for more tokens here comes from trying to decide too early, before the shared, unboundedly-recursive material (`Expr`, not just a bare `Identifier`) has been consumed. That shared material isn't a fixed-length prefix, so it can't be left-factored into a new grammar rule the textbook way (`A -> αβ1 | αβ2` ⟹ `A -> αA'`, `A' -> β1 | β2` assumes a finite `α`); the factoring instead happens implicitly, in the recursive-descent implementation itself, by calling `parse_expr()` to consume however much of that material is actually present and deferring the branch to right after.
- Matches the course text precisely: *"the tokens `(`, `(`, type-or-keyword, `)`, expression, `)` form the cast pattern."*
- Nested casts (`((Animal)((Dog)obj))`) and casts combined with `instanceof` (`(((Dog) x) instanceof Animal)`, three parens) fall out of the same mechanism applied again at whichever recursion level is adjacent to the revealing token — no new machinery, and no need for the reinterpretation to "see through" however many layers of redundant nesting sit above it.
- Never consults a name/symbol table — purely structural.

### 4. Constructor delegation position — not an ambiguity between two legal productions

- `this(...)`/`super(...)` immediately followed by `(` in ordinary statement position isn't covered by *any* `Stmt` production — the grammar's only place for that exact shape is the optional prefix inline in P5/P6.
- So this isn't two productions sharing a prefix — it's a shape that must be actively rejected once it appears anywhere that prefix isn't legal.
- Same lookahead as decision point 2 (`this`/`super`, then `peek2()` for `(`), reused inside `parse_stmt` via `misplaced_delegation_error`.
- Algorithm:
  1. At the start of constructor-body parsing, check for `this`/`super` directly followed by `(`. If found, record it as the one legitimate delegation slot (which keyword). If not, no delegation was declared — legal, delegation is always optional.
  2. For the rest of the body, at any nesting depth: position is checked before keyword.
     - Nested inside any `if`/`while` → `E_DELEGATION_NOT_FIRST_STATEMENT`, unconditionally, even if it's also an opposite-keyword case (nesting wins first).
     - A literal top-level sibling statement using the opposite keyword from the one recorded → `E_DELEGATION_BOTH_SUPER_AND_THIS`.
     - Every other top-level-sibling case (none recorded yet, or the same keyword repeated) → `E_DELEGATION_NOT_FIRST_STATEMENT`.
  3. Anything not delegation-shaped falls through to ordinary `Stmt` parsing.
- Not flagged: `super(n); this.setup();` (the `this` there is followed by `.`, not `(`) and a root constructor with only `this.foo();` and no delegation at all — both fail the delegation-shaped test at step 1 and parse as ordinary statements.

---

## Parser support types

Threaded through `parse_var_decls_and_stmts`/`parse_block`/`parse_stmt`/`parse_var_decl` so `E_DUPLICATE_LOCAL` hoisting and decision point 4's delegation check both have what they need without a `Parser`-level field (which would leak state across sibling `parse_class_decl`/`parse_method_decl` calls on the same `Parser`).

```rust
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
```

Decision point 4's check, called from `parse_stmt` (P13's listing) before every `Stmt` dispatch:

```rust
// Decision point 4: this()/super() delegation is only legal as the one
// optional prefix of a constructor's own top-level body (P5/P6) -- not a
// Stmt production, so it must be actively rejected everywhere else a
// this(...)/super(...)-shaped fragment appears.
fn misplaced_delegation_error(&self, ctx: &ParseContext) -> Option<ParseError> {
    let constructor = ctx.constructor?; // not in a constructor body at all
    let is_this = self.check(&TokenKind::KwThis);
    let is_super = self.check(&TokenKind::KwSuper);
    if !(is_this || is_super) || self.peek2().kind != TokenKind::LParen {
        return None; // not delegation-shaped -- ordinary Stmt parsing applies
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
    Some(new_parse_error(code, self.peek1().line, "misplaced constructor this()/super() call"))
}
```

---

## Grammar walkthrough, P1–P53

For every production: the line as the grammar table states it, then the exact
parser code that implements it. Where a production is one `match` arm inside a
larger dispatcher, the snippet is that arm alone, not the whole function — the
function is shown in full once, at the first production it implements.
Productions with no content at LO-4 (P2, P3, P8, P24) are omitted.

### P1

**P1**: `Program -> (ClassDecl)*`
```rust
pub fn parse_program(tokens: &[Token]) -> Result<Program, ParseError> {
    let mut parser = Parser { tokens, pos: 0 };
    let mut classes = Vec::new();
    while !parser.check(&TokenKind::Eof) {
        classes.push(parser.parse_class_decl()?);
    }
    Ok(Program { classes })
}
```

### P4

**P4**: `ClassDecl -> class ClassName (extends ClassName)? ( (VarDecl)* ) ( [ (ConstructorDecl)+ ] )? { (MethodDecl)* }`
```rust
fn parse_class_decl(&mut self) -> Result<ClassDecl, ParseError> {
    let line = self.peek1().line;
    self.expect(TokenKind::KwClass, ErrorCode::EMalformedClassDecl)?;
    let class_name = self.parse_class_name()?; // P44: ClassName -> Identifier

    let extends = if self.check(&TokenKind::KwExtends) {
        // "(extends ClassName)?"
        self.advance();
        Some(self.parse_class_name()?) // P44: ClassName -> Identifier
    } else {
        None
    };

    self.expect(TokenKind::LParen, ErrorCode::EMalformedClassDecl)?;
    // "( (VarDecl)* )" -- inlined, same pattern as the (MethodDecl)* and
    // (ConstructorDecl)+ loops below: no dup-check here (E_DUPLICATE_FIELD is
    // checker-phase), so there's nothing beyond parsing for a wrapper to add.
    let mut fields = Vec::new();
    while !self.check(&TokenKind::RParen) {
        fields.push(self.parse_var_decl()?); // see P11
    }
    self.expect(TokenKind::RParen, ErrorCode::EMalformedClassDecl)?;

    // "( [ (ConstructorDecl)+ ] )?" -- the whole bracket section is optional,
    // but once `[` is seen at least one ConstructorDecl is required (+, not *).
    let constructors = if self.check(&TokenKind::LBracket) {
        self.advance();
        if self.check(&TokenKind::RBracket) {
            return Err(new_parse_error(
                ErrorCode::EMalformedClassDecl,
                self.peek1().line,
                "empty [ ] constructor section",
            ));
        }
        let mut parsed_constructors = Vec::new();
        while !self.check(&TokenKind::RBracket) {
            parsed_constructors.push(self.parse_constructor_decl(&class_name)?); // see P5/P6
        }
        self.advance();
        parsed_constructors
    } else {
        Vec::new()
    };

    self.expect(TokenKind::LBrace, ErrorCode::EMalformedClassDecl)?; // "{ (MethodDecl)* }"
    let mut methods = Vec::new();
    while !self.check(&TokenKind::RBrace) {
        methods.push(self.parse_method_decl()?); // see P7
    }
    self.advance();

    Ok(ClassDecl { class_name, extends, fields, constructors, methods, line })
}
```

No check is needed here for a misplaced `(`/`[` (or anything else) after the method body closes: `parse_program`'s loop calls `parse_class_decl` again for any non-`Eof` token, and that call's very first line, `self.expect(TokenKind::KwClass, ErrorCode::EMalformedClassDecl)`, already reports `EMalformedClassDecl` for *any* token that isn't `class` — including `(`, `[`, or literally anything else. A dedicated check here (there was one earlier) only changed the error *message*, never the code, so it was pure redundancy once the leading keyword-check was itself fixed to use `EMalformedClassDecl` instead of the generic sentinel.

### P5, P6

**P5**: `ConstructorDecl -> ClassName ( (Formals)? ) { ( this ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }`
**P6**: `ConstructorDecl -> ClassName ( (Formals)? ) { ( super ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }`

One nonterminal, same shape, differing only in the delegation keyword — one function, not two. Both snippets below together implement both P5 and P6; the this/super branch is inside `parse_constructor_delegation`.

```rust
fn parse_constructor_decl(&mut self, class_name: &str) -> Result<ConstructorDecl, ParseError> {
    let line = self.peek1().line;
    let constructor_name = self.parse_class_name()?; // P44, via P5/P6: ClassName
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
    self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?;
    let formals = if self.check(&TokenKind::RParen) {
        Vec::new() // "(Formals)?" -- absent
    } else {
        self.parse_formals()?
    };
    self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;

    // "{ ( this/super ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }" -- inlined,
    // same reason as P7's body: this is the only call site for a
    // ConstructorDecl's own body, nothing to share it with.
    let body_line = self.peek1().line;
    self.expect(TokenKind::LBrace, ErrorCode::EParsePhaseOther)?; // "{"
    let delegation = self.parse_constructor_delegation()?; // "( this/super (...) ; )?"
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
    let stmts = self.parse_var_decls_and_stmts(&mut ctx, StmtArity::ZeroOrMore)?; // "(VarDecl)* (Stmt)*"
    self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?; // "}"
    let body = BodyScope { locals, stmts, line: body_line };
    Ok(ConstructorDecl { formals, delegation, body, line })
}

// P5: ConstructorDecl -> ClassName ( (Formals)? ) { ( this ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }
// P6: ConstructorDecl -> ClassName ( (Formals)? ) { ( super ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }
fn parse_constructor_delegation(&mut self) -> Result<Option<ConstructorDelegation>, ParseError> {
    let line = self.peek1().line;
    let is_this = self.check(&TokenKind::KwThis);
    let is_super = self.check(&TokenKind::KwSuper);
    if !is_this && !is_super {
        return Ok(None); // prefix absent -- legal, both productions mark it "?"
    }
    if self.peek2().kind != TokenKind::LParen {
        return Ok(None); // e.g. this.foo() — not a delegation call
    }
    self.advance(); // this/super
    self.advance(); // "("
    let args = if self.check(&TokenKind::RParen) {
        Vec::new() // "(Actuals)?" -- absent
    } else {
        self.parse_actuals()?
    };
    self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
    self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?; // ";"
    Ok(Some(if is_this {
        ConstructorDelegation::ThisCall(args, line) // P5: ConstructorDecl -> ClassName ( (Formals)? ) { ( this ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }
    } else {
        ConstructorDelegation::SuperCall(args, line) // P6: ConstructorDecl -> ClassName ( (Formals)? ) { ( super ( (Actuals)? ) ; )? (VarDecl)* (Stmt)* }
    }))
}
```

Note: neither P5 nor P6 writes `<Block>` — see P12 for why that matters.

### P7

**P7**: `MethodDecl -> Type MethodName ( (Formals)? ) { (VarDecl)* (Stmt)+ }`
```rust
fn parse_method_decl(&mut self) -> Result<MethodDecl, ParseError> {
    let line = self.peek1().line;
    let return_type = self.parse_type()?; // P7: Type
    let method_name = self.parse_method_name()?; // P45: MethodName -> Identifier
    self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?;
    let formals = if self.check(&TokenKind::RParen) {
        Vec::new() // "(Formals)?" -- absent
    } else {
        self.parse_formals()?
    };
    self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
    // "{ (VarDecl)* (Stmt)+ }" -- written inline in P7 itself, not as a
    // reference to <Block> (P12). See P12 for why that's a real distinction.
    // Inlined here rather than factored into its own function: this is P7's
    // only call site for its own body, so there's nothing else to share it
    // with -- unlike parse_var_decls_and_stmts (P12's own listing), which
    // genuinely has three call sites.
    let body_line = self.peek1().line;
    self.expect(TokenKind::LBrace, ErrorCode::EParsePhaseOther)?; // "{"
    let mut locals = Vec::new();
    let mut ctx = ParseContext { locals: &mut locals, constructor: None };
    let stmts = self.parse_var_decls_and_stmts(&mut ctx, StmtArity::OneOrMore)?; // "(VarDecl)* (Stmt)+"
    self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?; // "}"
    let body_scope = BodyScope { locals, stmts, line: body_line };
    Ok(MethodDecl {
        return_type,
        method_name,
        formals,
        body: MethodBody::UserDefined(body_scope),
        line,
    })
}
```

### P9

**P9**: `Formals -> Type Identifier ( , Type Identifier)*`
```rust
// Called only when the caller already checked the next token isn't `)` --
// the "(Formals)?" optionality lives at the call site (P5/P6/P7), not here.
fn parse_formals(&mut self) -> Result<Vec<Formal>, ParseError> {
    let mut formals = Vec::new();
    loop {
        let line = self.peek1().line;
        let declared_type = self.parse_type()?; // Type
        let identifier = self.parse_identifier()?; // Identifier
        formals.push(Formal { declared_type, identifier, line });
        if self.check(&TokenKind::Comma) {
            self.advance(); // "( , Type Identifier)*"
        } else {
            break;
        }
    }
    Ok(formals)
}
```

### P10

**P10**: `Actuals -> Expr ( , Expr)*`
```rust
// Same optionality note as parse_formals: called only when the caller
// already knows the next token isn't `)`.
fn parse_actuals(&mut self) -> Result<Vec<Expr>, ParseError> {
    let mut actuals = vec![self.parse_expr()?]; // Expr
    while self.check(&TokenKind::Comma) {
        self.advance(); // "( , Expr)*"
        actuals.push(self.parse_expr()?);
    }
    Ok(actuals)
}
```

### P11

**P11**: `VarDecl -> Type Identifier ( , Identifier)* ;`

One pure function for the production itself, used at two call sites (P4's field-parens, inlined in P4's own snippet above; and the body-position loop shown below) — the same production (P11), reused verbatim in two structural positions. `parse_var_decl` does nothing beyond P11's own grammar: no hoisting, no duplicate-check, no context. Neither call site flattens — each `VarDecl` keeps every identifier it declared, matching P11's own grouped shape.

```rust
fn parse_var_decl(&mut self) -> Result<VarDecl, ParseError> {
    let line = self.peek1().line;
    let declared_type = self.parse_type()?; // Type
    let mut identifiers = vec![self.parse_identifier()?]; // Identifier
    while self.check(&TokenKind::Comma) {
        self.advance(); // "( , Identifier)*"
        identifiers.push(self.parse_identifier()?);
    }
    self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?; // ";"
    Ok(VarDecl { declared_type, identifiers, line })
}
```

The body-position loop over `(VarDecl)*` is not itself a numbered production — it's the leading half of the shared "(VarDecl)* (Stmt)+/*" tail called from P5/P6, P7, and P12 (shown once, in full, at P12 below, since that's the last of the three call sites this walkthrough reaches). It's also the *only* place hoisting and `E_DUPLICATE_LOCAL` happen: every `VarDecl` this loop reads, at any `if`/`while` nesting depth, calls the same `parse_var_decl` above and then checks + pushes into the one shared `ctx.locals`.

### P12

**P12**: `Block -> { (VarDecl)* (Stmt)+ }`

Only cited by name from P14/P15 (if/while) — P7's own body and P5/P6's own body spell the same-looking shape out inline instead, so they are *not* this production; see their own entries above.

```rust
fn parse_block(&mut self, ctx: &mut ParseContext) -> Result<Vec<Stmt>, ParseError> {
    self.expect(TokenKind::LBrace, ErrorCode::EParsePhaseOther)?; // "{"
    let mut nested_ctx = ParseContext {
        locals: &mut *ctx.locals, // reborrow, not a new Vec -- this IS the hoist
        constructor: ctx.constructor.map(|_| ConstructorContext {
            at_top_level: false,
            delegation: None,
        }),
    };
    let stmts = self.parse_var_decls_and_stmts(&mut nested_ctx, StmtArity::OneOrMore)?; // "(VarDecl)* (Stmt)+"
    self.expect(TokenKind::RBrace, ErrorCode::EParsePhaseOther)?; // "}"
    Ok(stmts)
}
```

`delegation` is always reset to `None` here rather than threaded through: it's never read once `at_top_level` is `false`, since `misplaced_delegation_error` only consults `.delegation` inside its `at_top_level` branch.

The AST return type is a bare `Vec<Stmt>`, not a `Block`/`BodyScope` struct: every
`VarDecl` parsed here is diverted into `ctx.locals` (the enclosing method/constructor's
`BodyScope`), so by the time this returns, the "(VarDecl)*" part of P12 has always
already been emptied out elsewhere — nothing is ever left for a `Block`-shaped wrapper
to hold beyond the `Stmt` list.

`parse_block` is the third and last of the three call sites for the shared
"(VarDecl)* (Stmt)+/*" tail (P5/P6 and P7 are the other two, shown at their own
entries above, each just calling it) — shown here in full, since this is where
hoisting and `E_DUPLICATE_LOCAL` actually happen:

```rust
// "(VarDecl)* (Stmt)+" (P7, P12) or "(VarDecl)* (Stmt)*" (P5, P6), per
// `arity` -- shared by P5/P6, P7, and P12. The VarDecl loop is where flat
// hoisting lives: every VarDecl
// read here, at any if/while nesting depth, calls the same parse_var_decl
// (P11) and lands in the one ctx.locals shared across the whole
// method/constructor body via the reborrow in parse_block above.
fn parse_var_decls_and_stmts(
    &mut self,
    ctx: &mut ParseContext,
    arity: StmtArity,
) -> Result<Vec<Stmt>, ParseError> {
    while self.is_start_of_var_decl() {
        let decl = self.parse_var_decl()?; // P11, reused verbatim
        self.hoist(decl, ctx)?;
    }
    let mut stmts = Vec::new();
    while !self.check(&TokenKind::RBrace) {
        stmts.push(self.parse_stmt(ctx)?); // "(Stmt)+" or "(Stmt)*", per `arity`
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

// The sole E_DUPLICATE_LOCAL site. Checks every identifier in `decl` against
// both its own siblings ("int x, x;") and every VarDecl already hoisted into
// ctx.locals, from this body or an enclosing/sibling block, before pushing.
fn hoist(&self, decl: VarDecl, ctx: &mut ParseContext) -> Result<(), ParseError> {
    for (i, identifier) in decl.identifiers.iter().enumerate() {
        // decl.identifiers[..i]: the names in THIS VarDecl seen before this
        // one ("int x, x;" case). The .any(...) call: does ANY VarDecl
        // already in ctx.locals -- from this body or an enclosing/sibling
        // block -- already contain this name?
        let is_duplicate = decl.identifiers[..i].contains(identifier)
            || ctx.locals.iter().any(|existing| existing.identifiers.contains(identifier));
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
```

### P13

**P13**: `Stmt -> return Expr ;`
```rust
// P13-P19: Stmt -> ...
fn parse_stmt(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    if let Some(delegation_err) = self.misplaced_delegation_error(ctx) {
        return Err(delegation_err);
    }

    let line = self.peek1().line;
    match self.peek1().kind.clone() {
        // P13: Stmt -> return Expr ;
        TokenKind::KwReturn => {
            self.advance();
            let expr = self.parse_expr()?;
            self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?;
            Ok(Stmt::Return(expr, line))
        }
        // P14: Stmt -> if ( Expr ) Block else Block
        TokenKind::KwIf => self.parse_if_else_stmt(ctx),
        // P15: Stmt -> while ( Expr ) Block
        TokenKind::KwWhile => self.parse_while_stmt(ctx),
        // P16: Stmt -> break ;
        TokenKind::KwBreak => {
            self.advance();
            self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?;
            Ok(Stmt::Break(line))
        }
        // P18: Stmt -> ;
        TokenKind::Semicolon => {
            self.advance();
            Ok(Stmt::Empty(line))
        }
        // P17: Stmt -> Var = Expr ;  vs.  P19: Stmt -> ObjName . MethodName (...) ;
        // Second token decides: another token isn't possible here (Var is just
        // one Identifier), so it's `=` -> P17, anything else -> P19.
        TokenKind::Ident(_) => {
            if self.peek2().kind == TokenKind::Equals {
                let name = self.parse_var()?; // P50: Var -> Identifier
                self.advance(); // =
                let expr = self.parse_expr()?;
                self.expect(TokenKind::Semicolon, ErrorCode::EParsePhaseOther)?;
                Ok(Stmt::Assign(name, expr, line))
            } else {
                self.parse_call_stmt() // P19, ObjName::Var case (P46)
            }
        }
        // P19 continued: ObjName's this/super/(Expr) alternatives (P47-P49)
        TokenKind::KwThis | TokenKind::KwSuper | TokenKind::LParen => self.parse_call_stmt(),
        other => Err(new_parse_error(
            ErrorCode::EParsePhaseOther,
            line,
            format!("unexpected token starting statement: {:?}", other),
        )),
    }
}
```

### P14

**P14**: `Stmt -> if ( Expr ) Block else Block`

Same `parse_stmt` as P13 above (the `TokenKind::KwIf` arm dispatches here), plus its own function. Named `parse_if_else_stmt`, not `parse_if_stmt`: P14 is one production where `else` is mandatory, not a separate optional case — LO has no bare "if without else" form, so the name says what's actually always parsed here.

```rust
fn parse_if_else_stmt(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let line = self.peek1().line;
    self.advance(); // if
    self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
    let cond = self.parse_expr()?; // Expr
    self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
    let if_body = self.parse_block(ctx)?; // Block (P12)
    self.expect(TokenKind::KwElse, ErrorCode::EParsePhaseOther)?; // "else"
    let else_body = self.parse_block(ctx)?; // Block (P12)
    Ok(Stmt::If(cond, if_body, else_body, line))
}
```

### P15

**P15**: `Stmt -> while ( Expr ) Block`
```rust
fn parse_while_stmt(&mut self, ctx: &mut ParseContext) -> Result<Stmt, ParseError> {
    let line = self.peek1().line;
    self.advance(); // while
    self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
    let cond = self.parse_expr()?; // Expr
    self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
    let body = self.parse_block(ctx)?; // Block (P12)
    Ok(Stmt::While(cond, body, line))
}
```

### P16

**P16**: `Stmt -> break ;`

The `TokenKind::KwBreak` arm of `parse_stmt` — see P13's full listing above.

### P17

**P17**: `Stmt -> Var = Expr ;`

The `TokenKind::Ident(_)` arm's `if self.peek2().kind == TokenKind::Equals` branch of `parse_stmt` — see P13's full listing above.

### P18

**P18**: `Stmt -> ;`

The `TokenKind::Semicolon` arm of `parse_stmt` — see P13's full listing above.

### P19

**P19**: `Stmt -> ObjName . MethodName ( (Actuals)? ) ;`

The `TokenKind::Ident`-else-branch and `TokenKind::KwThis | TokenKind::KwSuper | TokenKind::LParen` arms of `parse_stmt` (P13's listing) both call this:

```rust
fn parse_call_stmt(&mut self) -> Result<Stmt, ParseError> {
    let line = self.peek1().line;
    let obj_name = self.parse_obj_name()?; // ObjName (P46-49)
    let call = self.parse_method_call_suffix(obj_name, line)?; // ". MethodName ( (Actuals)? )"
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
            self.advance();
            Ok(ObjName::This(line)) // P47: ObjName -> this
        }
        TokenKind::KwSuper => {
            self.advance();
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

// ". MethodName ( (Actuals)? )" -- shared verbatim by P19 (caller appends
// ";") and P23 (caller doesn't).
fn parse_method_call_suffix(
    &mut self,
    obj_name: ObjName,
    line: u32,
) -> Result<MethodCall, ParseError> {
    self.expect(TokenKind::Dot, ErrorCode::EParsePhaseOther)?; // "."
    let method_name = self.parse_method_name()?; // P45: MethodName -> Identifier
    self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
    let actuals = if self.check(&TokenKind::RParen) {
        Vec::new() // "(Actuals)?" -- absent
    } else {
        self.parse_actuals()?
    };
    self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
    Ok(MethodCall { obj_name, method_name, actuals, line })
}
```

### P20

**P20**: `Expr -> this`
```rust
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
            self.advance();
            Ok(Expr::Null(line))
        }
        // P22: Expr -> new ClassName ( (Actuals)? )
        TokenKind::KwNew => self.parse_new_expr(),
        // P31: Expr -> Var  vs.  P23 via ObjName's Var alternative (P46)
        TokenKind::Ident(_) => {
            let identifier = self.parse_var()?; // P50: Var -> Identifier
            if self.check(&TokenKind::Dot) {
                let obj_name = ObjName::Var(identifier, line);
                Ok(Expr::Call(self.parse_method_call_suffix(obj_name, line)?))
            } else {
                Ok(Expr::Var(identifier, line))
            }
        }
        // P20: Expr -> this  vs.  P23 via ObjName's this alternative (P47)
        TokenKind::KwThis => {
            self.advance();
            if self.check(&TokenKind::Dot) {
                Ok(Expr::Call(self.parse_method_call_suffix(ObjName::This(line), line)?))
            } else {
                Ok(Expr::This(line))
            }
        }
        // No "Expr -> super" production exists (P20 is `this` only) -- bare
        // `super` always goes to P23 via ObjName's super alternative (P48).
        TokenKind::KwSuper => {
            self.advance();
            Ok(Expr::Call(self.parse_method_call_suffix(ObjName::Super(line), line)?))
        }
        // P25-P30 vs. P23 via ObjName's ( Expr ) alternative (P49)
        TokenKind::LParen => {
            let value = self.parse_paren_expr()?;
            if self.check(&TokenKind::Dot) {
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
```

### P21

**P21**: `Expr -> null`

The `TokenKind::KwNull` arm of `parse_expr` — see P20's full listing above.

### P22

**P22**: `Expr -> new ClassName ( (Actuals)? )`

The `TokenKind::KwNew` arm of `parse_expr` (P20's listing) calls this:

```rust
fn parse_new_expr(&mut self) -> Result<Expr, ParseError> {
    let line = self.peek1().line;
    self.advance(); // "new"
    let class_name = self.parse_class_name()?; // P44: ClassName -> Identifier
    self.expect(TokenKind::LParen, ErrorCode::EParsePhaseOther)?; // "("
    let actuals = if self.check(&TokenKind::RParen) {
        Vec::new() // "(Actuals)?" -- absent
    } else {
        self.parse_actuals()?
    };
    self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
    Ok(Expr::New(class_name, actuals, line))
}
```

### P23

**P23**: `Expr -> ObjName . MethodName ( (Actuals)? )`

`parse_expr`'s `Ident`/`KwThis`/`KwSuper`/`LParen` arms (P20's listing) each check for `Dot` directly and, if present, build their own `ObjName` and call `parse_method_call_suffix` -- resolving P31-vs-P23 and P20-vs-P23 explicitly at each call site rather than through a shared helper, since each caller already holds the exact `ObjName`/bare-value pair it needs and the only decision left is a single `check(&TokenKind::Dot)`.

A second, independent call site is `parse_paren_suffix`'s `check(&TokenKind::Dot)` (P25's listing) -- the same P23 resolution, reached one recursion level down, for a receiver that turned out to be `((x).m())` rather than a cast.

`parse_method_call_suffix` (the ". MethodName ( (Actuals)? )" tail) is shown once, at P19.

### P25

**P25**: `Expr -> ( Expr ? Expr : Expr )`
```rust
// P25-P30: Expr -> ...
fn parse_paren_expr(&mut self) -> Result<Expr, ParseError> {
    let line = self.peek1().line;
    self.advance(); // consume outer '('

    if let Some(op) = Self::to_unop(&self.peek1().kind) {
        // P27: Expr -> ( Unop Expr )
        self.advance(); // "~" or "!"
        let operand = self.parse_expr()?;
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        return Ok(Expr::Unop(op, Box::new(operand), line));
    }

    // A primitive keyword can never start an Expr, so seeing one here is
    // unambiguous before consuming anything -- always a cast.
    if self.is_primitive_type_ahead() {
        self.advance(); // second '('
        let ty = self.try_parse_primitive_type().expect("is_primitive_type_ahead confirmed this");
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        let value = self.parse_expr()?;
        self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
        return Ok(Expr::Cast(ty, Box::new(value), line));
    }

    // Anything else -- P25/P26/P28/P30, or a ClassName-typed P29 cast, which
    // shares its one Identifier token with Var and can't be told apart from
    // lookahead alone. parse_paren_suffix decides which, once this returns.
    let primary_expr = self.parse_expr()?;
    self.parse_paren_suffix(primary_expr, line)
}

fn parse_paren_suffix(&mut self, primary_expr: Expr, line: u32) -> Result<Expr, ParseError> {
    // P29: Expr -> ( ( Type ) Expr ), ClassName Type -- a bare operand
    // immediately follows a Var with no connector, a shape only a cast
    // produces, so primary_expr was really this cast's Type all along.
    if let Expr::Var(identifier, _) = &primary_expr {
        if self.is_start_of_expr() {
            let value = self.parse_expr()?;
            self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
            return Ok(Expr::Cast(Type::Class(identifier.clone()), Box::new(value), line));
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

// P28: Expr -> ( Expr ), if it closes here; else P25/P26/P30 continue it via
// parse_operator_suffix (which itself consumes the ")").
fn parse_paren_close_or_operator(&mut self, value: Expr, line: u32) -> Result<Expr, ParseError> {
    if self.check(&TokenKind::RParen) {
        self.advance();
        Ok(value)
    } else {
        self.parse_operator_suffix(value, line)
    }
}

// first(Expr), i.e. every token that can legally start an Expr (P20's own
// dispatch, minus its error fallback). Tests only the current token; not a
// peek_ahead call, no lookahead beyond it.
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

// P25/P26/P30: Expr -> ... (continuation after the caller's own first Expr)
fn parse_operator_suffix(&mut self, primary_expr: Expr, line: u32) -> Result<Expr, ParseError> {
    match self.peek1().kind.clone() {
        TokenKind::Question => {
            // P25: Expr -> ( Expr ? Expr : Expr )
            self.advance();
            let if_expr = self.parse_expr()?;
            self.expect(TokenKind::Colon, ErrorCode::EParsePhaseOther)?;
            let else_expr = self.parse_expr()?;
            self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
            Ok(Expr::Ternary(Box::new(primary_expr), Box::new(if_expr), Box::new(else_expr), line))
        }
        TokenKind::KwInstanceof => {
            // P30: Expr -> ( Expr instanceof ClassName )
            self.advance();
            let class_name = self.parse_class_name()?; // P44: ClassName -> Identifier
            self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
            Ok(Expr::InstanceOf(Box::new(primary_expr), class_name, line))
        }
        other => {
            if let Some(op) = Self::to_binop(&other) {
                // P26: Expr -> ( Expr Binop Expr ), Binop -> [+-*/%&|<>=] (P33)
                self.advance();
                let right = self.parse_expr()?;
                self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?;
                Ok(Expr::Binop(Box::new(primary_expr), op, Box::new(right), line))
            } else {
                Err(new_parse_error(
                    ErrorCode::EParsePhaseOther,
                    self.peek1().line,
                    format!("expected ?, instanceof, an operator, or ) here, found {:?}", other),
                ))
            }
        }
    }
}
```

### P26

**P26**: `Expr -> ( Expr Binop Expr )`

Same `parse_paren_expr` / `parse_operator_suffix` as P25 above (the `other` arm, guarded by `to_binop`), plus the P33 lookup table:

```rust
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
```

### P27

**P27**: `Expr -> ( Unop Expr )`

The `Tilde`/`Bang` check at the top of `parse_paren_expr` — see P25's full listing above.

### P28

**P28**: `Expr -> ( Expr )`

One spot: `parse_paren_close_or_operator`'s `if self.check(&TokenKind::RParen) { ... }` (P25's listing) -- reached both from an ordinary parenthesized value and from a `Var` that turned out not to be a cast after all (e.g. `((Ident))`).

### P29

**P29**: `Expr -> ( ( Type ) Expr )`
```rust
// LL(2) lookahead disambiguating a primitive-Type P29 from P25/P26/P28/P30.
fn is_primitive_type_ahead(&self) -> bool {
    self.check(&TokenKind::LParen)
        && matches!(
            self.peek2().kind,
            TokenKind::KwInt | TokenKind::KwBool | TokenKind::KwString | TokenKind::KwVoid
        )
}
```

The primitive case is fully resolved by `is_primitive_type_ahead`, shown in full at P25 above, since a primitive keyword can't be mistaken for anything else. The `ClassName` case has no equivalent upfront lookahead function at all -- there's nothing to pre-detect, since `(Identifier)` is legal as either a `Var` or a cast's `Type` and only the token following it tells them apart. That case is resolved after the fact, by the `if let Expr::Var(identifier, _) = &primary_expr { if self.is_start_of_expr() { ... } }` guard in `parse_paren_suffix` (P25's listing) -- see "LL(2) decision points," decision 3, for why this is structured as a post-parse check rather than more lookahead.

### P30

**P30**: `Expr -> ( Expr instanceof ClassName )`

The `TokenKind::KwInstanceof` arm of `parse_operator_suffix` — see P25's full listing above.

### P31

**P31**: `Expr -> Var`

The `TokenKind::Ident(_)` arm of `parse_expr`, no-`Dot` case — see P20's full listing above.

### P32

**P32**: `Expr -> Literal`

`Literal` is its own nonterminal (P40-P43 below are its four alternatives), so it gets its own procedure rather than being folded into `parse_expr` directly:

```rust
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
```

Called from `parse_expr`'s `Num`/`KwTrue`/`KwFalse`/`Str` arms — see P20's listing above.

### P33

**P33**: `Binop -> [+-*/%&|<>=]`

`to_binop` — see P26's full listing above.

### P34

**P34**: `Unop -> [~!]`

`Unop` is its own nonterminal, so — matching P33's `to_binop` — it gets its own classifier rather than an inline token check:

```rust
// P34: Unop -> [~!]
fn to_unop(kind: &TokenKind) -> Option<Unop> {
    match kind {
        TokenKind::Tilde => Some(Unop::Neg),
        TokenKind::Bang => Some(Unop::Not),
        _ => None,
    }
}
```

Called from `parse_paren_expr` — see P25's full listing above.

### P35

**P35**: `Type -> void`
```rust
// P35/P37/P38/P39: Type -> void | int | bool | String
fn try_parse_primitive_type(&mut self) -> Option<Type> {
    let ty = match &self.peek1().kind {
        TokenKind::KwInt => Type::Int,       // P37: Type -> int
        TokenKind::KwBool => Type::Bool,     // P38: Type -> bool
        TokenKind::KwString => Type::String, // P39: Type -> String
        TokenKind::KwVoid => Type::Void,     // P35: Type -> void
        _ => return None,
    };
    self.advance();
    Some(ty)
}
```

### P36

**P36**: `Type -> ClassName`
```rust
// P35-P39: Type -> ...
fn parse_type(&mut self) -> Result<Type, ParseError> {
    if let Some(ty) = self.try_parse_primitive_type() {
        return Ok(ty);
    }
    // P36: Type -> ClassName
    let class_name = self.parse_class_name()?; // P44: ClassName -> Identifier
    Ok(Type::Class(class_name))
}
```

### P37

**P37**: `Type -> int`

`try_parse_primitive_type`'s `KwInt` arm — see P35's full listing above.

### P38

**P38**: `Type -> bool`

`try_parse_primitive_type`'s `KwBool` arm — see P35's full listing above.

### P39

**P39**: `Type -> String`

`try_parse_primitive_type`'s `KwString` arm — see P35's full listing above.

### P40

**P40**: `Literal -> Num`

The `TokenKind::Num(n)` arm of `parse_literal` — see P32's full listing above.

### P41

**P41**: `Literal -> true`

The `TokenKind::KwTrue` arm of `parse_literal` — see P32's full listing above.

### P42

**P42**: `Literal -> false`

The `TokenKind::KwFalse` arm of `parse_literal` — see P32's full listing above.

### P43

**P43**: `Literal -> String`

The `TokenKind::Str(s)` arm of `parse_literal` — see P32's full listing above.

### P44

**P44**: `ClassName -> Identifier`

`ClassName`, `MethodName` (P45), and `Var` (P50) are three distinct nonterminals — each derives from `Identifier` and adds no shape of its own, but "identical shape" isn't "the same nonterminal": each gets its own procedure, all three built on the one shared token-level primitive that actually consumes an `Identifier` token (`parse_identifier`, shown in "Token-level helpers" below, since P53 itself is lexical, not a parser production).

```rust
// P44: ClassName -> Identifier
fn parse_class_name(&mut self) -> Result<String, ParseError> {
    self.parse_identifier()
}
```

### P45

**P45**: `MethodName -> Identifier`

```rust
// P45: MethodName -> Identifier
fn parse_method_name(&mut self) -> Result<String, ParseError> {
    self.parse_identifier()
}
```

### P46

**P46**: `ObjName -> Var`

The `TokenKind::Ident(_)` arm of `parse_obj_name` — see P19's full listing above. Calls `parse_var` (P50).

### P47

**P47**: `ObjName -> this`

The `TokenKind::KwThis` arm of `parse_obj_name` — see P19's full listing above.

### P48

**P48**: `ObjName -> super`

The `TokenKind::KwSuper` arm of `parse_obj_name` — see P19's full listing above.

### P49

**P49**: `ObjName -> ( Expr )`

The `TokenKind::LParen` arm of `parse_obj_name` — see P19's full listing above.

### P50

**P50**: `Var -> Identifier`

```rust
// P50: Var -> Identifier
fn parse_var(&mut self) -> Result<String, ParseError> {
    self.parse_identifier()
}
```

### P51, P52, P53

**P51**: `Num -> ([0-9])+`
**P52**: `String -> " (string char)* "`
**P53**: `Identifier -> [a-zA-Z]([a-zA-Z0-9'_'])*`

Lexical productions — implemented by the lexer (`lexer.rs`/`lexer_design.md`), not the parser. The parser only ever consumes the already-classified `Token::Num`/`Token::Str`/`Token::Ident` produced by `tokenize`.

---

## Token-level helpers

Shared machinery every function above calls into; not itself a numbered production, so not part of the P1–P53 walkthrough, but included here since several entries reference it.

```rust
fn peek1(&self) -> &Token {
    &self.tokens[self.pos]
}

// A fixed function, not one parameterized by an offset -- a bare
// `peek_ahead(n: usize)` could be called with any n, silently exceeding the
// bound this file documents. Only this one exists; looking further ahead
// requires visibly adding another.
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

// P53: Identifier -> [a-zA-Z]([a-zA-Z0-9'_'])* is lexical, not a parser
// production -- this consumes the lexer's already-classified Identifier
// token. Shared by parse_class_name (P44), parse_method_name (P45), and
// parse_var (P50): three distinct nonterminals, each with its own procedure,
// all built on this one token-level primitive.
fn parse_identifier(&mut self) -> Result<String, ParseError> {
    let line = self.peek1().line;
    match self.peek1().kind.clone() {
        TokenKind::Ident(identifier) => {
            self.advance();
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

// The LL(2) lookahead that tells P11 (VarDecl) apart from P17 (assignment)
// and P19 (call statement) at the top of a body.
fn is_start_of_var_decl(&self) -> bool {
    match &self.peek1().kind {
        TokenKind::KwInt | TokenKind::KwBool | TokenKind::KwString | TokenKind::KwVoid => true,
        TokenKind::Ident(_) => matches!(self.peek2().kind, TokenKind::Ident(_)),
        _ => false,
    }
}
```

Plus one free function outside `impl Parser`:

```rust
fn new_parse_error(code: ErrorCode, line: u32, message: impl Into<String>) -> ParseError {
    ParseError { code, line, message: message.into() }
}
```
