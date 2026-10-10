# How AST-to-IR lowering works

Lowering converts a type-checked LO program into functions, control-flow blocks,
static data, and explicit runtime calls. It removes structured syntax such as
`if`, `while`, and `new`, while preserving evaluation order and language behavior.
The result uses typed, mutable virtual registers; physical registers, shadow-stack
frames, and machine instructions are assigned by later passes.

The entry point is:

```rust
lower_program(
    program: &TypedProgram,
    classes: &ClassTable,
    target: TargetLayout,
) -> Result<CheckedIr, String>
```

`TypedProgram` supplies bodies and resolved bindings. `ClassTable` supplies
inheritance, effective fields, constructor signatures, and stable method slots.
`TargetLayout` selects WASM32 or x86-64. Lowering relies on the type checker's
results rather than performing name resolution or inheritance checking again.

The existing compiler CLI still emits WASM directly from the typed AST. This IR
entry point is currently exercised through tests. `CheckedIr` means the generated
program passed structural, type, and definite-assignment checks; ordinary
safepoint rooting is a separate transformation.

## Where the implementation lives

| File | Responsibility |
|---|---|
| [lower.rs](../compiler/src/ir/lower.rs) | Public API, function construction, statements, expressions, control flow, constructors, and startup |
| [lower/program.rs](../compiler/src/ir/lower/program.rs) | Program context, declarations, class layouts, signatures, static bytes, and metadata |
| [lower/layout.rs](../compiler/src/ir/lower/layout.rs) | Target widths, ABI offsets, alignment, and checked offset conversion |
| [lower/runtime.rs](../compiler/src/ir/lower/runtime.rs) | Binding access, field stores, calls, allocation, I/O wrappers, and String operations |
| [mod.rs](../compiler/src/ir/mod.rs) | IR types, instructions, terminators, symbols, and data definitions |
| [verify.rs](../compiler/src/ir/verify.rs) | Validation before constructing `CheckedIr` |

## From a program to checked IR

The program context owns the `ProgramIr` being assembled, references the checked
class table, and maintains lookup tables for symbols, layouts, and interned bytes.
Program lowering proceeds in five stages:

1. Register runtime functions, `LO_EMPTY_STRING`, the static binding frame, startup, class
   metadata symbols, constructors, and methods. Compute class layouts.
2. Emit descriptors, reference-field offset arrays, and vtables for each class.
3. Lower every constructor and method into a `FunctionIr`.
4. Generate the startup function, `lo_entry`.
5. Verify the assembled `ProgramIr` and return `CheckedIr`.

Declarations precede all bodies and metadata relocations, so recursion and
references to classes declared later are supported. Layout computation visits a
parent recursively before laying out a child.

Function signatures are interned: equal parameter/result type sequences share a
`SignatureId`. The existing length-prefixed class and method symbol names avoid
identifier collisions. Static byte buffers are interned by contents, so repeated
literals share their source bytes. Nonempty String literal evaluation still calls
`lo_string_new`; byte interning does not cache the resulting managed object.

## Types, storage, and function setup

Source types map to IR value types as follows:

| Source value | IR type |
|---|---|
| `int` | `Int32` |
| `bool` | `Bool` |
| String or class reference | `Ref` |
| Descriptor, raw byte address, or root-slot address | `Ptr` |
| Loaded method target | `CodePtr(SignatureId)` |

Void functions have no result operand; `void` is not a register type. String and
class identity are erased from register types. Lowering consults the original
expression type when it needs to distinguish String operations from scalar or
reference operations.

A source method or constructor receives `this` in register 0. Formal parameters
follow in declaration order and already contain their incoming values. Locals
receive named registers and explicit entry-block initialization:

| Local type | Initial operand |
|---|---|
| `int` | `0` |
| `bool` | `false` |
| Class | `null` |
| String | `@LO_EMPTY_STRING` |

Registers are mutable storage locations. Reassigning a local writes to the same
register rather than creating an SSA version. Reading a local or formal copies
its current value into a fresh temporary, preserving the value observed before
later expressions execute. For example, `x = (x + 1)` conceptually produces:

```text
t0:Int32 = x
t1:Int32 = t0 + 1
x:Int32 = t1
```

Virtual registers are allocated through `FunctionIr::new_register`, which keeps
the register type and optional name tables aligned. Instruction source lines are
retained for diagnostics.

## Building the control-flow graph

The function builder holds blocks under construction and an optional `current`
block. `emit` appends to that block. `finish` installs its terminator and clears
`current`. A missing current block means the path has ended.

Every finished block has exactly one terminator: a jump, conditional branch,
return, or nonreturning abort. While construction is in progress, a block may
have no terminator. Finalization rejects any such block still left open.

Expression lowering returns an `Operand` and may emit both instructions and
blocks. Its continuation is whatever block is current when it returns. Callers
must use that continuation rather than assuming the expression stayed in its
starting block. This allows ternaries, guarded division, and calls with null
checks to nest inside other expressions.

### Statements

Assignment evaluates the RHS and then writes the resolved binding. Return
evaluates its value and ends the current path. Statements after a terminated path
in the same list are skipped. Empty statements emit nothing.

An `if` evaluates its condition, branches into two arms, and saves each arm's
final open endpoint. If either arm can continue, those endpoints jump to a new
join block. If both arms end, no join is created.

A `while` creates a test region, body, and exit. The preceding block jumps to the
test; a true condition enters the body and a false condition reaches the exit.
An open body endpoint jumps back to the test. A stack of loop exits makes `break`
target the innermost loop. Constant conditions are not simplified here, so even
`while (true)` has a structurally present exit.

An open endpoint at method completion becomes a void return. Non-void
fallthrough is an error. These are structural checks, not a general proof of
constant-condition reachability.

### Conditional expressions and short-circuiting

`choose` implements a ternary by assigning both arms into one result register:

```text
              branch condition
               /            \
       result = yes      result = no
               \            /
                    join
```

Exactly one arm executes at runtime. Both arms are lowered at compile time.
Because the result register is mutable, the join needs no phi instruction.
Definite-assignment verification checks that the value is available on incoming
paths.

Boolean AND becomes `lhs ? rhs : false`; OR becomes `lhs ? true : rhs`.
This reuses the same CFG construction and preserves short-circuiting. A ternary
with two null branches uses a `Ref` result despite having no concrete source type.

### Scalar arithmetic and division

Ordinary binary operands are evaluated left to right before the operation is
emitted. Scalar arithmetic is represented with Binary or Unary instructions;
general constant folding is not performed.

Division and remainder encode LO's total arithmetic rules explicitly:

| Divisor | Division result | Remainder result |
|---|---|---|
| `0` | `-1` | Dividend |
| `-1` | Wrapping negation of dividend | `0` |
| Other | Ordinary signed division | Ordinary signed remainder |

An immediate divisor selects the appropriate case during lowering. A register
divisor gets branches checking zero and negative one before the ordinary
operation. Every case writes the same destination and reaches a join. Both
operands have already been evaluated, even when a special case determines the
result. A computed constant, such as unary negation of `1`, is still a register
operand and receives the runtime guards.

## Object layout and static metadata

The target layout follows the [runtime ABI](../runtime-abi.md). Memory offsets
are concrete during lowering; address relocations remain symbolic until emission.
The later backend must use the same target selection.

| Property | WASM32 | x86-64 |
|---|---:|---:|
| Pointer width/alignment | 4 | 8 |
| Object header size | 12 | 16 |
| Every field slot width/alignment | 4 | 8 |
| Int/Bool access width | 4 | 4 |
| Reference field width/alignment | 4 | 8 |
| Class descriptor size | 32 | 56 |
| Descriptor vtable offset | 28 | 48 |
| Output stream-selector offset | 12 | 16 |

Int/Bool accesses occupy four bytes, even when their field slot is eight bytes.
The unused half of an x86-64 scalar slot remains padding, zero-filled by lo_alloc.
Encoding Boolean runtime arguments and results remains a backend ABI responsibility.

Every effective field occupies one pointer-sized slot, in ClassInfo.effective_fields
order. Inherited fields form the prefix, so offset(i) = header + ptr * i on both
targets. The instance size is header + ptr * (field_count + hidden_slots), where
hidden_slots is one for Output and zero otherwise. Output's hidden stream selector
is at offset(field_count), which is the header because Output has no declared
fields. There is no per-type packing or parent-tail-padding rule.

Slot width does not determine access width: Store.ty and the Load destination type
still select four bytes for Int32/Bool and pointer width for Ref/Ptr/CodePtr.
This matches [the shared class layout](../compiler/src/layout.rs).

The descriptor contains the name address and byte length, parent descriptor,
instance size, reference-offset array and count, vtable size, and vtable address.
Its field offsets are `0/4/8/12/16/20/24/28` on WASM32 and
`0/8/16/24/32/40/44/48` on x86-64. Padding is emitted explicitly to match C layout.
Class-name byte buffers are null-terminated; their recorded lengths exclude the
terminator.

Reference-offset arrays include String and class fields, including inherited
ones. Vtables use the checked stable slot order and the effective declaring
method for each entry. An override replaces its inherited slot; new methods
append slots. Empty arrays remain addressable through a sentinel allocation while
the descriptor's element count remains zero.

Descriptors, tables, and literal bytes are read-only; the static binding frame is
writable. `DataItem::Addr` expresses relocations, `U32` fixed-width values,
`Bytes` byte buffers, and `Zero` padding or initial storage. Checked arithmetic
rejects layout overflow and values outside the signed offset/length range used
by this IR.

## Binding reads and writes

`BindingInfo` determines how a source name is accessed:

| Binding | Read | Write |
|---|---|---|
| Local/formal | Snapshot its register | Copy into its register |
| Field | Load from `this` at its checked offset | Scalar Store or reference barrier |
| Prebound | Load its root directly from the static frame | Store into that static root |

Field offsets are looked up using the checked owner and field name. Assignment
lowers its RHS before emitting the target access, so any calls in the RHS precede
the field write.

A managed reference field write calls
`lo_gc_write_barrier(receiver, offset, value)`. The helper performs the store;
lowering does not emit another Store afterward. Null writes also use the barrier.
Direct static references such as `LO_EMPTY_STRING` retain the verifier's ordinary
Store exemption because the referenced object cannot move.

### The static shadow frame for prebound bindings

`in`, `out`, and `err` are the three roots of one writable Data symbol,
`lo_bindings`. The symbol contains a complete ABI ShadowFrame, registered with
`lo_push_frame`. Its address is fixed; the collector updates the reference values
stored in its roots when the corresponding objects move.

| Frame component | WASM32 offset | x86-64 offset |
|---|---:|---:|
| Parent pointer | 0 | 0 |
| num_roots = 3 (U32) | 4 | 8 |
| in root | 8 | 16 |
| out root | 12 | 24 |
| err root | 16 | 32 |
| Total size | 20 | 40 |

The frame is pointer-aligned. Static data initializes its parent to zero and its
count to three; padding up to the roots offset and all three roots are zero.
The offsets come from the shared Target::frame() layout. Registering the frame
links it into the runtime shadow stack; it remains registered throughout Main.

A binding read is one Load from Symbol(lo_bindings), at that binding's root offset.
Assignment is one ordinary Ref Store into the same symbolic base. For x86-64:

```text
object:Ref = load [@lo_bindings + 24]          # read out
store Ref [@lo_bindings + 24], anotherOutput  # assign out
```

The verifier permits reference stores with a Data-symbol base only when the
symbol has a matching writable DataDef, identifying writable static storage
rather than a heap object. External Data symbols with unknown sections do not
receive this exemption. Ptr-register bases still require barriers, even when
copied from a writable Data symbol. The direct StaticRef-value exemption remains.

Write permission is checked independently: every Store with a symbolic
read-only destination is rejected, including scalar values and StaticRef values.
The barrier exemptions cannot bypass this check. Section checks do not track
addresses indirectly through registers.

There are no globals containing root-slot addresses, indirect root-store
instructions, or exposed addresses of roots in startup's local frame. The
collector walks the registered static frame's roots directly.

Copying out into a local snapshots its current object reference. Reassigning out
leaves the copy referring to the earlier object. The later GC pass roots ordinary
local copies and passed references separately from the static bindings.

## Calls and dispatch

`method_call` evaluates the receiver once, evaluates actual arguments once from
left to right, and then emits the receiver null check. This order matters: an
argument's side effects execute before a null-dispatch abort.

The guard compares the receiver with null and branches to either a continuation
or an Abort terminator calling `lo_abort_null_receiver`. Static method-name bytes
and their byte length provide the runtime diagnostic. An abort has no successor.

For virtual dispatch, the static class supplies a stable slot and compatible
signature; the runtime object supplies the actual method implementation:

```text
receiver:Ref = evaluate receiver
arguments   = evaluate actuals left to right
branch on receiver == null

non-null path:
    descriptor:Ptr = load [receiver + 0]
    table:Ptr = load [descriptor + target.vtable_offset]
    target:CodePtr(signature) = load [table + slot * pointer_width]
    result = call_indirect target(receiver, arguments...)
```

Loading the CodePtr with an interned signature lets verification check argument
and result types. Layout correctness and vtable contents remain trusted lowering
responsibilities.

A `super` call directly targets the checked declaring ancestor, bypassing virtual
overrides. Built-in I/O calls directly target synthetic Input/Output wrappers.
Both still receive the receiver first and use the null guard. Constructor calls
are direct calls with an already allocated receiver.

`invoke` allocates a destination only when the signature has a result. Expression
calls require that result; statement calls have no destination for their void
signature. These are semantic Call instructions: root publication and reloads
are not inserted here.

## Strings and runtime type operations

Empty Strings are represented by the address of the external static object
`LO_EMPTY_STRING`. Nonempty literals call `lo_string_new(bytes, byte_length)`;
lengths are UTF-8 byte counts, not character counts.

| Source operation | Lowering |
|---|---|
| String `+` | `lo_string_concat` |
| String `*` integer | `lo_string_repeat` |
| String reversal | `lo_string_reverse` |
| String `=`, `<`, `>` | `lo_string_compare`, then compare its Int32 result with zero |

String equality therefore compares contents. Negative repetition and other
runtime failures are handled by their ABI helpers. Scalar and class-reference
operations use their existing IR instructions.

Casts evaluate their operand once. Checked upcasts and literal-null casts return
the operand unchanged. Downcasts call `lo_cast_check(object, descriptor)` and use
its Ref result. `instanceof` calls `lo_instanceof(object, descriptor)` and uses
its Bool result. The runtime makes null downcasts succeed and null instanceof
return false.

## Allocation and constructors

A new expression first evaluates its actuals, then allocates and constructs:

```text
arguments = evaluate actuals left to right
object:Ref = call @lo_alloc(@class_descriptor)
store @LO_EMPTY_STRING into every effective String field
call @selected_constructor(object, arguments...)
result = object
```

`lo_alloc` supplies zeroed integer, Boolean, and class-reference defaults. String
fields require the static empty-string default, including fields inherited from
ancestors. Those stores occur before constructor execution.

Constructors take an existing receiver and return void. An explicit constructor
lowers its checked `this(...)` or `super(...)` delegation before its statement
body. The selected target is identified by class and arity. An implicit
constructor stores the typed formal-to-field mapping, using barriers for managed
references. Allocation returns its original receiver after the constructor call;
the later GC pass must preserve that live reference across the call.

## I/O wrappers and startup

Synthetic I/O method bodies use the same function builder and registered source
signatures. Their receiver is not forwarded to the runtime helpers.

| Source method | Runtime helper |
|---|---|
| `read_int` | `lo_read_int` |
| `read_bool` | `lo_read_bool` |
| `read_string` | `lo_read_string` |
| `eof` | `lo_eof` |
| `print_int` | `lo_print_int` |
| `print_bool` | `lo_print_bool` |
| `print_string` | `lo_print_string` |
| `println` | `lo_println` |

Input wrappers return the runtime result. Output wrappers load the private
selector from their receiver and append it to runtime arguments. Thus the stream
belongs to the Output object and survives aliasing, parameter passing, and
prebound reassignment. A wrapper ends with a value or void Return.

`lo_entry() -> Int32` is marked as `ProgramIr.startup`. It:

1. Calls `lo_runtime_init` before any other runtime operation.
2. Calls lo_push_frame with Symbol(lo_bindings), registering the static frame.
3. Allocates and constructs Input and Output objects. Sets Output selectors to
   zero for stdout and one for stderr, and Stores each object into its static root.
4. Allocates and constructs Main, dispatches main, saves its Int32 result, calls
   lo_pop_frame, and returns the result.

## Verification and the GC handoff

Finalization requires terminated blocks and correct fallthrough behavior.
Program verification checks symbols, signatures, parameter and result types,
branch targets, instruction operands, local root-slot indices, and definite
assignment on reachable incoming paths. Symbolic loads trust lowering's field,
descriptor, and static-frame layouts.

All functions, including startup, currently have zero local root slots. The three
persistent roots live in lo_bindings, outside every function's local shadow frame.
RootAddr and RootStoreIndirect have been removed. RootStore and RootLoad remain
available for local root operations introduced by the later GC pass.

A later pass consuming CheckedIr supplies liveness, temporary roots, reference
publication/reloads at safepoints, and physical local-frame construction. It must:

- Preserve the explicit static-frame registration and pop emitted by startup.
- Keep local frame pushes and pops balanced around the registered static frame.
  Any startup local frame must be registered after runtime initialization; all
  additional frames must be unwound in shadow-stack order before normal return.
- Initialize local frame slots to null and preserve live receivers, arguments,
  intermediate values, allocation results, and returns across safepoints.
- Reload local reference copies independently of static binding reassignment.

The static frame solves persistent binding storage; CheckedIr alone does not
establish moving-collector safety for the other live references or supply their
physical frame cleanup. The explicit lo_push_frame/lo_pop_frame pair registers
and unregisters only lo_bindings.

## Behavioral coverage and known differences

[Scalar tests](../compiler/src/ir/lower_tests.rs) exercise the program entry point
while retaining arithmetic and CFG regressions.
[Program tests](../compiler/src/ir/program_tests.rs) execute static data, memory,
direct and indirect calls, roots, and injectable runtime/I/O behavior through a
test-only IR machine on both targets. A regression compares lowering field offsets and instance sizes with the shared
class_layout function for both pointer widths and all shared smoke fixtures, plus
a mixed-field inheritance fixture. Tests cover layouts, inheritance, String
operations, constructor delegation, evaluation order, casts, dispatch, persistent
bindings, and shared LO smoke fixtures. The machine does not simulate GC or emit
native/WASM instructions.

Supported fixtures compare results and I/O with the AST interpreter. Two existing
interpreter differences require explicit expected-result tests instead: prebound
assignment panics internally despite being accepted by the checker, and its null
check occurs before argument evaluation. Lowering follows the existing WASM
backend's argument-before-null-check behavior. These differences are documented,
not hidden by changes to the interpreter.

[Verifier tests](../compiler/src/ir/tests.rs) cover local root operations and the
static-store exemption: defined writable Data-symbol bases accept reference
stores, while Ptr-register bases still require barriers. Tests also reject stores
of scalar and reference values into read-only symbols, and reject the root
exemption for external data whose section is unknown. Program tests check the static frame
layout, registration/pop order, and prebound reassignment on both targets. Backend conformance and actual
moving-GC execution are responsibilities of the later passes.
