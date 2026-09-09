# Parser Design — LO (LiveOak) P1

Scope: `Vec<Token>` in, `Program` (the AST) out. Lexing is `lexer_design.md`. Type
checking, the interpreter, and codegen are separate documents — referenced here only
where they constrain a parsing or AST-shape decision.

---

## Decisions

| Decision | Why |
|---|---|
| Recursive descent, one function per grammar nonterminal | Grammar tables (P1–P53) are written to translate this way directly. Keeps the parser "recognizably the grammar" for code review. |
| Build the AST directly while parsing — no concrete syntax tree stage, ever | Every compound expression in the grammar is mandatorily fully parenthesized, so there's no operator-precedence ambiguity to defer — the one thing a CST would normally buy you doesn't apply here. No source document mandates a particular AST shape or discusses a CST at all; skipping one is an inferred conclusion from the grammar's full-parenthesization property and the handout's own framing that the AST is the team's to design. A CST also wouldn't grant more lookahead than direct-to-AST parsing does — both are built by the same forward-moving recursive descent, and the token stream is already fully materialized in a `Vec<Token>` either way. |
| Target the LO-4 grammar only, one entry point (`Program → ClassDecl*`) | The grammar table itself shows LO-2's `Program → MethodDecl*` and LO-0/1's `Program → Body` are structurally absent at LO-3/4 ("empty in LO-3"). The real `lo-testing` repo confirms LO-2's bare top-level tests are explicitly optional practice scaffolding, not part of what the graded LO-4 parser needs to accept. |
| 2 tokens of lookahead (LL(2)) via cursor + `peek`/`peek2` over the pre-built `Vec<Token>` | Two real ambiguous points exist in the grammar; both resolve with small, bounded, purely structural lookahead — no backtracking anywhere. See below. |
| Fail-fast: stop at the first parse error | Not stated as policy anywhere in the handout, the language reference, or `error-codes.md` — this is a team assumption adopted for implementation simplicity, not a confirmed requirement (`error-codes.md` arguably leans the other way, explicitly permitting one error to emit multiple codes when it "genuinely spans phases"). Doesn't affect grading correctness, since the harness only substring-matches the declared code in stderr regardless of which phase raised it — but does affect code-review legibility ("is the parser recognizably the grammar," "is the checker's phase structure legible"), which is a real, separate reason to keep the discipline. |
| Declaration pre-scan (class table, vtable layout) is NOT part of the parser | The class/method-table pre-scan is the type checker's Pass 1, run over the finished AST. Vtable layout specifically isn't clearly assigned to a single pass by the handout — it describes signature-collection and vtable-layout-collection as one undifferentiated pre-scan happening "before emission," without stating whether that's literally Pass 1 or a separate step. Either way, neither is parser work — parsing's job ends at "tokens in, `Program` out" — and vtable slot assignment needs the full class hierarchy (parent-first slot inheritance), so it can't run before Pass 1's cycle/hierarchy checks succeed regardless of which pass it's formally part of. The type-checker design should settle whether vtable layout is literally inside Pass 1 or a dependent step right after it. |
| The synthetic `Input`/`Output` preamble classes are NOT built by the parser | Never parsed from text — two `ClassDecl` values constructed directly in Rust by a small named step, `add_io_classes(program: Program) -> Program`, that takes the parser's finished output and returns it with both classes added, immediately before the type checker runs. Lives in its own module, not in `parser.rs` and not a `Parser` method — it has no tokens, no lookahead, nothing parser-shaped about it. See "Preamble injection" below for the concrete shape. |
| `Main`/`Input`/`Output`/reserved-variable-name checks are NOT parser-level, except `String` | `String` is a lexical keyword, so `extends String` fails to parse as `E_RESERVED_KEYWORD_AS_IDENTIFIER` for free. `Main`/`Input`/`Output`/`in`/`out`/`err` are ordinary identifiers to the parser; reservation is checked at name resolution. `Main` is explicitly *permitted*, just shape-constrained by its own dedicated entry-point codes. |
| Constructor name must equal enclosing class name — checked in the parser | `E_MALFORMED_CONSTRUCTOR` is a parse-phase code, with trigger text: *"A constructor declaration whose name does not equal the enclosing class's name."* The enclosing class's name is already local parser state the moment a constructor is parsed. |
| Every local in a method or constructor is hoisted to that method/constructor's flat scope, regardless of how deeply nested in `if`/`while` it's textually declared — permanent, per course staff | Three explicit consequences: (1) every local holds its type's default value from the start of the body, even before its declaring block runs; (2) declaring the same local name twice anywhere in one body — including once per `if`/`else` arm — is unconditionally illegal, `E_DUPLICATE_LOCAL`, regardless of matching types; (3) a use may textually precede its declaration. |
| Hoisting is done by the **parser**, at parse time — not deferred to the checker | The AST mirroring exact nested position buys nothing once "a use may precede its declaration" holds — only each node's own `line` matters for diagnostics, and that travels with the node regardless of where it sits in the tree. Flattening at parse time makes `E_DUPLICATE_LOCAL` a natural parser-level check: the parser already builds one flat name collection per method as a direct byproduct, so checking for a repeat is available immediately, with no forward-reference or cross-method dependency needed. This also matches how real compiler backends already treat function-local declarations — LO's own semantics (every local default-valued from body entry, regardless of whether its declaring block ran) mirror the "hoist every declaration to the function's entry block" convention used before register-promotion (e.g. LLVM's `mem2reg`): `BodyScope.locals` is already shaped like the entry-block declaration list a 3-address-code lowering pass would want to build for itself in P2. |
| The "check it in the parser if the info is local" principle only applies where no published categorization exists yet | `E_LOCAL_SHADOWS_FORMAL` and `E_BREAK_OUTSIDE_LOOP` are both explicitly filed under well-formedness (checker-phase) in `error-codes.md`, despite being just as locally trackable during parsing as `E_DUPLICATE_LOCAL` is (a "current enclosing loop" flag and a "does this name match a formal" check are exactly as local as a flat-locals collection). The rule: respect the published phase categorization when one exists; only make an independent local-info-based call for codes that are genuinely unpublished (like `E_DUPLICATE_LOCAL`). Neither of these checks a use resolves to — both are purely declaration-vs-declaration name collisions, never reference resolution, so none of this creeps toward the parser resolving names in the sense that stays forbidden. |
| Rejecting empty `[ ]` constructor brackets uses `E_MALFORMED_CONSTRUCTOR`'s sibling code, `E_MALFORMED_CLASS_DECL` | Trigger text: *"has them in the wrong order, or has empty `[ ]` brackets."* Not a generic/unspecified syntax error — cite this code specifically. |
| `if`/`while` bodies are plain `Vec<Stmt>`, not a wrapper type | Once every `VarDecl` is diverted to the enclosing `BodyScope` during parsing, there's nothing left for an `if`/`while` body to hold besides statements — no reason for a struct. |

---

## Naming

A few node names were revisited for clarity. General rules applied: if the language
reference itself already uses a term, keep it (consistency with course vocabulary
matters for viva voce); if a name was our own invented shorthand and it doesn't
self-explain, prefer something more descriptive over something merely shorter; when a
name requires the reader to make an inferential leap, prefer the more literal
alternative even if it's marginally less precise.

| Old name | Final name | Why |
|---|---|---|
| `Delegation` | `OtherConstructorCall` | `Delegation` is the reference's own word, but as a bare type name doesn't say what's being delegated. The concrete distinguishing fact: this calls *another* constructor (sibling via `this`, parent via `super`) *on the object already being constructed* — no allocation, unlike `New`. `OtherConstructorCall` states that directly. `ConstructorForward` was considered and rejected — "forward" doesn't say what's being forwarded, the same vagueness `Delegation` had. |
| `MethodBody::Intrinsic` / `IntrinsicOp` | `MethodBody::Io` / `IoOp` | "Intrinsic" and "Preamble" (the reference's own term, considered as an intermediate step) both require already knowing course-specific jargon. All 8 of these operations (4 reads, 4 prints) are genuinely I/O, so `IoOp` is both more self-explanatory and more precise than either. **Capitalization matters here**: `IoOp`, not `IOOp` — Rust API guidelines and `clippy`'s default `upper_case_acronyms` lint treat acronyms in type names as one word (`Uuid` not `UUID`, `Http` not `HTTP`), and "idiomatic use of your chosen language" is part of the graded code-review bar. |
| `Block` | `BodyScope` | No longer a generic "curly braces" container — it has exactly one job: hold every local for one method or constructor body. `LocalScope` and `CallableScope` were considered. `CallableScope` is arguably more precise (methods and constructors are both things you invoke) but requires an inferential leap from "callable" to "oh, that means method-or-constructor" — `BodyScope` says what it is with no leap required. The only real ambiguity risk (confusion with an `if`/`while` "body") is prose-level only, not a type-level collision: `if`/`while` bodies are plain `Vec<Stmt>` with no dedicated type at all. |
| `Receiver` | kept | Standard OOP terminology *and* the reference's own word ("the receiver's static type"). Specifically avoid `Callee` if tempted toward a synonym — that means "the function being called," not "the object it's called on," and would be a real misnomer here. |
| `Stmt::CallStmt`, `MethodCall` struct | kept, unchanged | `MethodCall` is deliberately narrow — it's only ever a real instance-method dispatch with a receiver, never a constructor call. `New` (allocating) and `OtherConstructorCall` (forwarding within construction) are separately-shaped types that never wrap it, so the name being narrow is accurate, not misleading. Renaming `Expr::Call`/`Stmt::CallStmt` to `Expr::MethodCall`/`Stmt::MethodCallStmt` for symmetry was considered and rejected — that would read as "the one place all calls go," which is exactly wrong given `New` and `OtherConstructorCall` exist as separate call-like constructs. |
| `ty: Type` (on `VarDecl`, `Param`) | `declared_type: Type` | Plain `type` isn't available — it's a reserved Rust keyword, so `type: Type` would need the `r#type` raw-identifier escape, which reads worse than `ty`, not better. `declared_type` is unambiguous, accurate (it's the type as written in source, not something inferred), and avoids the keyword collision. |

---

## Grammar → function mapping

| Nonterminal | Function | Produces |
|---|---|---|
| `Program` | `parse_program` | `Program` |
| `ClassDecl` | `parse_class_decl` | `ClassDecl` |
| `ConstructorDecl` | `parse_constructor_decl` | `ConstructorDecl` |
| `MethodDecl` | `parse_method_decl` | `MethodDecl` |
| `VarDecl` (method/constructor locals) | `parse_var_decl` | `VarDecl` (routed into the enclosing `BodyScope`, not returned to its lexical position) |
| `Block` (method body) | `parse_method_body_scope` | `BodyScope` |
| constructor body (**not** the `Block` nonterminal — see note below) | `parse_constructor_body_scope` | `BodyScope` |
| `Block` (if/while body) | `parse_nested_block` | `Vec<Stmt>` |
| `Stmt` | `parse_stmt` | `Stmt` |
| `Expr` | `parse_expr` | `Expr` |
| `ObjName` (receiver) | `parse_receiver` | `Receiver` |
| `Formals` | `parse_formals` | `Vec<Param>` |
| `VarDecl` (class field-parens section) | `parse_field_list` | `Vec<Param>` |
| `Actuals` | `parse_actuals` | `Vec<Expr>` |
| `Type` | `parse_type` | `Type` |
| `Literal` | folded into `parse_expr` | `Expr::Num` / `Bool` / `Str` |

`parse_formals`/`parse_actuals` are only called when the current token isn't already `)` — the empty-list case comes from the *caller* skipping the optional `(Formals)?`/`(Actuals)?`, not from either production handling zero internally.

**A class's field-parens section is not `Formals` — it's built from `VarDecl`.** P4 gives `⟨ClassDecl⟩ → class ⟨ClassName⟩ (extends ⟨ClassName⟩)? ( (⟨VarDecl⟩)* ) ...` — the field list is a sequence of `VarDecl`s (P11: `⟨Type⟩ ⟨Identifier⟩ ( , ⟨Identifier⟩)* ;`), the exact same production used for hoisted locals, which explicitly allows several comma-separated names sharing one type (`int a, b;`) — the conformance suite uses this shape for locals (e.g. `LO-3/ValidPrograms/test_44.lo`: `int a, b;`), and the grammar permits the identical shape in field position, and the corpus exercises it directly: `LO-3/ValidPrograms/test_36.lo:17` declares `class Fiver(int a, b; bool x, y; String str;)` — three `VarDecl`s flattening to five `Param`s. This is a *different* production from `Formals` (`⟨Type⟩ ⟨Identifier⟩ (, ⟨Type⟩ ⟨Identifier⟩)*` — no grouping, one type per name, no semicolons), whose one-name-per-entry shape happens to match `Param` directly — an easy but wrong model to reach for when parsing fields. `parse_field_list` parses each `VarDecl` (type + comma-separated names, terminated by `;`, zero or more of them) the same way `parse_var_decl` does for locals, then flattens every name into its own `Param` entry — the same per-name expansion required for `BodyScope.locals` (see the `VarDecl` struct's own comment on this). `E_DUPLICATE_FIELD` needs the identical per-name-not-per-node duplicate check as `E_DUPLICATE_LOCAL`, checked across the flattened `Vec<Param>`, not across the pre-flattening `VarDecl` sequence.

**Method bodies and constructor bodies are not the same grammar production, and must not be parsed by one shared function that treats them identically.** A `MethodDecl` body is literally `⟨Block⟩` (P12: `{ (VarDecl)* (Stmt)+ }` — **`Stmt+`, at least one statement required**). A `ConstructorDecl` body is a *different* inline production (P5/P6: `{ (this/super(Actuals?);)? (VarDecl)* (Stmt)* }`) — **`Stmt*`, zero statements allowed** — plus the optional leading delegation call (see decision point 5, below) that `Block` doesn't have at all. `parse_method_body_scope` enforces "at least one `Stmt`"; `parse_constructor_body_scope` consumes the optional delegation call first (as its own explicit step, checking for the two delegation error codes), then allows zero or more statements. A single shared function parameterized only by a "how many statements minimum" flag would still need to know about the delegation-call step being constructor-only, so two separate functions is the more honest shape, even though most of their VarDecl/Stmt-collection logic is otherwise identical and can share a common inner helper (`parse_locals_then_stmts`, parameterized by the minimum statement count).

**Threading the enclosing scope:** both body-parsing functions establish a handle to the `BodyScope` being built (its `locals: Vec<VarDecl>`) and pass it down through every recursive call into `parse_stmt`/`parse_nested_block`, however deeply `if`/`while` nest. Every `VarDecl` encountered at any depth is pushed onto that same handle, never attached to the immediate `Vec<Stmt>` it's lexically inside.

**This threading also has to work across *sibling* blocks, not just nested ones.** Two sequential (not nested) `while` loops in the same method sharing a duplicate local name must still be caught:
```
while (a) { int x; ... }
while (b) { int x; ... }   // must ALSO be E_DUPLICATE_LOCAL
```
This works "for free" if the handle is a genuine live mutable reference threaded by identity through every recursive call. It does **not** work if implemented as a "parse a block, return the locals it found, caller merges the returned list into its own" pattern instead — that pattern happens to also work correctly for nesting (since the caller still merges before continuing), but is a fundamentally different implementation than "one shared mutable collection," and is easy to reach for without realizing it's more code, more error-prone on merge timing, and easier to accidentally scope per-block instead of per-method. Use a shared mutable reference, not return-and-merge.

Declaring the same name in each arm of one `if`/`else` is caught correctly under sequential recursive descent, since arm 1 fully completes — pushing its locals to the shared handle — before arm 2 begins parsing.

---

## LL(2) decision points

All resolve with small, bounded, forward-only lookahead; none need backtracking or a semantic symbol table.

### 1. `VarDecl` vs. assignment `Stmt` vs. call `Stmt`

An `Ident` could start `Foo x;` (`VarDecl`), `x = 5;` (assignment), or `x.foo();` (call) — all share a one-token prefix. The second token resolves it by pure grammar shape, no semantic information needed:

| Second token | Means |
|---|---|
| another `Ident` | `VarDecl` continues — no other production has `Ident Ident` as its first two tokens |
| `=` | assignment `Stmt` |
| `.` | call `Stmt` |

### 2. `Stmt` doesn't only start with `Ident` — `this`, `super`, and `(` are also legal statement starts, and there is NO expression-statement production

`Stmt → ObjName.MethodName(Actuals);` means `ObjName` can be `Var` (an `Ident`), `this`, `super`, or `(Expr)` — so a statement can legally begin with any of those four token shapes, not just `Ident`. Each of `this`, `super`, `(` unambiguously means "this must be a call statement" the moment it's seen (nothing else can start a `Stmt`), so no extra lookahead is needed there. **The trap worth naming explicitly: there is no expression-statement production anywhere in this grammar.** `(x + 1);` is a syntax error — a bare parenthesized expression is never legal as a standalone statement. The only way a `(`-led statement is legal is if it's a receiver that gets chased by `.MethodName(...)`, e.g. `(new Circle(5)).area();` or `((Cat) a).purr();`. The rule: statement-start `(` is legal only when parsing it all the way through resolves to a `Receiver::Computed` immediately followed by `.MethodName(...)`; if it doesn't reach a trailing method call, it's an unexpected-token parse failure, not a valid statement.

### 3. Bare `this`/`super` vs. `this.foo()`/`super.foo()` — same trick as decision point 1, one level down

At the expression level, `this` alone (P20) and `this.foo()` (via `Receiver::This` feeding into `MethodCall`) share the one-token `this` prefix — resolved by peeking the next token: `.` continues into a method call, anything else means the bare `this` expression is complete. Same mechanism as decision point 1, applied to `this`/`super` specifically.

### 4. The general dispatch for anything starting with `(`, then the cast-specific case within it

Once the parser sees a `(` starting an `Expr`, the general algorithm is: peek the current token; `~`/`!` means a unop (P27), done; anything else that can start an `Expr` means parse it via a normal recursive `parse_expr()` call, then branch on whatever token comes next — `?` → ternary (P25), a `Binop` symbol → binop (P26), `instanceof` → `instanceof` (P30), the closing `)` → plain parenthesized expression (P28). `((x) instanceof Circle)` is a legal, ordinary case of this same general dispatch: parse `(x)` as a normal sub-expression, see `instanceof` next, consume it and the following `ClassName`, done. It fits the general algorithm above; it doesn't need special-casing the way the cast form does.

**The one case inside this general dispatch that genuinely is special: `(` immediately followed by another `(`.** `( ( Type ) Expr )` (a cast) and `( Expr )` where `Expr` itself starts with `(` (an ordinary nested sub-expression, which is what the general algorithm above already handles for every *other* leading token) share this specific `( (` prefix. A primitive keyword in the type position (`int`/`bool`/`String`/`void`) is unambiguous immediately — those can never start an `Expr`. A bare identifier there is genuinely ambiguous at the token level (`((Circle) obj)` and `((x))` start identically) and is resolved by parsing eagerly and checking what follows:

| What follows the closing `)` | Means |
|---|---|
| the outer closing `)`, immediately | Can't be a cast — nothing followed the tentative type. `((Ident))`, unwrap both layers, return the plain `Var`. |
| a `Binop` symbol or `?` | Was a genuine value, continuing as the left operand of an outer binop/ternary. |
| `instanceof` | Was a genuine value, continuing as the operand of an `instanceof` test — e.g. `((x) instanceof Circle)`. |
| the start of a new expression, no operator bridging | Only a cast has two adjacent expression-like fragments with nothing between them. Reinterpret the `Var`'s name as `Type::Class(name)`, parse what follows as the operand. |

No single decision point needs more than 1–2 tokens of *new* lookahead from wherever the parser currently is — this is several small local decisions strung across recursive calls, not one big lookahead window. Nested casts (`((Animal)((Dog)obj))`) fall out for free through ordinary recursion — the operand of a cast is parsed via a normal recursive `parse_expr()` call, which can resolve to another cast one level down using the identical bounded check. Matches the official course text precisely: *"the tokens `(`, `(`, type-or-keyword, `)`, expression, `)` form the cast pattern."*

This resolution mechanism never consults a name/symbol table — purely structural, keeping the parser consistent with "the parser never resolves or validates names" everywhere else in this design. (A semantic "known class names" table, the C-style "lexer hack," was considered specifically for this and rejected — see Alternate designs.)

Combining a cast and `instanceof` needs *three* leading parens, not two — `(((Dog) x) instanceof Animal)`, since P29's whole `(Type)Expr` pair needs its own wrapping paren and P30 separately wraps `(Expr instanceof ClassName)`. The two-paren form `((Dog) x instanceof Animal)` is not legal grammar. The algorithm above handles this correctly either way: it commits to the cast reading, then fails to find the expected closing `)` (finds `instanceof` instead) — a correct syntax error, not a silent misparse.

### 5. Constructor delegation errors — `E_DELEGATION_BOTH_SUPER_AND_THIS` / `E_DELEGATION_NOT_FIRST_STATEMENT` need explicit parser handling, not implicit fallout

Both codes are parse-phase per `error-codes.md`, with precise trigger text: `E_DELEGATION_BOTH_SUPER_AND_THIS` — *"A constructor body contains both a `super(...)` and a `this(...)` delegation"* (a specific keyword-mismatch case); `E_DELEGATION_NOT_FIRST_STATEMENT` — *"A `super(...)` or `this(...)` delegation appears anywhere other than the optional first statement"* (the general/catch-all misplacement case, covering any position, not just the constructor's own top level). Neither happens automatically as a side effect of ordinary `Stmt` dispatch — the grammar has no `Stmt` production matching a bare `this(...)`/`super(...)` in ordinary statement position at all (`this`/`super` only participate in a `Stmt` via `ObjName.MethodName(...)`, which requires a `.` afterward) — so both must be checked for explicitly, and the check has to run for the entire body, including inside nested `if`/`while` blocks, not just the constructor's own top-level statement list.

The algorithm:
1. At the very start of constructor-body parsing, check whether the current token is `this`/`super` directly followed by `(` (not `.`). If so, consume it as the one legitimate delegation slot, recording which keyword it was. If not, no delegation was declared (this is legal — delegation is always optional); proceed straight to ordinary body parsing.
2. For the rest of the constructor body — its own remaining statements, and every `Stmt`/`Vec<Stmt>` reachable from it via `if`/`while` nesting at any depth — check on every statement position: if the current token is again `this`/`super` directly followed by `(`, that fragment is illegal by construction, since a legitimate first statement (delegation or not) has already been decided in step 1. This check is threaded alongside the `BodyScope` handle through the same recursive descent used for hoisting — one additional "am I inside a constructor body" flag, true for the constructor's own statement list and everything nested inside it — so it fires inside `parse_stmt` itself wherever that flag is set, not just in a top-level-only loop. Position is checked before keyword — the two conditions below are not independent, and position always wins:
   - The fragment is nested inside any `if`/`while` (not a literal sibling statement in the constructor's own top-level statement list, however deep the nesting) → `E_DELEGATION_NOT_FIRST_STATEMENT`, unconditionally, regardless of which keyword it uses or what step 1 found. Example: `Dog(String n) { super(n); if (true) { this(5); } else { ; } }` — `this(5)` is nested, so this is `E_DELEGATION_NOT_FIRST_STATEMENT` even though it's also an opposite-keyword case; nesting is checked first and wins.
   - Only for a fragment that *is* a literal top-level sibling statement (same nesting depth as the constructor's own body): if a delegation was recorded in step 1 and this fragment uses the **opposite keyword** (recorded `this`, new one is `super`, or vice versa) → `E_DELEGATION_BOTH_SUPER_AND_THIS`. Example: `Dog(String n) { this(n); super(n); }`.
   - Every other top-level-sibling case — no delegation was recorded at all in step 1 (e.g. `Dog(String n) { x = 1; super(n); }`), or the fragment repeats the **same** keyword as the one already recorded (e.g. `Dog(String n) { this(n); this(5); }`) → `E_DELEGATION_NOT_FIRST_STATEMENT`.
3. Anything that isn't delegation-shaped falls through to ordinary `Stmt` parsing (decision point 2) as normal.

Cases this correctly does *not* flag: `super(n); this.setup();` — the second statement's `this` is followed by `.`, not `(`, so it never matches the delegation-shaped test and falls through to ordinary `Stmt` parsing (P19). A root class constructor with only `this.foo();` and no delegation at all — same reasoning, `this` followed by `.` at the very start fails the delegation-shaped test in step 1, concluding "no delegation" with no ambiguity.

---

## AST shape (parser's output)

```rust
enum Type {
    Int,
    Bool,
    String,        // primitive, NOT Class("String") — no methods exist on String at
                    // all (E_RECEIVER_NOT_CLASS_TYPE forbids calling anything on a
                    // primitive receiver), no subtyping, and its `=`/`<`/`>` are
                    // lexicographic value comparisons, not reference equality.
                    // Modeling it as Class("String") would need MORE special-casing,
                    // not less: exclusions from method dispatch, from reference
                    // equality, from cast-target legality, all by hand. As its own
                    // variant, all of that falls out for free by not matching Class(_).
    Void,
    Class(String),
}

struct Program {
    classes: Vec<ClassDecl>,
}

struct Param {
    declared_type: Type,
    name: String,
    line: u32,
    // Every declaration-shaped node in this AST carries its own line; matters for
    // accurate per-field/per-formal error reporting (E_DUPLICATE_FIELD, E_FIELD_TYPED_VOID,
    // E_FORMAL_TYPED_VOID, E_LOCAL_SHADOWS_FORMAL, E_FIELD_SHADOWING) — without it,
    // the checker could only report the enclosing ClassDecl/MethodDecl/ConstructorDecl's
    // line, which can be far from the actual offending field or formal in a multi-field
    // class or multi-parameter method. Doesn't affect grading (the harness only
    // substring-matches the error code, never the line), but the position is still
    // worth getting right.
}

struct ClassDecl {
    name: String,
    extends: Option<String>,   // None = forest root — no implicit "Object" superclass
    fields: Vec<Param>,
    constructors: Vec<ConstructorDecl>,
    // Empty Vec means "the [ ] bracket section was omitted from source" — NOT
    // "present but empty" (parser rejects a bare `[ ]` as E_MALFORMED_CLASS_DECL,
    // trigger text: "has empty [ ] brackets"). Root classes may omit the section;
    // classes with extends: Some(_) must have an explicit, non-empty section
    // (E_MISSING_CONSTRUCTOR_IN_INHERITING_CLASS).
    methods: Vec<MethodDecl>,
    line: u32,
}

struct ConstructorDecl {
    formals: Vec<Param>,
    // Overload by ARITY ONLY. Same arity twice in one class: E_DUPLICATE_CONSTRUCTOR_ARITY.
    other_constructor_call: Option<OtherConstructorCall>,
    // Syntactically optional, not semantically optional:
    //   extends: Some(_) => MUST be Some(SuperCall) or Some(ThisCall), never None
    //   extends: None     => may be None or Some(ThisCall), never Some(SuperCall)
    //                        (E_SUPER_IN_ROOT_CLASS)
    // When present, must be the first statement (grammar-enforced structurally).
    body: BodyScope,
    // Empty constructor body (no forwarding call AND no statements) is illegal —
    // checked semantically; constructor bodies use Stmt*, not Stmt+, so the grammar
    // allows it.
    line: u32,
}

struct MethodDecl {
    ret: Type,
    name: String,
    // No overloading — method names unique per class. Overrides must match the
    // parent's signature exactly (E_OVERRIDE_SIGNATURE_MISMATCH, no covariant returns).
    formals: Vec<Param>,
    body: MethodBody,
    line: u32,
}

enum OtherConstructorCall {
    ThisCall(Vec<Expr>, u32),
    SuperCall(Vec<Expr>, u32),
}

enum MethodBody {
    User(BodyScope),
    Io(IoOp),   // never parsed from text
}

enum IoOp {
    ReadInt, ReadBool, ReadString, Eof,
    PrintInt, PrintBool, PrintString, Println,
}
// Rust variant names stay PascalCase; SOURCE-level method names the parser matches
// against are snake_case (read_int, read_bool, read_string, eof, print_int,
// print_bool, print_string, println). Live on two synthetic classes injected before
// checking, never appearing in the source file the student writes:
//   Input  { read_int, read_bool, read_string, eof }         bound to `in`
//   Output { print_int, print_bool, print_string, println }  bound to `out` and separately to `err`
// Two distinct singleton Output instances — one wired to stdout via `out`, a separate
// one wired to stderr via `err` — not one shared object bound to two names. Matters
// later for the type checker: out == err must be false.

struct VarDecl {
    declared_type: Type,
    names: Vec<String>,   // one VarDecl can declare several names sharing a type
                           // (`int x, y;`) — duplicate-name checks must expand this
                           // per-name, not compare whole VarDecl nodes, or a collision
                           // like `int x;` vs. `bool x, y;` in different branches
                           // will be missed.
    line: u32,
}

struct BodyScope {
    locals: Vec<VarDecl>,  // every VarDecl belonging to this method/constructor,
                            // flattened here by the parser regardless of how deeply
                            // nested in if/while it was textually declared. Duplicate
                            // names anywhere in this list — after expanding VarDecl's
                            // Vec<String> — are E_DUPLICATE_LOCAL, checked by the
                            // parser as each name is added, no exceptions for
                            // matching types or mutually-exclusive if/else arms.
    stmts: Vec<Stmt>,
    line: u32,
}
// Used only for MethodDecl/ConstructorDecl bodies. if/while bodies are plain Vec<Stmt>.

enum Stmt {
    Assign(String, Expr, u32),
    Return(Expr, u32),
    If(Expr, Vec<Stmt>, Vec<Stmt>, u32),   // both branches mandatory — grammar requires else
    While(Expr, Vec<Stmt>, u32),           // LO's only loop construct
    Break(u32),                            // legal only inside a while (E_BREAK_OUTSIDE_LOOP
                                            // otherwise — well-formedness, not a parse error)
    Empty(u32),
    CallStmt(MethodCall),
}
// Grammar requires at least one Stmt in a method/if/while body (Stmt+); parser-enforced,
// syntax error if violated. Constructor bodies use Stmt*, allowed to be empty of
// statements (though not of forwarding-call+statements together — see ConstructorDecl).

struct MethodCall {
    receiver: Receiver,
    name: String,
    args: Vec<Expr>,
    line: u32,
}
// Shared by Stmt::CallStmt and Expr::Call — identical shape, no duplication.
// Deliberately narrow: only ever a real instance-method dispatch with a receiver,
// never a constructor call — New and OtherConstructorCall are separate types that
// never wrap this.

enum Expr {
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
    // Primitive casts illegal. Legal only when one static type is a subtype of the
    // other — sibling casts between unrelated classes are a COMPILE error
    // (E_CAST_UNRELATED_TYPES), not just a runtime one. Checker concern, not parser
    // — the parser accepts any (Type)(Expr) shape and lets the checker judge legality.
    InstanceOf(Box<Expr>, String, u32),
    // Unlike Cast, no subtype relation required — legal for any pair of class types.
}

enum Receiver {
    Var(String, u32),
    This(u32),
    Super(u32),
    // super.foo() IS legal in ordinary method bodies, not just constructor forwarding
    // calls. Resolved STATICALLY against the class the calling method is lexically
    // defined in, never the receiver's runtime class. Compiles to a direct call,
    // never virtual dispatch.
    Computed(Box<Expr>, u32),
    // The receiver is an arbitrary computed expression — (new Circle(5)).area(),
    // ((Cat) a).purr(). The only way a receiver can be anything other than a simple
    // name/this/super, and it does get constructed for real (not dead code). LO
    // forbids direct call-chaining without parens: `a.foo().bar()` is a syntax
    // error; `(a.foo()).bar()` is required.
}

enum BinOp { Add, Sub, Mul, Div, Mod, And, Or, Lt, Gt, Eq }
// & / | short-circuit. String `+` = concat, `*` = repeat (int RHS, left-string-only).
// String `<`/`>`/`=` = lexicographic value comparison. Class-type `=` is reference
// equality; `<`/`>` illegal on class types.
enum UnOp { Not, Neg }
// Neg (~) on int = negation; on String = reversal; illegal on Bool (use Not).
```

**What each part is:**

| Type | Represents | Key semantic notes |
|---|---|---|
| `Program` | the whole compilation unit | one or more `ClassDecl`s, nothing else at top level |
| `Param` | one `(declared_type, name)` pair, with its own `line` | shared by `ClassDecl.fields`, `ConstructorDecl.formals`, `MethodDecl.formals` — but `fields` is flattened from `VarDecl`s (grouped names legal, e.g. `int x, y;`), while `formals` comes from `Formals` (already one name per entry) — see the field-list note above |
| `ClassDecl` | one class | forest root if `extends: None`; empty `constructors` means the bracket section was omitted |
| `ConstructorDecl` | one constructor | arity-only overload resolution; forwarding-call mandatoriness depends on `extends` |
| `OtherConstructorCall` | the `this(...)`/`super(...)` call at the top of a constructor | constructor-only, never in a method body; calls another constructor on the same object, no allocation |
| `MethodBody` | a method's implementation | `User` for real code, `Io` for the 8 built-in I/O methods, never parsed from text |
| `VarDecl` | one declaration, possibly several names sharing a type | flattened into the enclosing `BodyScope` by the parser regardless of original nesting |
| `BodyScope` | every hoisted local + top-level statements for one method or constructor | the *only* place `locals` exists in this AST |
| `Stmt` | one statement | `If`'s `else` branch mandatory; `While` is the only loop; bodies are plain `Vec<Stmt>` |
| `MethodCall` | a receiver, method name, and arguments | shared shape between statement-position and expression-position calls; instance-method dispatch only |
| `Expr` | one expression | every compound form was fully parenthesized in source; none of those parens survive parsing |
| `Receiver` | what a call is invoked on | `Super` resolves statically; `Computed` is required for any non-trivial receiver |

---

## Open items

Genuinely unresolved questions — need an answer from course staff, not a team assumption.

1. Fail-fast has no supporting citation anywhere in the source material for any compiler phase. This is a team assumption, adopted for implementation simplicity and for code-review legibility (a fail-fast parser/checker is easier to read than one juggling accumulated-error state). Not expected to be controversial, but not confirmed either.
2. No error code exists for duplicate names *within one formal-parameter list* (`void foo(int x, int x)`) — the well-formedness section of `error-codes.md` gives `E_LOCAL_SHADOWS_FORMAL` for local-vs-formal and `E_DUPLICATE_LOCAL` for local-vs-local, and nothing covers formal-vs-formal.

---

## Notes

Settled facts and implementation guidance — not open questions, just worth recording here rather than leaving implicit.

- Binary-operator operand evaluation order: left-to-right. Never stated explicitly in the language reference the way actual-argument-list order is, but every compound expression in this grammar is mandatorily fully parenthesized, so there's no ambiguity for it to resolve — left-to-right is simply the only sensible reading, not a judgment call.
- `super(x)`/`this(x)` referencing a local `x` declared later in the same constructor body is legal under the hoisting rules (`x` holds its default value, e.g. `0` for `int`, since a use may precede its declaration). Not special-cased anywhere — this falls out of the ordinary rules and is worth knowing about rather than mistaking for a bug. Good candidate for a contributed test.
- The parser threads real state through recursive descent that a purely-syntactic parser wouldn't otherwise need: the enclosing class's name (`E_MALFORMED_CONSTRUCTOR`), the current `BodyScope` handle (hoisting, `E_DUPLICATE_LOCAL`), and an "inside a constructor body" flag (the delegation checks). All three are scoped per method/constructor and must never leak across method boundaries — bundle them into one explicit parser-context type threaded through the recursive calls, not separate loose parameters.
- For the type-checker design: `Type::String` is deliberately its own variant, not `Type::Class(_)`. Correct for dispatch/subtyping/cast-legality — but `String` is still heap-allocated and needs the same shadow-stack rooting and write-barrier treatment as a class-typed local, so "is this local pointer-typed for GC purposes" is `matches!(ty, Type::Class(_) | Type::String)`, not just `Type::Class(_)`.
- For LO-5 feature selection, if the team has influence over it: generics would directly collide with this design. Cast disambiguation (decision point 4) works *because* `Type` is always exactly one token; a type-argument payload on `Class` would be a breaking change, and angle-bracket syntax would collide with `<`/`>` already being live `Binop` tokens, reintroducing the C++ template-parsing ambiguity. Closures and exceptions would both be smooth, additive extensions by comparison.

---

## Alternate designs considered, not chosen

| Option | Rejected because |
|---|---|
| Separate concrete/raw syntax tree, pruned into the AST afterward | No source document mentions a CST/parse tree; skipping one is an inferred conclusion, not an explicit course recommendation. The inference holds regardless: the grammar's mandatory full parenthesization removes the only reason (deferred precedence resolution) such a tree would normally exist here. |
| Parser-combinator or grammar-generator tooling | Course's stated stance against "parser generator escape hatches" — hand-written recursive descent, defensible at the whiteboard. |
| Backtracking / PEG-style speculative parsing | Grammar is provably resolvable with small, bounded, forward-only lookahead everywhere, including the cast case. |
| A semantic "known class names" table (the C-style "lexer hack") for cast vs. nested-paren disambiguation | C needs this because its `Type` can be an arbitrarily complex compound declarator; LO's `Type` is always exactly one token, so bounded structural lookahead resolves it without a name table. Would also have broken "the parser never resolves or validates names," which holds for every other identifier in this AST. |
| Emitting code directly from parser actions, no AST at all (true Wirth-style "codegen during parse") | "Codegen during parse" is not merely historical color — the handout mandates it as a live, active discipline for *this project's own WASM back end* specifically ("This constraint is pedagogical and deliberate"). That discipline governs the WASM emission phase, a different phase from parsing, and doesn't require the *parser* itself to skip building an AST — the course's own compiler is AST-based and multi-pass overall (declaration pre-scan → build/check AST → single emission pass over it), and the WASM back end's one-pass-no-intervening-IR rule operates on the already-built AST, not on raw tokens. |
| Two parallel AST types — one from the parser, a separately-typed one from the checker | The checker annotates the same tree in place (`Cell`-based fields on the handful of node kinds needing post-check resolution), optionally wrapped in a `Checked` marker once verified. Avoids two hand-written type definitions that have to be kept in lockstep. |
| Keeping the body-scope type uniform across method, constructor, `if`, and `while`, with hoisting done later by the checker | Considered specifically to hedge against flat-hoisted semantics possibly changing to block-scoped. Hoisting is confirmed permanent by course staff, and separately, exact textual position stopped mattering for legality once "a use may precede its declaration" was confirmed — both original justifications for the hedge no longer apply, so flattening at parse time (this document's current design) is preferred instead. |
| Merging `VarDecl` into the same list as `Stmt`, with a "reorder declarations to the top" step as a removable pass | Blurs a distinction the grammar makes on purpose (`Block → { (VarDecl)* (Stmt)+ }` is two nonterminal categories, not one interleaved list), and reordering *within* a single block doesn't reproduce cross-block hoisting unless it *also* reaches into nested blocks — the same work as full flattening, just placed worse. |
| `ConstructorForward` / `IntrinsicOp` / `Preamble` / `LocalScope` / `CallableScope` as final names | See "Naming" table above for the specific reasoning behind each rejection. |

---

## `add_io_classes`

`fn add_io_classes(program: Program) -> Program`. Free function, own module, not part
of the parser — takes the parsed `Program`, returns it with `Input`/`Output` added, runs
between `parse_program` and the type checker's Pass 1. Pure: no tokens, no `ParseError`.

Builds two `ClassDecl`s, prepended to `program.classes` (reference: preamble is "treated
as if it preceded the first user class"), signatures copied verbatim from the reference's
own preamble listing (§3.4.6):

```
class Input ( ) {
    int    read_int()      // IoOp::ReadInt
    bool   read_bool()     // IoOp::ReadBool
    String read_string()   // IoOp::ReadString
    bool   eof()           // IoOp::Eof
}

class Output ( ) {
    void print_int(int n)         // IoOp::PrintInt
    void print_bool(bool b)       // IoOp::PrintBool
    void print_string(String s)   // IoOp::PrintString
    void println()                // IoOp::Println
}
```

| Field | Value | Why |
|---|---|---|
| `extends` | `None` | Forest roots — not extensible per the reference, no parent given anywhere. |
| `fields` | `vec![]` | Ordinary empty field-parens (P4's `( (VarDecl)* )`, zero entries) — same as any class with no fields, e.g. `Main ()`. |
| `constructors` | `vec![]` | `[ ]` section omitted entirely — legal only for root classes, which these are; implicit no-arg constructor, same convention as `Main () { ... }` in the corpus. |
| every `line` | `0` | Real parsed nodes are always `>= 1`; `0` unambiguously marks the node as synthetic. |

Grammar-conformant by construction, cross-checked against `LO-4/ValidPrograms/test_3_hello_world.lo`'s `class Main () { ... }` for the root/empty-fields/omitted-brackets shape, and against `Formals`' one-`Param`-per-name production for the method parameters. `MethodBody::Io(op)` is the one deliberate divergence from a real parse — these bodies are never written as text, so there's no `BodyScope` to build; `IoOp` exists for exactly this.

**Out of scope for this step:**
- Binding `in`/`out`/`err` — those are pre-bound singleton variables, not classes; that wiring is the type checker's (name resolution) and interpreter/WASM back end's (actual stdout/stderr/stdin plumbing) responsibility, not something `Program.classes` holds.
- `E_RESERVED_CLASS_NAME` / `E_RESERVED_VARIABLE_NAME` checks — a user file declaring `class Input` or a local `out` still parses fine; Pass 1 catches the collision against the now-present synthetic class, same as any other duplicate.
- Idempotency — not guaranteed and not needed; called exactly once in the pipeline, `add_io_classes(parse_program(tokens)?)`.
