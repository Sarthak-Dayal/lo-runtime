# Interpreter Design — LO (LiveOak) P1

Scope: a checked (typed) AST in, a process exit status out, with observable behavior
identical to compiled code.

---

## Decisions

| Decision | Why |
|---|---|
| Consume the **checked** AST; assume well-formed | The type checker has already rejected every ill-typed program. The interpreter never re-checks and emits no diagnostics. States the checker made impossible become `unreachable!()` with an internal-error message, not user-facing errors. |
| Objects and strings live in an **interpreter-owned arena**, addressed by `ObjId(u32)` handles | Required for OOM parity (exit 137, §9). Also sidesteps `Rc` cycles, which the OOM conformance test constructs directly (a `Node` list) and which LO programs can build freely. |
| **Byte budget** on the arena, default 64 MiB, `--heap-limit` to override | `lo_alloc` aborts 137 on exhaustion. Native `Rc`/`Box` allocation would instead consume all system RAM and be killed by the OS. A budget makes the abort deterministic and fast. |
| **No garbage collection** | The interpreter models LO's *dynamic semantics*, not its memory management. See the fidelity caveat in §4. |
| Interpreter **consumes the checker's `ClassTable`** directly | The finalized checker returns a `ClassTable` carrying `effective_fields`, `effective_methods`, and `is_subtype` — exactly what dispatch and layout need. The evaluator uses it read-only; the earlier bridge table is gone. See §5. |
| Runtime values are **self-describing**; operator dispatch reads the value, not a static type | LO's overloaded operators (`+ * ~ < > =`) resolve on the operand values, which already carry their kind. The checker has pre-resolved everything that needs a static type (dispatch via `MethodResolution`, cast direction via `CastDirection`), so the evaluator does not depend on any type annotation being correct. |
| Aborts are a **control-flow signal**, not a direct `process::exit` | Lets unit tests observe an abort without spawning a subprocess. The driver is the only place that exits. |
| One `io` module, one `abort` helper | The interpreter is the only I/O implementation the team writes (codegen emits call sites; the runtime implements the behavior). Confining it gives one place to fix a fidelity mismatch. |

---

## 1. Input contract

```
interpret(program: &TypedProgram, table: &ClassTable, heap_limit: usize,
          io: &mut Io) -> Outcome
```

Both inputs come from `type_checker::check_program`, which the interpreter runs only after
it returns `Ok`. It assumes:

- Every name is resolved — every `Var`/`Assign`/receiver carries a `BindingInfo`, and every
  call a `MethodResolution`. No unknown class, method, field, local, or formal remains.
- Every value-bearing expression carries its static type inline (`Binary.ty`, `Cast.target`,
  `Call.return_type`, `Var.binding.ty()`, …); `Null` and a both-branches-null `Ternary` carry
  none, and never need one at runtime.
- Every method's declared return type matches every `return`; arities and argument types are
  pre-validated.
- Constructor delegation is present and first where LO-4 requires it.
- `Main` exists, declares `int main()` directly, and has a zero-arg constructor.

It does **not** assume anything about runtime state: null receivers, failed casts, bad
stdin, and heap exhaustion are all reachable in a well-typed program and are handled (§9).

---

## 2. Values

```rust
enum Value {
    Int(i32),
    Bool(bool),
    Str(StrId),            // never null — see below
    Obj(Option<ObjId>),    // None == LO null
}
```

`Str` is not optional. LO's `String` is primitive-equivalent in the type system: its
field type-default is the empty string, not null, and `null` is assignment-compatible only
with *class*-typed targets (Ref. Ch.4 §4.2). A `String` local therefore never holds null,
and the interpreter should not represent a state the language cannot reach.

`Obj(None)` is the only null. It is produced by the class type-default, by the `null`
literal, and by a null-returning cast.

**Type-defaults**, applied to fields at allocation and to locals at frame setup:

| Type | Default |
|---|---|
| `int` | `Int(0)` |
| `bool` | `Bool(false)` |
| `String` | `Str(EMPTY)` — the interned empty string, mirroring `LO_EMPTY_STRING` |
| class `C` | `Obj(None)` |

The `String` case is the one that catches people: it is *not* zero. The ABI makes codegen
store `LO_EMPTY_STRING` explicitly after `lo_alloc` for exactly this reason (§3.1), and the
interpreter has to match.

---

## 3. The heap

One arena, one handle space, one budget:

```rust
enum Cell {
    Obj { class: ClassId, fields: Vec<Value> },
    Str(Rc<str>),          // immutable; LO has no string mutation
}

struct Heap {
    cells: Vec<Cell>,
    charged: usize,        // bytes charged against the budget
    limit: usize,
}
```

Strings share the object arena rather than living separately, because string allocation
must count against the same budget — `lo_string_repeat` and `lo_string_concat` can exhaust
the heap just as `new` can.

**Charging model**, mirroring the ABI's wasm32 layout so the budget corresponds to
something real:

| Allocation | Charged |
|---|---|
| object of class `C` | `12 + 4 × field_count(C)` — header (§2, 12 bytes on wasm32) plus one word per field |
| string of `n` bytes | `12 + 4 + n` — header, `length: u32`, inline UTF-8 tail |

Exceeding `limit` raises `Abort::OutOfMemory` (§9). The check happens *before* the cell is
pushed, so a failed allocation leaves the heap unchanged.

> **Fidelity caveat — no GC.** Compiled code collects; the interpreter does not. A valid
> program that allocates heavily but drops its references will OOM in the interpreter and
> not in compiled code. This does **not** affect the OOM conformance test, which allocates
> an unbounded *reachable* list — genuinely uncollectable, so both back ends abort. The
> mitigation is to set the budget well above what any conformance program needs. If a
> `ValidPrograms` test ever trips it, the fix is a mark-sweep over the arena rooted at the
> frame stack, which the handle-based design already permits.

---

## 4. Strings

All five operations follow ABI §3.2 exactly. Note which unit each one works in:

| LO | Operation | Semantics |
|---|---|---|
| `(a + b)` | concat | byte concatenation |
| `(s * n)` | repeat | `n` copies. **`n < 0` aborts 120**; `n == 0` yields the empty string |
| `(~s)` | reverse | **by codepoint**, not by byte. Walk forward to find boundaries, then emit in reverse |
| `(a < b)` `(a > b)` `(a = b)` | compare | lexicographic over **UTF-8 bytes**, sign of the comparison |

Byte order for comparison and codepoint order for reversal are different units on purpose;
getting one of them wrong produces a test failure only on non-ASCII input, which the public
corpus may not cover. Rust's `str` gives both directly: `as_bytes().cmp(..)` for compare,
`chars().rev()` for reverse.

---

## 5. Class table

The interpreter **does not build its own** — it consumes the `ClassTable` that
`check_program` returns (`type_checker.rs`). Per class it exposes:

- `effective_fields: Vec<FieldInfo>` — **parent-first**, inheritance/overrides resolved.
- `effective_methods: HashMap<String, MethodEntry>` — the effective set (own plus
  inherited-and-not-overridden), one lookup and no parent-chain walk; each `MethodEntry`
  names the declaring `owner`.
- `is_subtype(a, b)`, `parent`, `ancestors`, `kind`.

Four properties make the rest simple, and they hold because LO forbids field shadowing and
method overloading and permits constructor overloading by arity only:

- **Fields are parent-first**, so an inherited field has the same index in every descendant —
  the ABI's layout rule (§2, Ref. Ch.4 §3.3), which keeps the interpreter's field indices and
  codegen's byte offsets cross-checkable.
- **`effective_methods` is flattened**, so virtual dispatch is one map lookup on the *runtime*
  class.
- **Constructors are selected by arity alone** (scan a class's `constructors`).
- **The hierarchy is a forest** — `parent: None` at a root, no implicit `Object`; subtype
  tests walk up and terminate.

The preamble `Input`/`Output` appear as ordinary `Preamble`-kind classes, so they need no
special case here.

**One index the interpreter builds itself.** `effective_methods` gives a signature and a
declaring `owner`, not a body. Build, once at startup from `TypedProgram`, a map
`(owner_class, method_name) -> &TypedMethodDecl` (plus a class-by-name map). A `Virtual`
dispatch resolves the override winner via `effective_methods[m].owner`, then indexes this map;
a `Super` call indexes it directly at `(declaring_class, m)`.

---

## 6. Frames and name resolution

A call frame holds the receiver and the method's locals and formals:

```rust
struct Frame { this: Option<ObjId>, slots: Vec<Value> }
```

LO makes this simpler than it looks. Block-local declarations are **hoisted** to the
enclosing method or constructor (Ref. Ch.3 §3.3), locals may not shadow formals, and fields
may not be shadowed — so the set of names visible in a body is flat and knowable before
execution, with no scope stack to push and pop.

Resolution order for a bare identifier is locals → formals → fields of the enclosing class,
first match wins (Ref. Ch.3 §3.3). The three preamble bindings `in`, `out`, `err` sit in a
scope enclosing every body and are checked last.

The evaluator touches variables through exactly two operations:

```rust
fn read_var (&self, f: &Frame, v: &VarRef) -> Value;
fn write_var(&mut self, f: &mut Frame, v: &VarRef, val: Value);
```

`VarRef` is the checker's `BindingInfo`, attached to every `TypedExpr::Var`,
`TypedStmt::Assign`, and `TypedReceiver::Var`:

- `Local` / `Formal` — a slot in `frame.slots`, indexed through a per-method name→slot map
  built once from `params ++ locals`.
- `Field { .. }` — an index into `this`'s runtime-class `effective_fields`, by name (unique
  because shadowing is forbidden).
- `Prebound` — one of the `in` / `out` / `err` singletons.

All three fit behind `read_var`/`write_var`, so the evaluator is written once. The per-method
slot map is worth building regardless — it is the same artifact codegen needs for
shadow-stack frame layout.

---

## 7. Evaluation

### Control flow

```rust
enum Signal { Break, Return(Value), Abort(AbortKind) }
type Exec<T> = Result<T, Signal>;
```

`Break` unwinds to the nearest enclosing `while`. `Return` unwinds to the nearest call
boundary. `Abort` unwinds all the way to the driver, which is the only place that exits the
process. Using one signal type for all three keeps `?` working throughout the evaluator.

### Operators

Dispatch reads the operand values. `Value::Int` versus `Value::Str` selects integer
arithmetic or the string operation, which is exactly the information the runtime has at the
corresponding call site.

**Integer semantics are total** (Ref. Ch.2 §3.1) and must not use Rust's default operators,
which panic in debug and differ from LO in release:

| LO | Rust |
|---|---|
| `+ - * ~` | `wrapping_add` / `wrapping_sub` / `wrapping_mul` / `wrapping_neg` |
| `x / 0` | `-1` |
| `x % 0` | `x` |
| `INT_MIN / -1` | `INT_MIN` (`wrapping_div`) |
| `INT_MIN % -1` | `0` (`wrapping_rem`) |
| otherwise | `wrapping_div` / `wrapping_rem` |

`&` and `|` **short-circuit** — evaluate the right operand only if the left does not decide
the result. They are single-character tokens in LO; there is no non-short-circuit form.

`~` is integer negation on `Int` and string reversal on `Str`. `!` is Boolean NOT.

### Casts and `instanceof`

Both take the *runtime* class of the operand, per ABI §3.5. The checker tags each cast with a
`CastDirection`: `Upcast` and a null-source `Null` cast pass through unchecked, and only
`Downcast` runs the check below:

- `((T) e)` — null returns null without a check. Otherwise, if the runtime class is `T` or
  a descendant, return the value; else `Abort::CastFailed`, which needs both class names.
- `(e instanceof T)` — null yields `false`. Never aborts.

Unrelated-type casts are a *compile* error (`E_CAST_UNRELATED_TYPES`), so the interpreter
never sees one.

Steered by the call's `MethodResolution` (`Virtual` / `Super` / `Io`):

- **`r.m(...)`** (`Virtual`) — evaluate `r`. If null, `Abort::NullReceiver` carrying `m`'s
  name. Else look up `m` in the *runtime* class's effective method table.
- **`super.m(...)`** (`Super { declaring_class }`) — statically dispatched. The checker has
  already pinned the target to the ancestor that declares it, bypassing any override below
  it (Ref. Ch.4 §4.1); this is the one call form that does not consult the runtime class, and
  no lookup is needed.
- **`out.print_*` / `in.read_*`** (`Io { op }`) — no frame; match on the op (§8).
- **`new C(...)`** — allocate with all fields at their type-defaults, then run the
  constructor selected by arity.

### Constructor order

1. Allocate; all fields (including inherited) take their type-defaults.
2. Run the delegation prefix — `super(...)` or `this(...)` — to completion first. A
   `this(...)` chain eventually reaches a `super(...)`, so exactly one constructor body runs
   per hierarchy level, parent portion first.
3. Run the body.

At the point a `super(...)` returns, the parent's fields are initialized and the current
class's own fields are still at their type-defaults (Ref. Ch.4 §5). A constructor is not
obliged to assign every field.

---

## 8. I/O

The preamble is injected as ordinary classes, so `out.print_string(s)` is an ordinary call
and needs no dedicated AST node. The checker tags these eight methods as
`MethodResolution::Io { op }` (bodies `TypedMethodBody::Io(op)`); when a call resolves to
`Io`, the interpreter matches on the op instead of pushing a frame.

**The `Output` stream is a runtime property.** `out` and `err` are both `Output`, and the
reference explicitly permits passing either as an `Output` formal (Ch.3 §4.6 —
`void log(Output sink, String msg)`). The stream therefore cannot be resolved from the
receiver's *name*. The finalized preamble `Output` has **no fields**, so there is no place on
the object to store an `fd`. Instead the interpreter allocates the three singletons once at
startup, records their `ObjId`s, and a `print_*` op picks the stream by comparing the
receiver's `ObjId` against the stored `err` id (→ stderr), else stdout. A reference passed as
an `Output` formal is the same handle, so this stays correct. `Input` needs no such tag.

**Write formats** (verified against `lo-runtime/rust/src/io.rs`) — all byte-exact, none
append a newline:

| Op | Output |
|---|---|
| `print_int` | decimal |
| `print_bool` | `true` / `false` |
| `print_string` | raw UTF-8 bytes. **A null argument prints nothing and does not abort** |
| `println` | a single `\n` |

Flush stdout on every write, as the runtime does — otherwise the interpreter and compiled
code disagree about what reached stdout when a program aborts.

**Read semantics** (ABI §3.7):

| Op | Behavior |
|---|---|
| `read_int` | Skip whitespace. At EOF → **111**. Accept optional `+`/`-`, then digits. No digits → **110**. Parse overflow → **110** |
| `read_bool` | Skip whitespace, take bytes to the next whitespace, accept exactly `true`/`false`. Anything else → **112**, and that **includes EOF** — there is no separate EOF code here |
| `read_string` | Read to the next `\n`; the newline is consumed and excluded. Empty string on immediate EOF |
| `eof` | True iff at end-of-input, **consuming nothing** |

The reader needs peek-without-consume, so it holds its own buffered handle rather than
reading line-at-a-time.

> The `eof()` trap is worth a comment in the code: it is a robust guard for *line* reads,
> because `read_string` consumes its trailing newline, and **not** for *token* reads,
> because a trailing newline leaves `eof()` false after the last token — so
> `while (!eof()) { read_int(); }` reads once too many and aborts 111. Reproduce this;
> do not "fix" it.

---

## 9. Aborts

One `AbortKind`, seven producers. `interpret` **returns** `Outcome::Abort(kind)` rather than
exiting; `kind.code()` and `kind.message()` are the contractual pair the future driver writes
to stderr and exits with. Returning rather than exiting keeps aborts unit-testable.

| Code | `AbortKind` | Message |
|---|---|---|
| 101 | `CastFailed { from, to }` | `lo_cast_check: cannot cast <from> to <to>` |
| 102 | `NullReceiver { method }` | `lo_abort_null_receiver: cannot dispatch <method>` |
| 110 | `ReadIntMalformed` | `lo_read_int: malformed token` |
| 111 | `ReadIntEof` | `lo_read_int: end of input` |
| 112 | `ReadBoolInvalid` | `lo_read_bool: invalid token` |
| 120 | `RepeatNegative(n)` | `lo_string_repeat: negative count <n>` |
| 137 | `OutOfMemory` | `lo_alloc: out of memory` |

The conformance harness substring-matches stderr and exact-matches the exit code, so the
message prefixes are contractual. `<from>` and `<to>` are class names read from the class
table — the same names the runtime reads from `ClassDescriptor.name`.

---

## 10. Entry and the (out-of-scope) driver

The library entry `interpret` runs the program itself, mirroring `lo_entry` (ABI §3.6):
construct `Main` via its zero-arg constructor, then call `main()`. There is no separate init
step — the interpreter has no heap to initialize beyond its arena. It returns:

- `Outcome::Exit(rv)` — a clean finish; `rv` is `Main.main()`'s return value.
- `Outcome::Abort(kind)` — a runtime abort (§9).

Wiring this into a CLI (`read file -> lex -> parse -> check -> interpret -> render/exit`, with
`interpret` mode behaving exactly as `check` mode does on an invalid program) is a **separate
plan**. The driver is where `Outcome` becomes the process exit status and where an abort's
message is written to stderr.

---

## 11. Open items

Resolved by the finalized type checker: the typed-AST shape (`TypedProgram`, carrying
per-node types rather than a single `Expr.ty`), whether `Var` is resolved (`BindingInfo`, §6),
and class-table ownership (consume `ClassTable`, §5). Still open:

| Item | Blocked on | Note |
|---|---|---|
| `err` lowering | Instructor | The ABI has **no stderr print entry point** — `lo_print_*` is stdout-only, and `host.write_stderr` is documented as the runtime's abort path, not a codegen target. The interpreter can implement `err` correctly for free; **codegen cannot**. No public test uses `err.`, and none passes `Output` as a formal. Raise it; record the answer in the design note. |
| Heap budget default | Measurement | 64 MiB is a guess. Set it after measuring the largest `ValidPrograms` allocation. |
| GC in the interpreter | Deferred | Not needed unless a valid program trips the budget (§3). |
| `check`-mode failure exit code | Team (driver) | Specified only as "nonzero". A driver concern (out of scope here); pick one, document it in the README, use it in all modes. |

---

## Alternate designs considered, not chosen

| Option | Rejected because |
|---|---|
| `Rc<RefCell<Obj>>` for objects instead of an arena | LO programs build cyclic structures freely — the OOM conformance test constructs a linked list directly — and `Rc` leaks every cycle, so the byte budget would never fall and OOM parity would be accidental rather than modeled. Handles also make a future mark-sweep possible without redesign. |
| Link the native `liblo_runtime.a` and call the real `lo_print_*` / `lo_read_*` via FFI | Tempting, and it works for the six scalar ops. But `lo_print_string` takes and `lo_read_string` returns `*mut Object` in the runtime's GC heap, so using them means building real `StringObject`s at the ABI's layout — dragging the object model, the shadow stack, and `lo_runtime_init` into an interpreter whose whole advantage is not having them. It would also split I/O across two mechanisms. |
| Dispatch operators on the static type rather than on the runtime value | Would make the evaluator's correctness depend on the checker's annotations being right, coupling two components that can otherwise be tested independently. The values already carry their kind, and the checker has pre-resolved the cases that genuinely need a static type (`super` dispatch via `MethodResolution`, cast direction via `CastDirection`). |
| Re-check types while interpreting, for debuggability | Doubles the type checker and slows the evaluator to defend against a state the checker has already excluded. Internal-error panics catch the same bugs at the same place, and only in development. |
| Walk the parent chain on every method call instead of flattening the effective method set | One map lookup versus a loop per call, for no benefit. Flattening also makes the interpreter's dispatch table directly comparable to codegen's vtable, which is a cheap cross-check between the two back ends. |
| Exit the process at the abort site | Untestable without spawning subprocesses. Routing aborts through `Signal` lets unit tests assert on abort kind and message directly. |
