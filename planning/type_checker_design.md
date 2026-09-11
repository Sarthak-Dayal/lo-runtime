# Type Checker Design — LO (LiveOak) P1

Scope: `Program` (the parser's AST, LO-4-complete) in, either `Ok((TypedProgram,
ClassTable))` or the first `TypeError` out. Preamble injection (`Input`/`Output`,
`in`/`out`/`err`) is part of this stage — it's semantic synthesis, not syntax. The
interpreter and codegen are separate documents; this one settles what they get to
assume once `check_program` returns `Ok`.

**Revision note:** this is the second full pass over this design. The first pass
proposed `Checked(Program)` — the same untyped tree, unmodified, with no per-node
annotation at all — after cutting an even earlier plan to annotate `ast.rs` in place
with `Cell` fields. A team review (23 comments, two reviewers) rejected that middle
position: the consensus is a genuine **typed AST**, produced by the checker as a
transformation (`Program` → `TypedProgram`), with every typed node carrying its
resolved facts directly and non-optionally. This document reflects that decision.

---

## Decisions

| Decision | Why |
|---|---|
| Four gated passes — `gather_declarations` → `resolve_inheritance` → `check_entry_point` → `check_bodies` — each requiring the previous to succeed before it runs | `parser_design.md` already names the first of these "the type checker's Pass 1." Body-checking needs the *whole* class table (a method may call a method declared later, in a class declared later in the file — LO-2 §2.3.2's "use before definition is allowed" for callables), so declarations must be fully gathered before any body is walked. Running a later pass over a class table a prior pass already found broken produces cascading noise, not signal, so each pass gates the next rather than collecting independent errors across passes. |
| **Reversed:** the checker produces a genuine `TypedProgram`, not the original tree wrapped in a marker | Team review rejected the "no second type at all" position from the prior draft. `Scope` now maps names to `BindingInfo` (type *and* binding kind — local/formal/field-with-owner), not just `Type`, so the typed tree can record which one resolved a name. A resolved method call keeps its owning class and full signature. A cast keeps its resolved direction. None of these fields are `Option`-wrapped — if a node exists in `TypedProgram`, it has already been fully checked, so there is nothing left to be uncertain about. `ast.rs` is still untouched: `TypedProgram` and its node types are new types living entirely in `sema.rs`, so this doesn't reopen the earlier, specifically-rejected idea of mutating a file `parser_implementation.md` owns — see "Alternate designs" for why those are two different objections. |
| `check_program(Program) -> Result<(TypedProgram, ClassTable), TypeError>`, with `inject_preamble` called **inside** `check_program`, not as a separate step before it | **Reversed** from the prior draft, which kept preamble injection as its own pipeline stage "so callers don't forget a compiler stage in between." Team review pushed the other way: folding it into `check_program` means a caller can't construct a `TypedProgram` that's missing the preamble by skipping a step — there is exactly one way to get a `TypedProgram` at all, and it always includes `Input`/`Output`. The previous draft's `main.rs`/implementation-doc code already called `inject_preamble` from inside `check_program` even though this document said otherwise; that inconsistency is resolved in this document's favor of what the code already did. |
| Vtable slot assignment is **not** part of the type checker | `parser_design.md`'s own open item: *"The type-checker design should settle whether vtable layout is literally inside Pass 1 or a dependent step right after it."* Settled here: it's neither — it's codegen's declaration pre-scan, which consumes this checker's `effective_methods` table (name, signature, owning class) as input and assigns slot indices itself. Type-checking correctness never needs a slot number, only "does this override an ancestor's method with an identical signature." Keeping slots out of the checker also keeps `ClassTable` reusable by the interpreter, which dispatches by name lookup on the receiver's *runtime* class and never touches a vtable at all. |
| Effective fields and effective methods are computed **once per class**, memoized, in topological order over the (by then validated-acyclic) `extends` forest | Every field read and every method call would otherwise re-walk the ancestor chain from scratch. LO-4 inheritance is a forest of bounded, shallow depth, but there's no reason to re-derive the same answer once per use site when it's knowable once per class, up front. |
| Every declared class carries a `ClassKind` (`User` or `Preamble`) | New this pass, from review: `Input`/`Output` were sitting in `ClassTable` as ordinary entries, which meant nothing stopped `new Input()` or `class Foo extends Output()` even though §4.6 says *"the synthesized wrapper is the only code that instantiates Input and Output"* and both are meant to be non-extensible, same as every other LO-3/4 class the language never lets user code subclass implicitly. `ClassKind` is the cheapest way to give the checker something to test against, rather than re-deriving "is this one of the two magic ones" from the class's declaration line being `0` at every call site that cares. |
| `Type::String` is handled as its own case throughout — never folded into the general class-type machinery | It's a distinct AST variant, not a legal `extends` target, cast target, or `instanceof` target (structurally unreachable — `String` is a lexer keyword and can never occupy a `ClassName` slot), and it has no ancestors/descendants to walk. |
| Fail-fast: `Result<T, TypeError>`, stop at the first error | Matches `LexError`/`ParseError`'s existing shape and the parser's stated convention. |
| Classes, fields, and methods are keyed by `String` in `HashMap`s — no `ClassId`/interning layer | LO programs are course-scale (tens of classes at most). |
| `is_subtype(a, b)` is a linear scan of a precomputed ancestor-chain `Vec<String>` (root-terminated, **not** including `a` itself) | LO-4 is single inheritance — chains are short. |
| Ternary result type is computed via **least common ancestor**, not a bidirectional subtype check | **Reversed** from the prior draft, and this was a real bug, not just an unfinished design question: with `Cat <: Animal` and `Dog <: Animal` but no relation between `Cat` and `Dog`, the bidirectional `is_subtype(a,b) \|\| is_subtype(b,a)` check tries both directions, finds neither, and incorrectly rejects `(cond ? cat : dog)` — when the correct static type is `Animal`. LCA over the same precomputed ancestor chains subsumes the old bidirectional check as a special case (when one class is already an ancestor of the other, its own chain-walk finds it on the first step) while also handling true siblings correctly. See Algorithms §7. |
| `resolve_inheritance` iterates classes in source declaration order, not `HashMap` key order | Iterating a `HashMap`'s keys directly means *which* error/code comes back for a program with more than one independent violation is not guaranteed stable across runs. `ClassTable` records declaration order explicitly for this. |

---

## Naming

| Name | What it is | Considered and rejected |
|---|---|---|
| `ClassTable` | The whole-program registry built by `gather_declarations`, consumed by every later pass | `SymbolTable` — over-general; LO's only top-level symbols this table needs to hold are classes. Locals/formals get their own, separate `Scope` (below), never merged into this one. |
| `ClassInfo` | Per-class record: own declarations (Pass 1) plus derived inheritance data (Pass 2) | Splitting into `ClassDecl`-mirroring vs. derived-data structs was considered — rejected as two things to keep in sync for a size of data that doesn't justify the split. |
| `Scope` | Per-body flat `name → BindingInfo` map (formals + hoisted locals), built once at the start of checking a body | `LocalScope` — redundant; program-scope `in`/`out`/`err` are handled as a fixed final lookup tier, not a `Scope` value (Algorithms §5). |
| `BindingInfo` | `Local(Type) \| Formal(Type) \| Field { owner: String, ty: Type }` | New this pass — see Decisions. Considered keeping `Scope` mapping to bare `Type` and reconstructing binding kind separately when the typed tree needs it — rejected because the reconstruction logic (is this name in `formal_names`? does it match an effective field?) is exactly the logic `resolve_name` already runs once; recomputing it a second time for annotation purposes is duplicated work for no benefit. |
| `TypedProgram` / `Typed*` node family | The checker's output tree — `TypedClassDecl`, `TypedMethodDecl`, `TypedConstructor`, `TypedStmt`, `TypedExpr`, `TypedMethodCall`, `TypedReceiver` | See "The typed AST," below, for the full shape and the reasoning for each field. |
| `TypeError` / `ErrorCode` | Mirrors `ParseError`/`ErrorCode` and `LexError`/`LexErrorKind`: `{ code, line, message }` plus `as_str()` | Reusing `parser.rs`'s `ErrorCode` directly was considered and deliberately deferred — see Open items. |
| Struct field ordering: `line` last | Every `ast.rs` struct (`Param`, `ClassDecl`, `MethodDecl`, `VarDecl`, `BodyScope`, `MethodCall`) puts `line` as its trailing field. This document's own structs (`MethodSig`, `ConstructorSig`) follow the same convention, fixed this pass — `ConstructorSig` previously had `this_target_arity` trailing `line`, which was simply an oversight, not a considered choice. |

---

## The typed AST

```rust
pub struct TypedProgram {
    pub classes: Vec<TypedClassDecl>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ClassKind { User, Preamble }

pub struct TypedClassDecl {
    pub name: String,
    pub kind: ClassKind,
    pub extends: Option<String>,
    pub fields: Vec<Param>,                  // types already validated; ast::Param as-is
    pub constructors: Vec<TypedConstructor>,
    pub methods: Vec<TypedMethodDecl>,
}

pub enum TypedConstructor {
    Explicit {
        params: Vec<Param>,
        delegation: Option<TypedDelegation>,
        locals: Vec<(String, Type)>,
        stmts: Vec<TypedStmt>,
    },
    Implicit {
        fields: Vec<(String, Type)>,          // this.field_i = formal_i, field order
    },
}

pub enum TypedDelegation {
    This { args: Vec<TypedExpr> },
    Super { args: Vec<TypedExpr> },
}

pub struct TypedMethodDecl {
    pub name: String,
    pub return_type: Type,
    pub params: Vec<Param>,
    pub body: TypedMethodBody,
}

pub enum TypedMethodBody {
    UserDefined { locals: Vec<(String, Type)>, stmts: Vec<TypedStmt> },
    Io(IoOp),
}

#[derive(Clone)]
pub enum BindingInfo {
    Local(Type),
    Formal(Type),
    Field { owner: String, ty: Type },
}

pub enum TypedStmt {
    Assign { target: String, binding: BindingInfo, value: TypedExpr },
    Return(TypedExpr),
    If(TypedExpr, Vec<TypedStmt>, Vec<TypedStmt>),
    While(TypedExpr, Vec<TypedStmt>),
    Break,
    Empty,
    CallStmt(TypedMethodCall),
}

pub struct TypedMethodCall {
    pub receiver: TypedReceiver,
    pub name: String,
    pub owner: String,             // the class whose effective_methods entry resolved this
    pub args: Vec<TypedExpr>,
    pub return_type: Type,
}

pub enum TypedReceiver {
    This(String),                              // enclosing class name
    Super,                                     // owner is on the enclosing TypedMethodCall
    Var { name: String, binding: BindingInfo },
    Computed(Box<TypedExpr>),
}

pub enum TypedExpr {
    Num(i32),
    Bool(bool),
    Str(String),
    Null,
    This(String),
    Var { name: String, binding: BindingInfo },
    New { class: String, args: Vec<TypedExpr> },
    Call(TypedMethodCall),
    Ternary { cond: Box<TypedExpr>, then_branch: Box<TypedExpr>, else_branch: Box<TypedExpr>, ty: Type },
    Binary { lhs: Box<TypedExpr>, op: BinaryOp, rhs: Box<TypedExpr>, ty: Type },
    Unary { op: UnaryOp, operand: Box<TypedExpr>, ty: Type },
    Cast { target: Type, operand: Box<TypedExpr>, direction: CastDirection },
    InstanceOf { operand: Box<TypedExpr>, class: String },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CastDirection { Upcast, Downcast }
```

**`TypedExpr::Null` carries no `Type`.** This is the one place a "resolved" node
still doesn't have a fixed type, and it's deliberate, not an oversight the
no-`Option`-fields rule should have caught: `null` genuinely has no type of its own
in LO (LO-3 §3.4.5 — "The literal `null` has no inherent type... its type at any use
site is determined by surrounding context"). Every consumer that places a `null` in a
typed context already resolves the concrete type at that placement — an assignment
target's type, a cast's target, a ternary's LCA result — so nothing downstream ever
needs to ask "what type is this specific `TypedExpr::Null`" in isolation; it always
already knows from the surrounding node. Giving `Null` a synthetic `Type` field would
mean inventing a value with no basis (which supertype would it be?) just to avoid one
`match` arm elsewhere.

**Cast direction for a `null` source is `Upcast`.** Per LO-4 §4.4.4, casting `null` to
any class type always succeeds and needs no runtime check — that's exactly what
`Upcast` signals to codegen (no `lo_cast_check` call emitted), even though "upcast" is
a slight abuse of the word when there's no runtime class to be wider or narrower than.
The alternative (a third `CastDirection` variant just for this) was considered and
rejected — it would exist for exactly one call site's benefit, and `Upcast` already
means precisely the thing codegen needs to know ("skip the check").

**A both-branches-`null` ternary (`cond ? null : null`) is a type error.** There is no
concrete `Type` to put in `TypedExpr::Ternary`'s `ty` field, and the no-`Option`-fields
rule means the typed tree can't represent "still untyped, resolve from context" the
way the untyped `ExprType::NullLiteral` internally can during checking. This is a
narrow, deliberately-accepted edge case — nobody writes this pattern for any reason
other than an actual mistake — not a gap in an otherwise-complete design.

---

## Preconditions inherited from the parser

By the time a `Program` reaches `sema::check_program`, the following already hold —
established in `parser_design.md`/`parser_implementation.md` — and must **not** be
re-checked here, only relied on:

- Every `BodyScope.locals` entry has a name distinct from every other local in the
  *same* body (`E_DUPLICATE_LOCAL`, parser-level), across arbitrary `if`/`while`
  nesting depth. **Not** yet checked against the enclosing method's formals — that
  stays checker-phase (`E_LOCAL_SHADOWS_FORMAL`).
- A constructor's name equals its enclosing class's name (`E_MALFORMED_CONSTRUCTOR`,
  parser-level).
- Constructor delegation is well-*shaped*: at most one of `this(...)`/`super(...)`,
  appearing only as the true first statement (`E_DELEGATION_BOTH_SUPER_AND_THIS` /
  `E_DELEGATION_NOT_FIRST_STATEMENT`, parser-level). The checker only resolves whether
  the delegation's *target* is legal — never re-verifies position or keyword
  exclusivity.
- `Type::Class("String")` never occurs — `String` is a lexer keyword.
- Every `if`/`while` body is a flat `Vec<Stmt>` with no `VarDecl`s of its own.
- A method body has at least one statement; a constructor body may have zero
  (`Block` vs. the constructor's own inline production).
- **Not** yet checked, contrary to what an earlier draft assumed: that formal
  parameter names within one method/constructor are pairwise distinct, and that every
  local's/formal's *declared type* actually resolves to something real. Both are
  checker-phase — see the rule table below.

---

## Data model (`ClassTable`)

```rust
pub struct ClassTable {
    classes: HashMap<String, ClassInfo>,
    order: Vec<String>,                   // source declaration order
}

pub struct ClassInfo {
    pub decl_line: u32,
    pub kind: ClassKind,                  // User | Preamble — see Decisions
    pub parent: Option<String>,           // Pass 2a, resolved + validated
    own_fields: Vec<(String, Type, u32)>, // Pass 1, flattened per-name
    own_methods: Vec<MethodSig>,          // Pass 1
    own_constructors: Vec<ConstructorSig>,// Pass 1

    // Filled in by resolve_inheritance (Pass 2), memoized:
    pub ancestors: Vec<String>,                        // strict ancestors only, root last
    pub effective_fields: Vec<(String, Type, String)>, // (name, type, owner), parent-first
    pub effective_methods: HashMap<String, (String, MethodSig)>,
}

#[derive(Clone, PartialEq)]
pub struct MethodSig {
    pub name: String,
    pub params: Vec<Type>,
    pub return_type: Type,
    pub line: u32,
}

#[derive(Clone)]
struct ConstructorSig {
    arity: usize,
    params: Vec<Type>,
    this_target_arity: Option<usize>, // Some(n) iff this ctor's own delegation is
                                       // this(...) targeting the n-arity constructor
                                       // of the same class — see Algorithms §4.
    line: u32,
}
```

`own_fields` is per-*name*, not per-`VarDecl`: `ClassDecl.fields: Vec<Param>` already
arrives pre-flattened by `parse_field_list`, so `E_DUPLICATE_FIELD` is checked across
this flattened list directly.

`is_subtype`'s special-case for `a == b` is what makes excluding self from `ancestors`
safe — a class is trivially its own subtype without needing to appear in its own
ancestor list.

---

## Spec rule → pass → error code

| Rule | Pass | Code(s) |
|---|---|---|
| Class names unique program-wide | 1 | `E_DUPLICATE_CLASS_NAME` |
| `Main`/`Input`/`Output` not redeclared by user code | 1 | `E_RESERVED_CLASS_NAME` |
| Field names unique per class (flattened) | 1 | `E_DUPLICATE_FIELD` |
| Method names unique per class (no overloading, ever) | 1 | `E_DUPLICATE_METHOD` |
| Formal parameter names unique within one method/constructor's own list | 1 | `E_DUPLICATE_FORMAL` *(invented — see Notes)* |
| Constructor arities distinct within a class | 1 | `E_DUPLICATE_CONSTRUCTOR_ARITY` |
| No field/formal typed `void` | 1 | `E_FIELD_TYPED_VOID` / `E_FORMAL_TYPED_VOID` |
| No local typed `void` | 4 | `E_LOCAL_TYPED_VOID` *(invented — see Notes)* |
| Every class name used in a signature (`extends`, field/formal/return/local type) resolves | 2a (declarations), 4 (locals) | `E_UNKNOWN_CLASS` |
| No `extends` cycle | 2b | `E_INHERITANCE_CYCLE` |
| Nothing extends `Main` | 2a | `E_ENTRY_POINT_OTHER` |
| Nothing extends a `Preamble`-kind class (`Input`/`Output`) | 2a | `E_INHERITANCE_CHECK_OTHER` |
| `new Input()`/`new Output()` rejected — only the synthesized wrapper instantiates them | 4 | `E_TYPE_CHECK_OTHER` |
| No field name in a subclass repeats an ancestor's effective field | 2c | `E_FIELD_SHADOWING` |
| Same-name-same-signature override only | 2c | `E_OVERRIDE_SIGNATURE_MISMATCH` |
| A class with `extends` has an explicit `[ ]` constructor section | 2c | `E_MISSING_CONSTRUCTOR_IN_INHERITING_CLASS` |
| An explicit constructor body is non-empty (a delegation, or at least one statement) | 4 | `E_WELL_FORMEDNESS_OTHER` *(no dedicated code — see Notes)* |
| `Main` declared, no `extends`, `int main()` with no formals, zero-arg constructor reachable | 3 | `E_NO_MAIN_CLASS`, `E_MAIN_CLASS_EXTENDS`, `E_NO_MAIN_METHOD`, `E_MAIN_METHOD_SIGNATURE`, `E_MAIN_NO_ZERO_ARG_CONSTRUCTOR` |
| Identifier resolves (local → formal → field → `in`/`out`/`err`) | 4 | `E_UNKNOWN_VARIABLE` |
| Local doesn't repeat a formal's name | 4 | `E_LOCAL_SHADOWS_FORMAL` |
| `in`/`out`/`err` not redeclared as a field, formal, or local | 1 (fields, formals), 4 (locals) | `E_RESERVED_VARIABLE_NAME` |
| Method lookup succeeds on the receiver's static type's effective methods | 4 | `E_UNKNOWN_METHOD` |
| Assignment / return / actual-argument compatibility (`<:` closure, `null` special-cased) | 4 | `E_ASSIGN_TYPE_MISMATCH`, `E_RETURN_TYPE_MISMATCH`, `E_ACTUAL_TYPE_MISMATCH` |
| Operator operand typing (int/bool/String only, per operator) | 4 | `E_BINOP_TYPE_MISMATCH`, `E_UNOP_TYPE_MISMATCH` |
| Ternary branch compatibility (least common ancestor, not bidirectional subtype — see Decisions) | 4 | `E_CONDITIONAL_TYPE_MISMATCH` |
| Ordinary call / `new` arity matches | 4 | `E_ARITY_MISMATCH` |
| Receiver has class type; not literally `null` | 4 | `E_RECEIVER_NOT_CLASS_TYPE`, `E_NULL_LITERAL_RECEIVER` |
| Void/non-void call used in the right position (**both** directions — see Notes) | 4 | `E_NONVOID_CALL_AS_STATEMENT`, `E_VOID_CALL_IN_EXPRESSION` |
| Return-path completeness (**any** statement returns, not just the last — see Notes) | 4 | `E_RETURN_MISSING`, `E_RETURN_IN_VOID_METHOD`, `E_RETURN_IN_CONSTRUCTOR` |
| `break;` only inside an enclosing `while` | 4 | *(unpublished — see Notes)* |
| `super(...)` only in a constructor whose class has a parent; `super.m(...)` only in a method body (never a constructor body — see Notes), only when a parent exists; target resolves; no delegation cycle; delegation arity matches | 4 | `E_SUPER_IN_ROOT_CLASS` (constructor `super(...)`), `E_SUPER_METHOD_IN_ROOT_CLASS` (method-body `super.m(...)`, no parent), `E_INHERITANCE_CHECK_OTHER` (`super.m(...)` used inside a constructor body), `E_SUPER_METHOD_UNRESOLVED`, `E_DELEGATION_CYCLE`, `E_DELEGATION_ARITY_MISMATCH` |
| Cast: `null` always succeeds (target-typed, `Upcast`); otherwise target/source are class types and one is a subtype of the other | 4 | `E_CAST_TARGET_NOT_CLASS`, `E_CAST_SOURCE_NOT_CLASS`, `E_CAST_UNRELATED_TYPES` |
| `instanceof`: source is a class type **or `null`** (legal, statically `bool`, runtime `false`) | 4 | `E_INSTANCEOF_SOURCE_NOT_CLASS` |

Every phase also carries its own `..._OTHER` sentinel for a genuinely uncategorized
failure in that phase.

`E_THIS_OUTSIDE_INSTANCE` is in the published vocabulary but has no reachable trigger
given this grammar and is omitted from `ErrorCode` entirely — not kept as untestable
dead code with a borrowed sentinel.

---

## Algorithms

### 1. Effective-member computation order

`resolve_inheritance` walks the (validated-acyclic) `extends` forest bottom-up: a
class's `effective_fields`/`effective_methods` are computed only after its parent's
are already sitting in the table, so computing them is "clone the parent's effective
set, then extend/override with this class's own declarations" — never a fresh
ancestor-chain walk.

### 2. Override matching and field shadowing both walk the *full* ancestor chain

§4.3.3: a method `m` in `C` overrides `m` in `P` for *any* `P` with `C <: P`, not only
`C`'s immediate parent. Because `effective_methods`/`effective_fields` are already the
parent's full inherited set (Algorithm 1), checking a new declaration against them
automatically reaches every ancestor — no separate "walk up further" step.

### 3. Nothing extends `Main` or a `Preamble`-kind class

Checked in the same 2a pass that resolves `extends` targets, right after confirming
the parent name exists: if the resolved parent is literally `"Main"`, or if
`table.get(parent).kind == ClassKind::Preamble`, reject before ever reaching cycle
detection or effective-member computation for that class. Neither check needs
anything beyond what 2a already has in hand.

### 4. Cycle detection — one generic helper, two call sites

Both `E_INHERITANCE_CYCLE` (over `extends` edges) and `E_DELEGATION_CYCLE` (over
`this(...)` edges within one class's constructors) are the same shape: a directed
graph, DFS with a "currently on the path" visiting-set, cycle iff a back-edge is
found. The `extends` case reads edges from `ClassInfo.parent`; the `this(...)` case
reads `ConstructorSig.this_target_arity`, populated once in Pass 1 directly from
`ctor.delegation`, so the delegation-cycle check runs entirely off `ClassTable` with
no second reference to the raw AST.

### 5. Name resolution order inside a body

Three tiers, first match wins, exactly LO-3 §3.3.3: locals → formals (both live in
`Scope`, mapping to `BindingInfo::Local`/`BindingInfo::Formal`) → fields (via the
enclosing class's `effective_fields`, mapping to `BindingInfo::Field { owner, ty }`).
A fourth tier — `in`/`out`/`err` — is checked last, as a fixed lookup outside `Scope`,
since those bindings are program-wide, not per-class or per-body.

### 6. `super.m(...)` resolves differently from an ordinary call — and only in a method body

An ordinary `r.m(...)` resolves `m` in `r`'s static type's `effective_methods`.
`super.m(...)` resolves in the *static parent's* `effective_methods`, deliberately
bypassing the current class's own override. **New this pass:** `super.m(...)` is
rejected outright when it appears inside a *constructor* body — LO-4 §4.1 introduces
this form specifically as something that happens "inside a method body," and
constructor-body delegation already has its own dedicated form, `super(...)`
(no dot, no method name, `ConstructorDelegation::SuperCall`). The two are easy to
conflate because the AST can structurally represent `super.foo();` as an ordinary
statement inside a constructor's own statement list (after its delegation, if any) —
nothing about the grammar stops a constructor body from containing an arbitrary
`MethodCall` with `Receiver::Super` — so this has to be an explicit `ctx.in_constructor`
check, not something that falls out for free.

### 7. Ternary branch typing uses least common ancestor

```rust
fn least_common_ancestor(a: &str, b: &str, table: &ClassTable) -> Option<String> {
    let chain_a: HashSet<&str> = std::iter::once(a)
        .chain(table.get(a)?.ancestors.iter().map(String::as_str))
        .collect();
    std::iter::once(b)
        .chain(table.get(b)?.ancestors.iter().map(String::as_str))
        .find(|c| chain_a.contains(c))
        .map(String::from)
}
```

`b`'s own chain, walked from `b` itself up through its ancestors, is monotonically
"more general" moving away from `b` — so the *first* class in that walk that also
appears anywhere in `a`'s chain is necessarily the *closest* common ancestor, not
just *a* common one. This is correct because LO-4 inheritance is a forest of simple
upward chains (single inheritance, no diamonds) — there's no branching above either
class to make "closest" ambiguous. `None` means the two classes are in genuinely
disjoint hierarchies (LO-4 has no universal root class, per §4.1: "Subtype relations
form a forest"), which is exactly `E_CONDITIONAL_TYPE_MISMATCH`'s trigger condition
("no common supertype"). This fully replaces the prior draft's bidirectional
`is_subtype` check for ternaries, which was a real, confirmed bug — `Cat`/`Dog`
siblings under `Animal` would incorrectly error under the old check (neither is a
subtype of the other) instead of resolving to `Animal`. Casts are unaffected — a cast
is legal only with a *direct* subtype relation in one direction, which is a genuinely
different, narrower question than "what's the widest common type of these two."

### 8. Return-path completeness

```rust
fn definitely_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| stmt_returns(s))
}
fn stmt_returns(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(..) => true,
        Stmt::If(_, then_b, else_b, _) => definitely_returns(then_b) && definitely_returns(else_b),
        Stmt::While(..) => false,
        _ => false,
    }
}
```

**Corrected this pass** — the prior draft only checked whether the *last* statement
in a sequence returns (or is an `if` where both arms do), which is wrong: the correct
rule, per course lecture material, is that *any* statement in the sequence
definitely-returning is enough, since anything textually after an unconditional
return is unreachable but doesn't retroactively make the method "not return." `while`
never contributes either way — the checker can't prove a loop body runs at all.

### 9. Void-call duality is resolved by *which typed node* wraps the call, not by a flag

`check_method_call` (shared) determines the callee's owner, signature, and typed
arguments, and returns one `TypedMethodCall` regardless of context. The two void/
non-void legality checks — `E_NONVOID_CALL_AS_STATEMENT` (statement position, non-void
result discarded) and `E_VOID_CALL_IN_EXPRESSION` (expression position, no value to
use) — happen once each, at the two call sites that wrap the result in `TypedStmt::
CallStmt` vs. `TypedExpr::Call`. **The expression-position check was missing entirely
in the prior draft's implementation** despite being claimed as covered — `check_expr`'s
`Expr::Call` arm wrapped whatever return type came back, including `Void`, with no
check at all. Fixed by adding the same shape of check the statement side already had,
symmetric on the other side of `Type::Void`.

---

## What downstream phases get

`check_program` hands back the `TypedProgram` and the `ClassTable` that produced it.
Per-node facts that a consumer needs (a call's resolved owner and signature, a cast's
direction, a variable's binding kind) are on the typed nodes directly, non-optionally.
Whole-class facts that aren't naturally per-node (effective methods for vtable slot
assignment, ancestor chains for `is_subtype` queries codegen might still want to run
itself) stay on `ClassTable`, returned alongside. `New`'s constructor selection is
recorded implicitly by which typed args accompany it; vtable slot indices are still
deliberately not this checker's output (see Decisions).

---

## Open items

Genuinely unresolved — need a decision, not a team assumption.

1. **~~Is `=` (or `<`/`>`) defined on class-typed operands at all?~~ Resolved.**
   Confirmed against the canonical language reference during review: no, they are not
   legal on class-typed operands. `(out = err)` is `E_BINOP_TYPE_MISMATCH`.
2. **~~Ternary result type for related-but-unequal class types.~~ Resolved.** Least
   common ancestor — see Algorithms §7. This was the single largest correctness fix
   in this revision.
3. **Locally-invented codes need a path to the real vocabulary, or reconciliation.**
   The list has grown this pass. See Notes for the full inventory.
4. **Should `TypeError`/`ErrorCode` be unified with `parser.rs`'s `ErrorCode` into one
   shared diagnostic type** used by lexer/parser/checker alike? Deferred, not
   rejected.
5. **`E_ARITY_MISMATCH` and `E_DELEGATION_ARITY_MISMATCH` overlap in the vocabulary
   itself.** `E_ARITY_MISMATCH`'s own trigger text lists `super(...)`/`this(...)`
   alongside ordinary calls and `new`. This implementation always prefers the more
   specific code for a delegation failure — defensible, but the vocabulary text
   doesn't force that choice.
6. **Pass structure vs. the vocabulary's own phase taxonomy.** Raised in review: could
   the four passes instead mirror §5's categories (well-formedness / name-resolution /
   type-check / inheritance-check / cast-and-instanceof / entry-point) more directly?
   Considered and **not adopted**, for a reason worth stating plainly rather than
   dropping the suggestion silently: the vocabulary's categories are organized by
   *error kind*, and this design's four passes are organized by *data dependency* —
   many name-resolution-category errors (`E_UNKNOWN_METHOD`, `E_UNKNOWN_VARIABLE`)
   literally cannot be checked until body-checking, long after well-formedness-category
   declaration checks, while others (`E_RESERVED_CLASS_NAME`) must happen at
   declaration time. Forcing one pass per vocabulary category would mean either
   re-validating already-known-good structure in a later pass or doing checks out of
   the order their data becomes available. The "Spec rule → pass → error code" table
   above already gives the cross-reference between the two schemes that motivated the
   suggestion — every vocabulary category maps onto one or more of the four passes,
   legibly, without forcing a 1:1 correspondence the underlying dependencies don't
   support.

---

## Notes

Settled facts and implementation guidance, not open questions.

- **Full inventory of codes needing course-staff reconciliation, as of this revision:**
  `E_BREAK_OUTSIDE_LOOP` (`break` outside a loop), `E_DUPLICATE_FORMAL` (duplicate
  formal parameter names — also flagged independently in `parser_design.md`'s own
  open items), `E_LOCAL_TYPED_VOID` (a local declared `void`), and reuse of the
  `..._OTHER` sentinels for: an inheriting-class constructor missing its required
  `super(...)`/`this(...)` first statement (`E_INHERITANCE_CHECK_OTHER`), an empty
  explicit constructor body (`E_WELL_FORMEDNESS_OTHER`), `super.m(...)` used inside a
  constructor body (`E_INHERITANCE_CHECK_OTHER`), instantiating `Input`/`Output`
  directly (`E_TYPE_CHECK_OTHER`), and something extending `Main`
  (`E_ENTRY_POINT_OTHER`) or a preamble class (`E_INHERITANCE_CHECK_OTHER`). All of
  these are genuine, spec-derived conditions with no dedicated published code — this
  is the complete list to take to course staff, not a partial one from an earlier
  draft.
- **`break;` outside a loop has no dedicated published code.** LO-2 §3.4.4. Checked
  in `check_bodies`, tracking "am I currently inside a `while`'s statement list" as a
  threaded boolean.
- **Reserved-name enforcement is entirely this checker's job**, per `lexer_design.md`.
- **`E_DUPLICATE_FIELD` operates on the same flattened, per-name list the parser
  already builds for fields.**
- **Duplicate formal parameter names were an outright gap, not just a missing
  diagnostic.** A prior draft's `Scope` inserted formals into a `HashMap` keyed by
  name with no duplicate check at all — `void foo(int x, int x)` would silently keep
  only the second `x`'s type, a real correctness bug (silent data loss), not merely an
  absent error message. Fixed by checking for the duplicate in Pass 1, where every
  other formal-parameter well-formedness check (void-typed, reserved-name) already
  lives, rather than in `Scope::build` — Pass 1 already walks every parameter list
  once; a second walk in Pass 4 would just repeat the same check redundantly per body.
- **Local variable declared types were never validated at all — another outright gap.**
  `Scope::build` inserted `decl.declared_type` straight into the map without ever
  checking it resolves (`Foo x;` for a nonexistent `Foo` passed silently) or that it
  isn't `void` (`void x;` also passed silently, with no dedicated code for it — see
  above). `Scope::build` now takes `&ClassTable` specifically to run the same
  `check_type_reference` helper Pass 2a uses for field/formal/return types, and
  rejects `Type::Void` before ever reaching the loop that inserts names into `Scope`.
- **`null` as a cast source, and as an `instanceof` source, were both incorrectly
  rejected — confirmed bugs against explicit spec text, not omissions.** §4.4.4:
  *"A cast applied to null always succeeds and produces null typed as the target."*
  §4.3.6: *"(null instanceof T) evaluates to false for any T."* A prior draft's
  pattern-matching on `ExprType::Concrete(Type::Class(_))` for both cast sources and
  `instanceof` sources rejected `NullLiteral` outright, contradicting spec text that
  had already been quoted correctly earlier in this document's own history — the gap
  was purely in translating the quoted rule into the actual match arms.
- **The interpreter and codegen consume the exact same `TypedProgram`**, plus the
  `ClassTable` alongside it. `check_bodies` must run to completion before either
  starts.
- **Null literal vs. null-valued expression, for `E_NULL_LITERAL_RECEIVER`:** this is
  a *syntactic* check on the receiver position specifically (`r.m(...)` where `r` is
  literally `Expr::Null`), not a data-flow analysis. A variable that happens to be
  null-typed or null-valued at runtime is fine statically and is exactly what
  `lo_abort_null_receiver` exists to catch dynamically.
- **A formal parameter that shares a field's name makes that field unassignable by
  bare identifier inside that constructor — a real language-level sharp edge, not a
  checker bug.** LO has no `this.field = value` assignment form anywhere in the
  grammar, and name resolution is local → formal → field. So `A(int x) { x = x; }`,
  written hoping to set field `x` from formal `x`, resolves *both* occurrences of `x`
  to the formal — the assignment is a no-op, and the field silently keeps its
  type-default forever. Every worked example in the language reference sidesteps this
  by giving the formal a different name than the field — that's the only way to write
  a correct explicit constructor when a formal and a field would otherwise collide.

---

## Alternate designs considered, not chosen

| Option | Rejected because |
|---|---|
| Accumulate all diagnostics across the whole program, report every one, instead of failing fast | Breaks convention with the already-established `LexError`/`ParseError` fail-fast shape for no reason specific to this phase. |
| `ClassId` interning instead of `String` keys | Solves a scale problem LO programs don't have. |
| Vtable slot assignment inside the type checker | Slot numbering is a pure object-layout fact, not a well-formedness fact, and keeping it out keeps `ClassTable` cleanly reusable by the interpreter. |
| Interleave declaration-gathering and body-checking in one pass | LO permits calling a method on a class declared later in the file, so a single-pass design would need speculative/patch-later bookkeeping a clean two-phase split avoids for free. |
| **The prior draft's "no second type, `check_program` just returns `Checked(Program)` + `ClassTable`, codegen re-derives everything on demand" position** | This was the position team review actually rejected, so it's worth stating precisely why the rejection is *not* a reversal of the *earlier* rejection (in-place `Cell` fields on `ast.rs`). Those were two separable objections: (1) mutating a file another phase's implementation plan owns, and (2) having two hand-written type definitions to keep in sync (`parser_design.md`'s original concern about parallel ASTs). The `Cell`-on-`ast.rs` plan had *both* problems. A standalone `TypedProgram` living in `sema.rs` has only the second, which the team judged worth accepting for the correctness and clarity of every downstream consumer *not* having to independently re-derive binding kinds, call targets, and cast directions from a bare `ClassTable` and an untyped tree. |
| A `CastDirection::NullNoop` variant distinct from `Upcast`, for a `null`-sourced cast | Would exist for exactly one call site's benefit; `Upcast` (meaning "codegen emits no runtime check") already says precisely the thing that call site needs said. |
| Giving `TypedExpr::Null` a synthetic resolved `Type` (e.g., defaulting to the nearest enclosing context's expected type, threaded bidirectionally) | Would require turning `check_expr` from a bottom-up into a bidirectional (expected-type-in, actual-type-out) type checker throughout, to thread "what type is expected here" down into every `null` literal — a much larger architectural change than the typed-AST adoption this revision already makes, for a case (a lone `null` needing its own resolved type independent of where it sits) that doesn't actually arise: every real consumer of a `TypedExpr::Null` already has the surrounding context's type in hand. |
