# Interpreter Implementation Plan — LO (LiveOak) P1

Implements everything decided in `interpreter_design.md`, against the `compiler/` crate
as it exists on `type-checker/phase-1-implementation` — the branch that introduces the
typed AST and class table in `type_checker.rs` (`TypedProgram`, `TypedExpr`, `TypedStmt`,
`MethodResolution`, `CastDirection`, `BindingInfo`, `ClassTable`, …). The interpreter
**consumes those types read-only**; it neither re-checks nor rebuilds them. `ast.rs` and
`type_checker.rs` stay untouched.

**Scope for this step: the interpreter core as a library.** The public surface is one
function, `interpret(&TypedProgram, &ClassTable, …) -> Outcome`, exercised by unit tests.
Wiring a CLI (`check`/`interpret`/`compile` modes into `main.rs`), the WASM back end, and
the end-to-end conformance runner are each separate plans (see *Out of scope*). Because
there is no driver yet, an abort is **returned** as a structured value, never
`process::exit` — the future driver renders the message and exits.

---

## Files to create / modify

| File | Change |
|---|---|
| `compiler/src/interpreter.rs` | New. Module root: `pub fn interpret`, `Outcome`, the run orchestration and startup indices, and `mod {value, heap, env, eval, strings, io, abort};`. |
| `compiler/src/interpreter/value.rs` | New. `Value` and `type_default`. |
| `compiler/src/interpreter/heap.rs` | New. `Heap`, `Cell`, `ObjId`/`StrId`, charging + OOM, string interning. |
| `compiler/src/interpreter/env.rs` | New. `Frame`, per-method slot map, `read_var`/`write_var`. |
| `compiler/src/interpreter/eval.rs` | New. `Signal`, expression/statement evaluation, operators, dispatch, casts, constructors. |
| `compiler/src/interpreter/strings.rs` | New. The five string operations. |
| `compiler/src/interpreter/io.rs` | New. Injectable I/O, buffered reader, write/read semantics. |
| `compiler/src/interpreter/abort.rs` | New. `AbortKind` with `code()` and `message()`. |
| `compiler/src/main.rs` | Add `mod interpreter;`. No CLI wiring this step. |
| `compiler/src/type_checker.rs`, `ast.rs` | **Untouched.** Consumed read-only. |

---

## Decisions resolved for this implementation

| Item | Decision |
|---|---|
| Interpreter input | `&TypedProgram` + `&ClassTable`, both from `type_checker::check_program`. The interpreter never sees the untyped `ast::Program`. |
| Class table | **Consume `ClassTable` directly.** The design doc's own bridge table is dropped; `effective_fields`, `effective_methods`, `is_subtype`, `parent`, `ancestors` are exactly what dispatch and layout need. |
| Method **bodies** | `effective_methods` yields a `MethodEntry` (owner + signature), not a body. Build a startup index `(owner_class, method_name) -> &TypedMethodDecl` from `TypedProgram`, plus class-by-name and constructor-by-arity indices. |
| Name resolution | Driven by the checker's `BindingInfo` on every `Var`/`Assign`/receiver — no re-resolution. `Local`/`Formal` → a per-method name→slot map; `Field` → an index into the runtime class's `effective_fields`; `Prebound` → the `in`/`out`/`err` singletons. |
| Dispatch | Driven by `MethodResolution`: `Virtual` looks up on the **runtime** class, `Super` is statically fixed to `declaring_class`, `Io` runs a built-in with no frame. |
| Casts | Driven by `CastDirection`: `Upcast`/`Null` pass through; only `Downcast` runs a runtime check. |
| `out` vs `err` | The finalized preamble `Output` has **no fields**, so there is no `fd` field to read. The three preamble singletons are allocated once at startup and their `ObjId`s recorded in interpreter state; a `print_*` op selects the stream by comparing the receiver's `ObjId` against the stored `err` id. |
| Aborts | A `Signal::Abort(AbortKind)` that unwinds to `interpret`, which returns `Outcome::Abort(kind)`. No process exit here. Keeps aborts unit-testable. |
| I/O | `stdin`/`stdout`/`stderr` are injected (any `BufRead`/`Write`), so tests capture output; `err` is written directly, which the interpreter can do for free (the ABI has no stderr codegen path — flagged in the design doc). |
| Memory | Interpreter-owned arena, byte budget (default 64 MiB), no GC. OOM aborts 137. Rationale in `interpreter_design.md` §3. |

---

## `src/interpreter.rs`

### Public entry and `Outcome`

```rust
pub enum Outcome {
    /// Clean finish: `Main.main()`'s return value, to become the process exit status.
    Exit(i32),
    /// A runtime abort. Renders to the contractual (code, message) via `AbortKind`.
    Abort(abort::AbortKind),
}

pub fn interpret<R: BufRead, W: Write, E: Write>(
    program: &TypedProgram,
    table: &ClassTable,
    heap_limit: usize,
    io: &mut Io<R, W, E>,
) -> Outcome
```

`interpret` assumes a well-formed program (the checker rejected everything else). States
the checker made impossible become `unreachable!("interpreter invariant: …")`, never
user-facing errors.

### Run orchestration

1. Build the startup indices (below).
2. Allocate the three preamble singletons — one `Input`, two `Output` — and record their
   `ObjId`s (`in_id`, `out_id`, `err_id`).
3. Allocate `Main` with all fields at type-defaults and run its zero-arg constructor.
4. Call `Main.main()` on that object (direct dispatch).
5. Map a clean `Return(Value::Int(rv))` to `Outcome::Exit(rv)`; map any `Signal::Abort` that
   reaches this point to `Outcome::Abort`.

There is no separate init step — the interpreter has no heap to initialize beyond its arena.

### Startup indices

```rust
struct Program<'p> {
    classes: HashMap<&'p str, &'p TypedClassDecl>,          // by name
    method_bodies: HashMap<(&'p str, &'p str), &'p TypedMethodDecl>, // (owner, method) -> body
    // constructors are found by scanning a TypedClassDecl's `constructors` for the arity.
}
```

`method_bodies` is keyed by the **declaring** class, so a `Virtual` dispatch resolves the
override winner through `effective_methods[m].owner` and then indexes here; a `Super` call
indexes directly at `(declaring_class, m)`. `Io` methods have no body and never appear here.

---

## `src/interpreter/value.rs`

```rust
pub enum Value {
    Int(i32),
    Bool(bool),
    Str(StrId),           // never null — String is primitive-equivalent
    Obj(Option<ObjId>),   // None is the one and only LO null
}

pub fn type_default(ty: &Type, heap: &Heap) -> Value; // Int(0) / Bool(false) / Str(EMPTY) / Obj(None)
```

The `String` default is the interned **empty string**, not zero — the case most often gotten
wrong (mirrors the ABI's `LO_EMPTY_STRING`). Applied to fields at allocation and to locals at
frame setup.

---

## `src/interpreter/heap.rs`

```rust
pub struct ObjId(u32);
pub struct StrId(u32);

enum Cell {
    Obj { class: Box<str>, fields: Vec<Value> },  // class name keys back into ClassTable
    Str(Rc<str>),                                 // immutable; LO has no string mutation
}

pub struct Heap { cells: Vec<Cell>, charged: usize, limit: usize, empty: StrId }
```

- **Charging** mirrors the wasm32 layout so the budget corresponds to something real:
  an object of class `C` costs `12 + 4 * effective_fields(C).len()`; a string of `n` bytes
  costs `12 + 4 + n`. The check happens **before** the cell is pushed, so a failed
  allocation leaves the heap unchanged and raises `Signal::Abort(OutOfMemory)`.
- Objects store fields in `effective_fields` order (parent-first) — the same order the ABI
  mandates, which keeps interpreter field indices and codegen byte offsets cross-checkable.
- The empty string is interned at construction; `Str(EMPTY)` reuses it.

---

## `src/interpreter/env.rs`

```rust
pub struct Frame { this: Option<ObjId>, slots: Vec<Value> }
```

LO has flat, per-body scoping (block declarations are hoisted), locals may not shadow
formals, and fields may not be shadowed — so the visible names of a body are flat and known
before execution, with no scope stack. Build a per-method name→slot map from
`params ++ locals` once; the frame's `slots` is laid out in that order.

`read_var`/`write_var` dispatch on the checker's `BindingInfo`:

- `Local` / `Formal` — index into `frame.slots` via the name→slot map.
- `Field { .. }` — index into `this`'s runtime-class `effective_fields` by name (unique
  because shadowing is forbidden).
- `Prebound` — one of `in_id` / `out_id` / `err_id`.

---

## `src/interpreter/eval.rs`

### Control flow

```rust
enum Signal { Break, Return(Value), Abort(AbortKind) }
type Exec<T> = Result<T, Signal>;
```

One signal type unifies `break`, `return`, and abort under `?`. `Break` unwinds to the
nearest `while`; `Return` to the call boundary; `Abort` all the way to `interpret`.

### Operators

Dispatch reads the operand **values** (`Value::Int` vs `Value::Str`), not the static `ty` —
the evaluator's correctness does not depend on the checker's annotations. Integer semantics
are **total** and must not use Rust's default operators (which panic in debug):

| LO | Rust |
|---|---|
| `+ - * ~`(neg) | `wrapping_add` / `wrapping_sub` / `wrapping_mul` / `wrapping_neg` |
| `x / 0` | `-1` |
| `x % 0` | `x` |
| `INT_MIN / -1` | `INT_MIN` (`wrapping_div`) |
| `INT_MIN % -1` | `0` (`wrapping_rem`) |
| otherwise | `wrapping_div` / `wrapping_rem` |

`Binop::And` / `Or` (`&` / `|`) **short-circuit** — evaluate the right operand only if the
left does not decide the result. `Unop::Neg` (`~`) is integer negation on `Int` and string
reversal on `Str`; `Unop::Not` (`!`) is Boolean NOT. Comparisons (`Lt`/`Gt`/`Eq`) apply only
to `Int` and `String` — the checker rejects class-typed operands, so the evaluator never
sees them.

### Casts and `instanceof`

Both read the **runtime** class of the operand and are steered by the node the checker
already classified:

- `Cast { direction: Upcast | Null, .. }` — pass the value through unchanged (`Null` yields
  `Obj(None)`).
- `Cast { direction: Downcast, .. }` — `Obj(None)` passes; otherwise if the runtime class
  `is_subtype` of `target`, pass through, else `Abort(CastFailed { from, to })`.
- `InstanceOf { operand, class }` — `Obj(None)` → `false`; otherwise `is_subtype(runtime, class)`.
  Never aborts.

### Dispatch

Steered by `MethodResolution`:

- `Virtual { .. }` — evaluate the receiver; if `Obj(None)`, `Abort(NullReceiver { method })`.
  Else look up `method` in the receiver's **runtime**-class `effective_methods`, take the
  entry's `owner`, and run `method_bodies[(owner, method)]` in a fresh frame.
- `Super { declaring_class }` — statically fixed; run `method_bodies[(declaring_class, method)]`
  with `this` as receiver. The one call form that ignores the runtime class.
- `Io { op }` — no frame; run the built-in (see `io.rs`). For `print_*` the receiver
  (`out`/`err`) selects the stream; for `read_*`/`eof` the receiver is `in`.

Receivers: `This` and `Super` are never null; `Var`/`Computed` are null-checked before use.

### `new` and constructors

`New { class, args }` (only user classes — `Preamble` targets were rejected by the checker):

1. Allocate the object with every field (inherited included) at its type-default.
2. Select the constructor by **arity** from the class's `constructors`
   (`Explicit.params.len()` or `Implicit.fields.len()`).
3. Run the delegation prefix to completion first — `Super { args }` runs the parent's
   arity-matched constructor, `This { args }` a sibling — so exactly one constructor body
   runs per hierarchy level, parent portion first.
4. Run the body. `Implicit { fields }` executes as `this.field_i = formal_i` in field order.

At the point a `super(...)` returns, the parent's fields are set and this class's own fields
are still at type-defaults. A constructor need not assign every field.

### Statements

`Assign` (via `write_var`), `Return(e)` → `Signal::Return`, `If`, `While` (catching
`Signal::Break`), `Break`, `Empty`, `CallStmt` (a void call, discard the value).

---

## `src/interpreter/strings.rs`

All five operations follow ABI §3.2 exactly; note the unit each works in — a wrong unit only
fails on non-ASCII input, which the public corpus may not cover:

| LO | Operation | Semantics |
|---|---|---|
| `(a + b)` | concat | byte concatenation |
| `(s * n)` | repeat | `n` copies; **`n < 0` → `Abort(RepeatNegative(n))` (120)**; `n == 0` → empty |
| `(~s)` | reverse | **by codepoint** (`chars().rev()`), not by byte |
| `(a < b)` `(a > b)` `(a = b)` | compare | lexicographic over **UTF-8 bytes** (`as_bytes().cmp`) |

Each produces a new interned string, charged to the heap budget.

---

## `src/interpreter/io.rs`

```rust
pub struct Io<R: BufRead, W: Write, E: Write> { stdin: R, stdout: W, stderr: E }
```

The reader needs peek-without-consume for `eof`, so it holds its own buffered handle.

**Write formats** (byte-exact, none append a newline except `println`):

| Op | Output |
|---|---|
| `PrintInt` | decimal |
| `PrintBool` | `true` / `false` |
| `PrintString` | raw UTF-8 bytes. **A null argument prints nothing and does not abort** |
| `Println` | a single `\n` |

Flush stdout on every write, so interpreter and compiled code agree about what reached
stdout when a program aborts.

**Read semantics** (ABI §3.7):

| Op | Behavior |
|---|---|
| `ReadInt` | skip whitespace; at EOF → **111**; accept optional `+`/`-` then digits; no digits or overflow → **110** |
| `ReadBool` | skip whitespace, take a token to the next whitespace, accept exactly `true`/`false`; anything else, **including EOF**, → **112** |
| `ReadString` | read to the next `\n`, which is consumed and excluded; empty string on immediate EOF |
| `Eof` | `true` iff at end-of-input, **consuming nothing** |

The `eof()` trap is worth a code comment: it is a robust guard for *line* reads
(`read_string` consumes its trailing newline) but **not** for *token* reads — a trailing
newline leaves `eof()` false after the last token, so `while (!eof()) { read_int(); }` reads
once too many and aborts 111. **Reproduce this; do not "fix" it.**

`out` vs `err`: the print target is chosen by comparing the receiver `ObjId` to `err_id`
(→ `stderr`), else `stdout`. References passed as an `Output` formal keep the same handle, so
this stays correct.

---

## `src/interpreter/abort.rs`

```rust
pub enum AbortKind {
    CastFailed { from: String, to: String }, // 101
    NullReceiver { method: String },          // 102
    ReadIntMalformed,                         // 110
    ReadIntEof,                               // 111
    ReadBoolInvalid,                          // 112
    RepeatNegative(i32),                      // 120
    OutOfMemory,                              // 137
}
```

`code()` and `message()` are the contractual pair — the conformance harness substring-matches
stderr and exact-matches the exit code, so the message prefixes are fixed:

| Code | Message |
|---|---|
| 101 | `lo_cast_check: cannot cast <from> to <to>` |
| 102 | `lo_abort_null_receiver: cannot dispatch <method>` |
| 110 | `lo_read_int: malformed token` |
| 111 | `lo_read_int: end of input` |
| 112 | `lo_read_bool: invalid token` |
| 120 | `lo_string_repeat: negative count <n>` |
| 137 | `lo_alloc: out of memory` |

`<from>`/`<to>` are class names taken from `ClassTable` — the same names the runtime reads
from `ClassDescriptor.name`.

---

## `src/main.rs` (add to the existing file)

```rust
mod interpreter;

// CLI wiring (check/interpret/compile) comes later. For reference, interpret mode is:
//   let program = parser::parse_program(&tokens)?;
//   let (typed, table) = type_checker::check_program(program)?;
//   let outcome = interpreter::interpret(&typed, &table, HEAP_DEFAULT, &mut io);
//   // render Outcome to stderr/exit code
```

---

## Acceptance tests

Unit tests, `cargo test` in `compiler/`. Whole-program rows build a `TypedProgram` by running
the real `lexer` + `parser::parse_program` + `type_checker::check_program` on the `.lo` source,
then call `interpret` with in-memory `stdin`/`stdout`; component rows exercise a module
directly. Assert on `Outcome` and captured stdout.

| Area | Case | Expected |
|---|---|---|
| entry | `class Main() { int main() { return 48; } }` | `Exit(48)` |
| int total | `return (7 / 0);` | evaluates to `-1` (no abort) |
| int total | `return (7 % 0);` | evaluates to `7` |
| int total | `INT_MIN / -1` (built via `((~2147483647)+(~1))`) | `INT_MIN`, no panic |
| overflow | `2147483647 + 1` | wraps to `INT_MIN` |
| short-circuit | `(false & rhs)` where `rhs` would abort | `false`, `rhs` never evaluated |
| unary | `(~5)` → `-5`; `(~"ab")` → `"ba"` | value/type per operand |
| string | `("ab" * 3)` → `"ababab"`; `("x" * 0)` → `""` | ok |
| string | `("x" * n)` with `n < 0` | `Abort(RepeatNegative)` (120) |
| string | reverse of a multi-byte-codepoint string | codepoint-reversed, valid UTF-8 |
| string | compare `"Z"` vs `"a"` | `"Z" < "a"` (byte/codepoint order) |
| dispatch | override called through a base-typed variable | runs the override |
| dispatch | `super.m()` where a grandchild overrides `m` | runs the ancestor's `m`, not the override |
| dispatch | call on a null receiver | `Abort(NullReceiver)` (102) |
| ctor | inheriting ctor with `super(...)` | parent fields set before child body runs |
| ctor | field left unassigned by the constructor | keeps its type-default |
| ctor | formal shares a field's name (`A(int x){x=x;}`) | field stays default — reproduce the quirk |
| cast | downcast to the wrong class | `Abort(CastFailed)` (101) |
| cast | `((T) null)` | `null`, no check |
| instanceof | `(null instanceof T)` | `false`, no abort |
| defaults | a `String` field read before assignment | empty string, not null |
| I/O | `out.print_int(48)` | stdout is `48` |
| I/O | `out` vs `err` routing | bytes land on the right stream |
| I/O | `read_int` at EOF | `Abort(ReadIntEof)` (111) |
| I/O | `read_bool` on a non-`true`/`false` token | `Abort(ReadBoolInvalid)` (112) |
| I/O | `read_string` then another at EOF | second returns `""` |
| I/O | `eof` line-read loop vs token-read loop | line loop clean; token loop aborts 111 |
| heap | allocate past the byte budget | `Abort(OutOfMemory)` (137), heap unchanged on the failing alloc |

---

## Explicitly out of scope for this step

- **The CLI / driver.** `main.rs` gains only `mod interpreter;`; wiring `check`/`interpret`/
  `compile` modes, file reading, and the process exit is a separate plan. `interpret` returns
  `Outcome`; it never calls `process::exit`.
- **The WASM back end** and any codegen-specific concern (vtable slots, shadow stack, write
  barriers). The interpreter shares no code with it; parity is observable-behavior only.
- **Interpreter-vs-compiler differential testing** — impossible until the back end exists.
- **An end-to-end conformance runner** over the `lo-testing` corpus — deferred.
- **Garbage collection.** The interpreter models dynamic semantics, not memory management;
  the byte budget stands in for the heap. Only revisit if a valid program trips it
  (`interpreter_design.md` §3).
