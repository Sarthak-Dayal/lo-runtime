# Type Checker Design — LO (LiveOak) P1

Scope: `Program` (the parser's AST, LO-4-complete) in, either `Ok(Checked)` or the first
`TypeError` out. Preamble injection (`Input`/`Output`, `in`/`out`/`err`) is part of this
stage — it's semantic synthesis, not syntax. The interpreter and codegen are separate
documents; this one settles what they get to assume once `check_program` returns `Ok`.

---

## Decisions

| Decision | Why |
|---|---|
| Four gated passes — `gather_declarations` → `resolve_inheritance` → `check_entry_point` → `check_bodies` — each requiring the previous to succeed before it runs | `parser_design.md` already names the first of these "the type checker's Pass 1." Body-checking needs the *whole* class table (a method may call a method declared later, in a class declared later in the file — LO-2 §2.3.2's "use before definition is allowed" for callables, and LO has no forward-declaration syntax to require otherwise), so declarations must be fully gathered before any body is walked. Running a later pass over a class table a prior pass already found broken produces cascading noise, not signal, so each pass gates the next rather than collecting independent errors across passes. |
| Vtable slot assignment is **not** part of the type checker | `parser_design.md`'s own open item: *"The type-checker design should settle whether vtable layout is literally inside Pass 1 or a dependent step right after it."* Settled here: it's neither — it's codegen's declaration pre-scan, which consumes this checker's `effective_methods` table (name, signature, owning class, override-or-not) as input and assigns slot indices itself. Type-checking correctness never needs a slot number, only "does this override an ancestor's method with an identical signature" (Runtime ABI chapter §4.3's parent-first-slot-inheritance rule is a layout fact, not a well-formedness fact). Keeping slots out of the checker also keeps `effective_methods` reusable by the interpreter, which dispatches by name lookup on the receiver's *runtime* class and never touches a vtable at all. |
| No parallel "checked AST" type, **and no in-place `Cell` annotations on `ast.rs` either.** `check_program` consumes the `Program` and returns a `Checked(Program)` marker (a zero-cost wrapper, not a restructured tree) alongside the `ClassTable` it built | `parser_design.md`'s alternate-designs table rejected two parallel AST types ("Avoids two hand-written type definitions that have to be kept in lockstep") and anticipated this document going further and caching per-node facts (cast direction, a variable's binding kind, a call's resolved return type) directly on `ast.rs` via `Cell` fields. An implementation review caught that this checker never actually reads any of those caches back for its own operation — `check_expr` already gets everything it needs through ordinary `Result` return values — and that the fields existed purely to serve a codegen phase that doesn't exist yet, whose actual needs are unknown. That's designing for a hypothetical future requirement, at the cost of a genuinely invasive change to a file `parser_implementation.md` owns. Cut entirely: a future codegen pass gets the `ClassTable` back from `check_program` and re-derives whatever it needs (is this a cast upcast or downcast, is this variable a field or a local) with the same small, cheap helper functions (`is_subtype`, `resolve_name`) this checker already has, on its own schedule, with zero `ast.rs` changes required now. |
| Preamble injection (`inject_preamble`) is its own named step, called once before `check_program`, not folded silently inside it | `parser_design.md` frames it the same way: *"a small named step... between parsing and checking."* Its output — a `Program` with `Input`/`Output` present as ordinary `ClassDecl`s — is what the interpreter and codegen consume downstream too, so it isn't type-checker-private setup; it's a pipeline stage in its own right that happens to live in `sema.rs` because synthesizing two `ClassDecl`s is a semantic-layer concern, not a lexical or syntactic one. |
| Effective fields and effective methods are computed **once per class**, memoized, in topological order over the (by then validated-acyclic) `extends` forest | Every field read and every method call would otherwise re-walk the ancestor chain from scratch. LO-4 inheritance is a forest of bounded, shallow depth (no course test program has anything resembling a deep hierarchy), but there's no reason to re-derive the same answer once per use site when it's knowable once per class, up front. |
| Per-body name resolution uses one flat `HashMap<String, Type>` (formals + `BodyScope.locals`), built once before any statement is checked — never threaded/discovered sequentially | `parser_design.md`'s hoisting decision already guarantees `BodyScope.locals` holds every local in the body, from arbitrary nesting depth, each visible from the top of the body ("a use may precede its declaration"), with no two locals sharing a name (`E_DUPLICATE_LOCAL`, checked by the *parser*). The type checker therefore never needs to discover locals in program order or worry about a declaration point — that concern was resolved one phase earlier. See "Preconditions inherited from the parser" below. |
| `Type::String` is handled as its own case throughout — never folded into the general class-type machinery | It's a distinct AST variant (parser's choice, noted in `parser_design.md`'s "Notes"), it's not a legal `extends` target, cast target, or `instanceof` target (structurally unreachable, since `String` is a lexer keyword and can never occupy a `ClassName` slot), and it has no ancestors/descendants to walk. Every subtype/cast/effective-member function that takes a `Type::Class(_)` must either reject `Type::String` outright or never be called with it — this document treats it as the latter (see "Algorithms" below), rather than adding defensive branches that can never actually fire given the grammar. |
| Fail-fast: `Result<T, TypeError>`, stop at the first error | Matches `LexError`/`ParseError`'s existing shape and the parser's stated convention. Not a confirmed course requirement any more than the parser's own fail-fast choice was (see Open items) — adopted for the same code-review-legibility reason `parser_design.md` gives, and for consistency across all three front-end phases now sharing one error-handling shape. |
| Classes, fields, and methods are keyed by `String` in `HashMap`s — no `ClassId`/interning layer | LO programs are course-scale (tens of classes at most). An interning pass would add a whole extra id-allocation concern for no measurable benefit at this size. |
| `is_subtype(a, b)` is a linear scan of a precomputed ancestor-chain `Vec<String>` (root-terminated, **not** including `a` itself — see Data model) | LO-4 is single inheritance — chains are short. A general subtyping lattice, memoized joins, or anything fancier would be solving a harder problem than the one the language actually has. |
| `resolve_inheritance` iterates classes in source declaration order, not `HashMap` key order | An implementation-review pass caught this: iterating a `HashMap`'s keys directly, as an early draft did, means *which* error/code comes back for a program with more than one independent violation is not guaranteed stable across runs — a real flakiness risk given the whole grading strategy is fail-fast + substring-matching one specific code. `gather_declarations` already sees classes in source order (it walks `Program.classes`, a `Vec`); `ClassTable` now records that order explicitly for `resolve_inheritance` to reuse, rather than re-deriving something weaker from the map. |

---

## Naming

| Name | What it is | Considered and rejected |
|---|---|---|
| `ClassTable` | The whole-program registry built by `gather_declarations`, consumed by every later pass | `SymbolTable` — over-general; LO's only top-level symbols this table needs to hold are classes. Locals/formals get their own, separate `Scope` (below), never merged into this one. |
| `ClassInfo` | Per-class record: own declarations (Pass 1) plus derived inheritance data (Pass 2) | Splitting into `ClassDecl`-mirroring vs. derived-data structs was considered — rejected as two things to keep in sync for a size of data (a handful of `Vec`s and `Option<String>`s per class) that doesn't justify the split. |
| `Scope` | Per-body flat `name → Type` map (formals + hoisted locals), built once at the start of `check_body` | `LocalScope` — redundant; nothing in this design ever needs a *non*-local scope object to distinguish it from (program-scope `in`/`out`/`err` are handled as a fixed final lookup tier, not a `Scope` value — see Algorithms §5). |
| `TypeError` / `ErrorCode` | Mirrors `ParseError`/`ErrorCode` and `LexError`/`LexErrorKind` exactly: `{ code, line, message }` plus `as_str()` | Reusing `parser.rs`'s `ErrorCode` directly was considered and deliberately deferred, not rejected outright — see Open items. Keeping it separate for now avoids blocking this design on a cross-branch decision, the same way the lexer's and parser's error enums already coexist independently despite an identical shape. |
| `gather_declarations`, `resolve_inheritance`, `check_entry_point`, `check_bodies` | The four passes | Named for what they produce/verify, not `pass1`/`pass2`/etc. — matches `parse_class_decl`-style naming already established for the parser (name states the job, not the position in a sequence). |

---

## Preconditions inherited from the parser

By the time a `Program` reaches `sema::check_program`, the following already hold —
established in `parser_design.md`/`parser_implementation.md` — and must **not** be
re-checked here, only relied on:

- Every `BodyScope.locals` entry has a name distinct from every other local in the
  *same* body (`E_DUPLICATE_LOCAL`, parser-level), across arbitrary `if`/`while`
  nesting depth. **Not** yet checked against the enclosing method's formals — that
  stays checker-phase (`E_LOCAL_SHADOWS_FORMAL`), per `parser_design.md`'s own stated
  principle: *"respect the published phase categorization when one exists."*
- A constructor's name equals its enclosing class's name (`E_MALFORMED_CONSTRUCTOR`,
  parser-level).
- Constructor delegation is well-*shaped*: at most one of `this(...)`/`super(...)`,
  appearing only as the true first statement, anywhere in the body including nested
  blocks (`E_DELEGATION_BOTH_SUPER_AND_THIS` / `E_DELEGATION_NOT_FIRST_STATEMENT`,
  parser-level). The checker only resolves whether the delegation's *target* is
  legal (arity match, no cycle) — never re-verifies position or keyword exclusivity.
- `Type::Class("String")` never occurs. `String` is a lexer keyword; it cannot fill a
  `ClassName` slot anywhere the grammar uses one (`extends`, `new`, cast target,
  `instanceof` target, field/formal/return type written as a bare class name — that
  last one still produces `Type::String` directly, a distinct variant, not
  `Type::Class(_)`).
- Every `if`/`while` body is a flat `Vec<Stmt>` with no `VarDecl`s of its own — all
  hoisted to the enclosing `BodyScope`.
- A method body has at least one statement; a constructor body may have zero
  (`Block` vs. the constructor's own inline production — `parser_design.md`, "Grammar
  → function mapping").

---

## Data model

```rust
struct ClassTable {
    classes: HashMap<String, ClassInfo>,
    order: Vec<String>,                   // source declaration order — see Decisions
}

struct ClassInfo {
    decl_line: u32,
    parent: Option<String>,               // Pass 2a, resolved + validated
    own_fields: Vec<(String, Type)>,       // Pass 1, flattened per-name (see below)
    own_methods: Vec<MethodSig>,           // Pass 1
    own_constructors: Vec<ConstructorSig>, // Pass 1

    // Filled in by resolve_inheritance (Pass 2), memoized:
    ancestors: Vec<String>,                // strict ancestors only (self excluded), root last
    effective_fields: Vec<(String, Type, /* owner */ String)>,  // parent-first order
    effective_methods: HashMap<String, (/* owner */ String, MethodSig)>,
}

struct MethodSig {
    name: String,
    params: Vec<Type>,
    return_type: Type,
    line: u32,
}

struct ConstructorSig {
    arity: usize,
    params: Vec<Type>,
    line: u32,
    this_target_arity: Option<usize>, // Some(n) iff this ctor's own delegation is
                                       // this(...) targeting the n-arity constructor
                                       // of the same class — see Algorithms §4.
}
```

`own_fields` is per-*name*, not per-`VarDecl`: `ClassDecl.fields: Vec<Param>` already
arrives pre-flattened by `parse_field_list` (`int a, b;` becomes two `Param`s), so
`E_DUPLICATE_FIELD` is checked across this flattened list directly — same algorithm
shape the parser already uses for `E_DUPLICATE_LOCAL`, one phase later, because the
published vocabulary files this one under well-formedness, not parse-phase.

`is_subtype`'s special-case for `a == b` is what makes excluding self from `ancestors`
safe — a class is trivially its own subtype without needing to appear in its own
ancestor list. (An earlier draft of this document said "self first" here while the
implementation never actually included it; the implementation was right, the comment
wasn't — fixed to match.)

---

## Spec rule → pass → error code

| Rule | Pass | Code(s) |
|---|---|---|
| Class names unique program-wide | 1 | `E_DUPLICATE_CLASS_NAME` |
| `Main`/`Input`/`Output` not redeclared by user code (String already impossible, see above) | 1 | `E_RESERVED_CLASS_NAME` |
| Field names unique per class (flattened) | 1 | `E_DUPLICATE_FIELD` |
| Method names unique per class (no overloading, ever) | 1 | `E_DUPLICATE_METHOD` |
| Constructor arities distinct within a class | 1 | `E_DUPLICATE_CONSTRUCTOR_ARITY` |
| No field/formal typed `void` | 1 | `E_FIELD_TYPED_VOID` / `E_FORMAL_TYPED_VOID` |
| Every class name used in a signature (`extends`, field/formal/return type) resolves | 2a | `E_UNKNOWN_CLASS` |
| No `extends` cycle | 2b | `E_INHERITANCE_CYCLE` |
| No field name in a subclass repeats an ancestor's effective field | 2c | `E_FIELD_SHADOWING` |
| Same-name-same-signature override only; same-name-different-signature is always illegal | 2c | `E_OVERRIDE_SIGNATURE_MISMATCH` |
| A class with `extends` has an explicit `[ ]` constructor section | 2c | `E_MISSING_CONSTRUCTOR_IN_INHERITING_CLASS` |
| `Main` declared, no `extends`, `int main()` with no formals, zero-arg constructor reachable | 3 | `E_NO_MAIN_CLASS`, `E_MAIN_CLASS_EXTENDS`, `E_NO_MAIN_METHOD`, `E_MAIN_METHOD_SIGNATURE`, `E_MAIN_NO_ZERO_ARG_CONSTRUCTOR` |
| Identifier resolves (local → formal → field → `in`/`out`/`err`) | 4 | `E_UNKNOWN_VARIABLE` |
| Local doesn't repeat a formal's name | 4 | `E_LOCAL_SHADOWS_FORMAL` |
| `in`/`out`/`err` not redeclared as a field, formal, or local | 1 (fields, method/constructor formals) and 4 (locals — Pass 1 never opens a body, so this half can't happen any earlier) | `E_RESERVED_VARIABLE_NAME` |
| Method lookup succeeds on the receiver's static type's effective methods | 4 | `E_UNKNOWN_METHOD` |
| Assignment / return / actual-argument compatibility (`<:` closure, `null` special-cased) | 4 | `E_ASSIGN_TYPE_MISMATCH`, `E_RETURN_TYPE_MISMATCH`, `E_ACTUAL_TYPE_MISMATCH` |
| Operator operand typing (int/bool/String only, per operator) | 4 | `E_BINOP_TYPE_MISMATCH`, `E_UNOP_TYPE_MISMATCH` |
| Ternary branch compatibility | 4 | `E_CONDITIONAL_TYPE_MISMATCH` |
| Ordinary call / `new` arity matches | 4 | `E_ARITY_MISMATCH` |
| Receiver has class type; not literally `null` | 4 | `E_RECEIVER_NOT_CLASS_TYPE`, `E_NULL_LITERAL_RECEIVER` |
| Void/non-void call used in the right position | 4 | `E_NONVOID_CALL_AS_STATEMENT`, `E_VOID_CALL_IN_EXPRESSION` |
| Return-path completeness / shape | 4 | `E_RETURN_MISSING`, `E_RETURN_IN_VOID_METHOD`, `E_RETURN_IN_CONSTRUCTOR` |
| `break;` only inside an enclosing `while` | 4 | *(unpublished — see Notes)* |
| `super(...)`/`super.m(...)` only when a parent exists; target resolves; no delegation cycle; delegation arity matches | 4 | `E_SUPER_IN_ROOT_CLASS` (constructor `super(...)`), `E_SUPER_METHOD_IN_ROOT_CLASS` (method-body `super.m(...)` — a *different* code from the constructor case, easy to conflate since both mean "no parent"), `E_SUPER_METHOD_UNRESOLVED`, `E_DELEGATION_CYCLE`, `E_DELEGATION_ARITY_MISMATCH` |
| Cast target/source are class types; one is a subtype of the other | 4 | `E_CAST_TARGET_NOT_CLASS`, `E_CAST_SOURCE_NOT_CLASS`, `E_CAST_UNRELATED_TYPES` |
| `instanceof` source is a class type (target: any declared class, no relation required) | 4 | `E_INSTANCEOF_SOURCE_NOT_CLASS` |

Every phase also carries its own `..._OTHER` sentinel (`E_WELL_FORMEDNESS_OTHER`,
`E_NAME_RESOLUTION_OTHER`, `E_TYPE_CHECK_OTHER`, `E_INHERITANCE_CHECK_OTHER`,
`E_CAST_INSTANCEOF_OTHER`, `E_ENTRY_POINT_OTHER`) for a genuinely uncategorized failure
in that phase — not used unless nothing more specific fits, exactly as the vocabulary
frames them.

`E_THIS_OUTSIDE_INSTANCE` is in the published vocabulary but has no reachable trigger
given this grammar: `this`/`super` only ever parse inside a method or constructor body
(there is no other place a `Stmt`/`Expr` exists), so "used outside an instance context"
can't occur structurally. Documented here as intentionally dead — not omitted by
oversight.

---

## Algorithms

### 1. Effective-member computation order

`resolve_inheritance` walks the (validated-acyclic) `extends` forest bottom-up: a
class's `effective_fields`/`effective_methods` are computed only after its parent's
are already sitting in the table, so computing them is "clone the parent's effective
set, then extend/override with this class's own declarations" — never a fresh
ancestor-chain walk. A simple post-order DFS from each root, memoizing as it goes (skip
any class already computed, e.g. reached again via a sibling), visits every class
exactly once.

### 2. Override matching walks the *full* ancestor chain, not just the direct parent

§4.3.3 of the language reference: a method `m` in `C` overrides `m` in `P` for *any*
`P` with `C <: P`, not only `C`'s immediate parent — a grandchild can override a
grandparent's method the middle class never touched. Because `effective_methods` is
already the parent's full inherited set (step 1), checking a new declaration against
it automatically reaches every ancestor, not just the direct one — there's no separate
"walk up further" step needed; it falls out of the memoized-clone construction.

### 3. Field shadowing likewise checks the full ancestor chain

Same reasoning: `E_FIELD_SHADOWING` compares a new field name against the parent's
*effective* fields (already the union of the whole chain above it), not just the
parent's own declared fields.

### 4. Cycle detection — one generic helper, two call sites

Both `E_INHERITANCE_CYCLE` (over `extends` edges) and `E_DELEGATION_CYCLE` (over
`this(...)` edges within one class's constructors) are the same shape: a directed
graph, DFS with a "currently on the path" visiting-set, cycle iff a back-edge is found.
One generic `detect_cycle` helper, parameterized by an edge-lookup closure, serves
both — no reason to write the traversal twice.

The `extends` case reads its edges straight from `ClassInfo.parent`. The `this(...)`
case needs each constructor's delegation *target* as data, which lives on the AST's
`ConstructorDecl.delegation`, not on anything `ClassTable` stores by default — so
`ConstructorSig` carries one extra field, `this_target_arity: Option<usize>`,
populated once in Pass 1 directly from `ctor.delegation` (`Some(args.len())` for a
`ThisCall`, `None` otherwise). This is what lets the delegation-cycle check run
entirely off `ClassTable` in Pass 4, with no second reference to the raw AST needed.
It also means the check is naturally run once per *class* (over the whole set of
`this_target_arity` edges at once), not once per constructor — running it once per
constructor would just rediscover the same cycle up to *n* times for an *n*-constructor
class, for no added information.

### 5. Name resolution order inside a body

Three tiers, first match wins, exactly LO-3 §3.3.3: locals (from `Scope`) → formals
(also in `Scope` — see Data model, both land in the same flat map since no name can be
in both, that being `E_LOCAL_SHADOWS_FORMAL`'s whole point) → fields (via the
enclosing class's `effective_fields`, so inherited fields resolve too). A fourth tier
— `in : Input`, `out : Output`, `err : Output` — is checked last, and only ever
matters for `Main`'s own methods and any class that doesn't otherwise shadow those
names (it can't; they're reserved). Implemented as a fixed three-entry lookup after the
other tiers miss, not folded into `Scope` or `effective_fields` — these bindings exist
program-wide, not per-class or per-body, and treating them as a `Scope` member would
misstate their actual scope.

### 6. `super.m(...)` resolves differently from an ordinary call

An ordinary `r.m(...)` resolves `m` in `r`'s static type's `effective_methods` — which
already includes everything inherited-and-not-overridden, so no extra walking is
needed. `super.m(...)` is different on purpose: it must resolve in the *static
parent's* `effective_methods`, deliberately bypassing the current class's own
override (that's the entire point of the form — "call the version I'm overriding").
Two distinct lookup paths, not one shared helper with a flag, because the receiver
concept doesn't apply to `super` the way it does to an ordinary `<ObjName>` — `super`
is checked for legality (`E_SUPER_IN_ROOT_CLASS`, `E_SUPER_METHOD_IN_ROOT_CLASS`) using
the *enclosing method's* class, not any expression's static type.

### 7. Cast legality is the one genuinely bidirectional check

```rust
fn cast_is_legal(target: &str, source: &str, table: &ClassTable) -> bool {
    table.is_subtype(source, target) || table.is_subtype(target, source)
}
```

This checker only needs *legality*, not which direction the cast goes — upcast
(no runtime check needed) vs. downcast (needs `lo_cast_check` at runtime) is a fact
codegen will want later, but it's a single extra `is_subtype` call for codegen to make
for itself at that point, not something worth caching now for a phase that doesn't
exist yet (see "What downstream phases get").

`instanceof` has no such requirement — any two declared class types are legal; the
result is simply `false` at runtime when unrelated. Implementing `instanceof`'s check
by calling `cast_is_legal` and treating `false` as "fine, just always false statically
would be wrong" would conflate two genuinely different rules; `instanceof` only checks
that the source expression is class-typed and the target name resolves, nothing more.

### 8. Return-path completeness — the one control-flow analysis this checker does

```rust
fn definitely_returns(stmts: &[Stmt]) -> bool {
    match stmts.last() {
        Some(Stmt::Return(..)) => true,
        Some(Stmt::If(_, then_branch, else_branch, _)) =>
            definitely_returns(then_branch) && definitely_returns(else_branch),
        _ => false,
    }
}
```

`while` never contributes — the checker can't prove a loop body runs at all, let alone
that it returns before exiting, so a `return` inside a `while` never satisfies
`E_RETURN_MISSING` for the enclosing method on its own. This is the only place body
checking reasons about *sequences* of statements rather than one statement at a time;
everywhere else, `check_stmt`/`check_expr` are purely local, bottom-up type
derivations.

### 9. Void-call duality is resolved by *which AST variant* holds the call, not by a flag

`Stmt::CallStmt(MethodCall)` and `Expr::Call(MethodCall)` wrap the identical struct.
`check_method_call` (shared) only ever determines the callee's return type; the
void/non-void legality check (`E_NONVOID_CALL_AS_STATEMENT` / `E_VOID_CALL_IN_EXPRESSION`)
happens once at each of the two call sites in `check_stmt`/`check_expr`, comparing that
returned type against `Type::Void` — never inside `check_method_call` itself, which has
no way to know which context it was called from and shouldn't need to.

---

## What downstream phases get

No `ast.rs` changes and no cached per-node facts. `check_program` hands back two
things once it succeeds: the `Checked(Program)` marker (proof, at the type level,
that a `Program` has actually been through this pass — nothing more than a one-line
newtype, not a restructured tree) and the `ClassTable` itself. A future codegen
implementation plan is free to hold onto that table and call the same `is_subtype`,
`resolve_name`-shaped lookups this checker already has, at whatever point it actually
needs "is this cast an upcast or a downcast" or "is this variable a field or a local"
— those are single, O(1)-ish lookups, cheap enough that caching them ahead of time
buys nothing beyond what the hypothetical consumer will ask for on its own schedule,
which is exactly the point at which its actual requirements will be known instead of
guessed at here. `New`'s constructor selection is likewise a cheap `args.len()` lookup
wherever it's needed again, and vtable slot indices are deliberately not this
checker's output at all (see Decisions).

---

## Open items

Genuinely unresolved — need a decision, not a team assumption.

1. **Is `=` (or `<`/`>`) defined on class-typed operands at all (reference
   equality/ordering)?** LO-2 §3.1's operator table says, verbatim, *"No other operand
   combination is legal"* beyond int/bool/String — which reads as: comparing two
   `Output` references with `=` is `E_BINOP_TYPE_MISMATCH`. But `parser_design.md`
   contains a forward-looking inline comment — *"later for the type checker: out ==
   err must be false"* — that only makes sense if class-typed `=` is legal (checked as
   reference equality) and the pre-bound `out`/`err` singletons are expected to compare
   unequal. These two sources disagree. Needed before `E_BINOP_TYPE_MISMATCH` can be
   implemented for class-typed operands at all.
2. **Ternary result type when branches are related-but-unequal class types.**
   `E_CONDITIONAL_TYPE_MISMATCH`'s trigger text only says *"no common supertype"* —
   the reference never states what the expression's static type *is* when one does
   exist. `type_checker_implementation.md` picks a concrete rule (the same bidirectional
   `is_subtype` check used for casts, resulting in the wider of the two types) so
   implementation isn't blocked, but this is a team/course-staff call, not a
   spec-derived fact.
3. **Locally-invented codes need a path to the real vocabulary, or reconciliation.**
   The parser already ships `E_DUPLICATE_LOCAL`, absent from the published vocabulary.
   This document proposes a second one, `E_BREAK_OUTSIDE_LOOP` (see Notes) — same
   situation. Both should go to course staff as vocabulary-addition requests, or get
   folded into their phase's `..._OTHER` sentinel if staff would rather not add codes
   mid-semester.
4. **Should `TypeError`/`ErrorCode` be unified with `parser.rs`'s `ErrorCode` into one
   shared diagnostic type** used by lexer/parser/checker alike? Identical shape today,
   kept separate per-phase for independence (Decisions). Worth revisiting once all
   three phases exist and the duplication is visible in the actual crate, not before.
5. **`E_ARITY_MISMATCH` and `E_DELEGATION_ARITY_MISMATCH` overlap in the vocabulary
   itself.** Missed in the first pass over the vocabulary, found during implementation
   review: `E_ARITY_MISMATCH`'s own trigger text is *"A method call, **new, super(...),
   or this(...)**, provides a wrong number of actual arguments"* — which already covers
   the delegation case that `E_DELEGATION_ARITY_MISMATCH` also exists specifically for.
   `type_checker_implementation.md` resolves this by always preferring the more
   specific code (`E_DELEGATION_ARITY_MISMATCH`) for a delegation failure and reserving
   `E_ARITY_MISMATCH` for ordinary calls/`new` — a defensible reading, but the
   vocabulary text itself doesn't force that choice, so it's listed here rather than
   silently assumed.

---

## Notes

Settled facts and implementation guidance, not open questions.

- **`break;` outside a loop has no dedicated published code.** LO-2 §3.4.4: *"`break;`
  appears only inside an enclosing `while`."* `parser_design.md` already flags this
  exact situation for a sibling check (`E_LOCAL_SHADOWS_FORMAL`'s neighbor,
  referred to there as `E_BREAK_OUTSIDE_LOOP`) as *"explicitly filed under
  well-formedness (checker-phase)"* despite being just as locally trackable during
  parsing as the parser's own `E_DUPLICATE_LOCAL`. This document follows that same
  precedent and keeps the check here, in `check_bodies`, tracking "am I currently
  inside a `while`'s statement list" the same way the parser tracks "am I inside a
  constructor body" for the delegation checks — a threaded boolean, not global state.
- **Reserved-name enforcement is entirely this checker's job.** `lexer_design.md` is
  explicit: `in`/`out`/`err`/`Main`/`Input`/`Output` lex as ordinary `Ident`s, "enforced
  as reserved later, at name resolution." Nothing upstream does any of this work.
- **`E_DUPLICATE_FIELD` operates on the same flattened, per-name list the parser
  already builds for fields** (`parser_design.md`'s "Grammar → function mapping"
  section, on `parse_field_list`) — this checker doesn't re-flatten anything, it
  consumes `ClassDecl.fields: Vec<Param>` as-is.
- **The interpreter and codegen consume the exact same `Checked(Program)`** this
  checker produces, plus the `ClassTable` alongside it (see "What downstream phases
  get"). `check_bodies` must run to completion before either starts — there is no
  partial/best-effort `Checked` value, only `Ok` after every pass succeeds or an `Err`
  with nothing usable.
- **Null literal vs. null-valued expression, for `E_NULL_LITERAL_RECEIVER`:** this is a
  *syntactic* check on the receiver position specifically (`r.m(...)` where `r` is
  literally `Expr::Null`), not a data-flow analysis. A variable that happens to be
  null-typed or null-valued at runtime is fine statically and is exactly what
  `lo_abort_null_receiver` exists to catch dynamically (Runtime ABI chapter §3.8) —
  this checker makes no attempt to prove or disprove nullability beyond the literal
  case.
- **A formal parameter that shares a field's name makes that field unassignable by
  bare identifier inside that constructor — a real language-level sharp edge, found
  while writing acceptance tests, not a checker bug.** LO has no `this.field = value`
  assignment form anywhere in the grammar (P17 is bare `<Var> = <Expr>;`, full stop),
  and name resolution is local → formal → field. So a constructor like
  `A(int x) { x = x; }`, written hoping to set field `x` from formal `x`, resolves
  *both* occurrences of `x` to the formal — the assignment is a no-op, and the field
  silently keeps its type-default forever. Nothing in the well-formedness rules
  forbids a formal sharing a field's name, so this type-checks without complaint and
  just produces a program that doesn't do what its author intended. Every worked
  example in the language reference sidesteps this by giving the formal a different
  name than the field (`Counter(int initial) { count = ...; }`) — that's not a
  coincidence, it's the only way to write a correct explicit constructor when a
  formal and a field would otherwise collide. Recorded here so nobody mistakes silent
  self-assignment for a checker defect later.

---

## Alternate designs considered, not chosen

| Option | Rejected because |
|---|---|
| Accumulate all diagnostics across the whole program, report every one, instead of failing fast | Would be a real usability improvement, but breaks convention with the already-established `LexError`/`ParseError` fail-fast shape for no reason specific to this phase. If the team wants to switch, switch all three phases together, not just this one. |
| `ClassId` interning instead of `String` keys | Solves a scale problem LO programs don't have (see Decisions). Adds an id-allocation concern and a second name↔id lookup direction for no measured benefit. |
| Vtable slot assignment inside the type checker | Resolves `parser_design.md`'s open question the other way. Rejected because slot numbering is a pure object-layout fact (Runtime ABI chapter §2, §4.4), not a well-formedness fact, and keeping it out keeps `effective_methods` cleanly reusable by the interpreter, which has no vtable concept at all. |
| Interleave declaration-gathering and body-checking in one pass, patching forward references as they resolve | LO permits calling a method on a class declared later in the file, so a single-pass design would need speculative/patch-later bookkeeping for exactly the case a clean two-phase split (gather everything, then check bodies) avoids for free. Not worth the complexity. |
| Full least-upper-bound search across the inheritance forest for the ternary's result type | Overkill relative to reusing the same bidirectional `is_subtype` check already built for casts (Algorithms §7) — and the reference doesn't ask for LUB semantics anywhere else, so introducing a lattice-join operation just for `?:` would be new machinery serving one call site. See Open item #2. |
| Caching cast-direction, variable-binding-kind, and call-return-type on the AST — either as in-place `Cell` fields on `ast.rs`, or as a side table (`HashMap<NodeId, T>`) keyed by a synthetic node id | Both were the original plan; both are cut (see Decisions and "What downstream phases get"). Neither is needed by this checker's own operation, both serve a codegen phase that doesn't exist yet and whose actual needs are unknown, and the `Cell` variant additionally requires an invasive, unrequested change to a file `parser_implementation.md` owns. A future codegen plan can re-derive any of these facts on demand from the `ClassTable` this checker already hands back. |
