# LO → WASM emission rules

**Status: WIP.** Proposed rules for discussion; open points remain at the end of this document.

Read each rule as “when we visit this LO construct, emit these operations.”
Production numbers refer to the supplied LO-4 grammar. These are readable emission
sketches; the writer renders actual LLVM WASM assembly and numeric local/label indices.

Input is the planned `TypedProgram` and `ClassTable`: types, bindings, inheritance,
constructor targets, and method targets are already checked. Exact Rust names can change.

## Reading the rules

An expression leaves one `i32` on the WASM operand stack; a statement leaves nothing.
`void` has no result. `emit e` recursively emits an expression.
`save e as x` evaluates e once into a temporary local; later uses read that saved value.
A saved String/class reference stays rooted until consumed, including across later arguments.
`CALL` below includes GC handling. These helpers describe emitter actions, not another IR.

## P1, P4 — program and class declarations

```text
<Program> → <ClassDecl>*
    pre-scan declarations → field offsets, vtable slots, signatures, symbols
    emit descriptors and vtables; visit each constructor/method body once
    emit lo_entry → program.s → llvm-mc → program.o → wasm-ld + runtime → program.wasm

class C extends P (...) [...] {...}
    fields(C) = inherited fields, then own fields in declaration order
    vtable(C) = parent's table, replacing overrides and appending new methods
    descriptor(C) = name, parent, instance size, pointer offsets, vtable
```

Process parents before children. Each object has a 12-byte header, followed by four-byte
fields: field i is at `12 + 4*i`. Instance size is `12 + 4*field_count`; allocation rounding
belongs to the runtime. Pointer offsets include inherited String/class fields.
Overrides retain their slots. Vtables contain function-table indices, resolved by the linker
from symbolic function references. Exact structures follow [ABI §2](../runtime-abi.md).
Buffer emitted instructions so local declarations/frame sizes can precede the body;
this still visits each body once. The frontend supplies the I/O preamble exactly once.

## P35–P39 — values and defaults

| LO type | WASM representation | Initial value | Root? |
|---|---|---|---|
| int | i32 | 0 | No |
| bool | i32, 0 or 1 | 0 | No |
| String | i32 object address | address of `LO_EMPTY_STRING` | Yes |
| class | i32 object address | 0 (null) | Yes |
| void | no value | — | No |

Descriptor addresses, raw literal-byte addresses, and function-table indices are not roots.

## Shared rule — calls and moving GC

```text
CALL target(saved arguments) → optional result
    spill this, reference parameters/locals, and active reference temporaries to roots
    clear inactive temporary root slots; load arguments in parameter order
    call target (or call_indirect with its signature and table index)
    save the result immediately; reload spilled references from updated roots
```

Apply CALL to user calls as well as runtime calls: either may allocate indirectly.
Keep named reference locals rooted for the whole function; release temporaries after use.
A new reference result becomes an active temporary before any further call; reloading
pre-call roots must not overwrite it. Runtime callees protect their own incoming references.
For `r.m(new A(), new B())`, both r and the first object must survive the second allocation.

```text
ENTER → reserve an aligned shadow frame below __stack_pointer; populate its roots;
        lo_push_frame(frame)
LEAVE → spill references, including a reference return value; lo_pop_frame();
        reload the return value if needed; restore __stack_pointer
```

Every generated function gets a frame, including I/O wrappers and lo_entry.
Frames and roots live in linear memory; layout follows [ABI §3.3](../runtime-abi.md).
ENTER/LEAVE rely on the supplied push/pop implementations being non-allocating.

## P7, P9, P11–P12 — functions, locals, and blocks

```text
T m(T1 p1, ..., Tk pk) { declarations; statements }
    function (this: i32, p1: i32, ..., pk: i32) → wasm(T)
        initialize all hoisted locals to defaults; ENTER
        block method_exit
            emit statements
            unreachable if T is non-void
        end
        LEAVE; return saved_return_value if T is non-void

<Block> → { <VarDecl>* <Stmt>+ }
    emit statements in order
```

`this` is parameter 0. Locals declared anywhere in the body are initialized before ENTER,
even when their declaration lies inside a skipped branch. A declaration emits nothing at
its source position and never resets a local on a later loop iteration.
All returns use the shared exit. Void methods and constructors finish by falling through.

## P5–P6 — constructors

```text
C(formals) { delegation? declarations; statements }
    function (this, formals...) → ()
        initialize hoisted locals; ENTER; emit delegation; emit statements; LEAVE

this(a1, ..., ak);  → save actuals left-to-right; CALL ctor(current_class, k)(this, actuals)
super(a1, ..., ak); → save actuals left-to-right; CALL ctor(parent_class, k)(this, actuals)
```

Delegation reuses the same object and preserves its descriptor and field values.
An inheriting class must explicitly delegate; we never invent an implicit `super()`.
Without an explicit constructor section, a root class gets a constructor whose parameters
match its fields and whose body assigns them using the field-store rules below.

## P13–P19 — statements

```text
return e;                       → save e as saved_return_value; br method_exit
if (e) { yes } else { no }       → emit e; if; emit yes; else; emit no; end
while (e) { body }               → block loop_exit
                                      loop loop_head
                                          emit e; i32.eqz; br_if loop_exit
                                          emit body; br loop_head
                                      end
                                  end
break;                          → br nearest_loop_exit
;                               → emit nothing
receiver.m(actuals);             → method-call rule, with no result (void methods only)

x = e;  [local/formal]           → emit e; local.set x
f = e;  [int/bool field]         → save e as v; load this and v; i32.store offset(f)
f = e;  [String/class field]     → save e as v; CALL lo_gc_write_barrier(this, offset(f), v)
```

Track all enclosing WASM blocks/loops/ifs to compute branch depths.
Evaluate a field assignment's RHS before loading this: evaluation may move the object.
The barrier performs the store itself. Use it even for null and default String stores.

## P20–P21, P28, P31–P32, P40–P53 — simple expressions

```text
this                 → local.get 0
null / false / true  → i32.const 0 / 0 / 1
integer n            → i32.const n
local/formal x       → local.get x
field f              → local.get this; i32.load offset(f)
(e)                  → emit e
string literal s     → place decoded UTF-8 bytes in static data;
                       CALL lo_string_new(address(bytes), byte_length) → result
```

An empty literal may use `address(LO_EMPTY_STRING)`. Literal bytes are not a String object.
Names resolve to bindings/symbols at compile time; receiver forms use the call rule below.

## P22 — allocation

```text
new C(a1, ..., ak)
    save actuals left-to-right
    CALL lo_alloc(address(descriptor(C))) → object
    initialize every inherited/own String field to LO_EMPTY_STRING through the barrier
    CALL ctor(C, k)(object, saved actuals)
    produce object
```

Keep object and reference arguments rooted throughout. Use the updated object after calls.
Allocation zeroes other fields; all String defaults are set before any constructor runs.

## P10, P23, P46–P49 — method calls

```text
receiver.m(a1, ..., ak)
    save receiver as object; save actuals left-to-right
    if object is null: CALL lo_abort_null_receiver(method-name bytes, length); unreachable
    slot = m's slot in the receiver's static class
    target = object.descriptor.vtable[slot]
    CALL_INDIRECT signature(m)(object, saved actuals; target) → result

super.m(actuals)
    save this and actuals; apply the same null guard
    CALL nearest ancestor implementation resolved by the checker(this, actuals) → result
```

Arguments run before the null guard (LO §3.4.3). Computed receivers run exactly once.
Ordinary calls, including this.m, dispatch virtually; super calls are direct.
Indirect calls use CALL's root protocol and push the table index after the arguments.
The signature includes this; omit result steps for void methods.

## P25–P27, P33–P34 — operators

```text
(c ? a : b)  → emit c; if (result i32); emit a; else; emit b; end
(a & b)      → emit a; if (result i32); emit b; else; i32.const 0; end
(a | b)      → emit a; if (result i32); i32.const 1; else; emit b; end
```

Only the selected branch runs. The checker supplies the resulting type for root tracking.
For int binary operators, evaluate left then right; use these instructions:

| Operator | Instruction/rule |
|---|---|
| +, -, * | i32.add, i32.sub, i32.mul (wrapping) |
| <, >, = | i32.lt_s, i32.gt_s, i32.eq |
| / | Save operands; return -1 for divisor 0, INT_MIN for INT_MIN / -1, otherwise i32.div_s |
| % | Save operands; return dividend for divisor 0, otherwise i32.rem_s (INT_MIN % -1 is 0) |

For String binary operators, save left then right and use CALL:

| Expression | Runtime operation |
|---|---|
| String + String | lo_string_concat |
| String * int | lo_string_repeat; runtime aborts for negative count |
| String <, >, = String | lo_string_compare, then compare its result with 0 |
| (~ String) | lo_string_reverse; reversal is by Unicode code point |

`(! bool)` emits i32.eqz. `(~ int)` emits `0`, the operand, then i32.sub (wrapping).
Other operand combinations are checker errors. String comparisons use contents.

## P29–P30 — casts and instanceof

```text
((T) e) [upcast/identity] → emit e
((T) e) [downcast]        → save e; CALL lo_cast_check(e, descriptor(T)) → result
(e instanceof T)         → save e; CALL lo_instanceof(e, descriptor(T)) → result
```

A null cast returns null; null instanceof is false. Failed downcasts abort in the runtime.

## Synthetic I/O and entry point

I/O methods get ordinary functions/vtable entries, identified by typed IoOp:
`read_int/read_bool/read_string/eof` call the corresponding `lo_*` functions;
stdout `print_int/print_bool/print_string/println` do likewise. Each uses ENTER/CALL/LEAVE.

```text
lo_entry: () → i32
    lo_runtime_init(); ENTER with persistent roots for in/out/err
    construct and root the I/O instances, then new Main()
    invoke Main.main(); save its integer result; LEAVE; return that result
```

Initialize before ENTER because initialization resets the frame chain. I/O exists before
Main's constructor runs. Store the pre-bound values in persistent entry-frame root slots;
static cells hold their slot addresses. Reads, and assignments if permitted, use those slots.
The collector updates them directly; never overwrite them with a stale cached reference.
Link one memory with the WASM runtime archive; export memory and lo_entry, with no start section.
Emit symbolic runtime/data/function references so llvm-mc and wasm-ld resolve addresses and tables.

## Open points and first check

- **stderr:** Output aliases must preserve their sink, but the print ABI only supports stdout.
  The host's stderr helper buffers aborts. This rule needs an agreed runtime solution.
- **Spec conflicts:** follow P1/ABI: barriers on pointer field stores and frames on every function.
  LO chapter 7 suggests omissions; push/pop also need their non-allocating assumption confirmed.
- **Environment:** verify llvm-mc in the course container. String/cast operations are local stubs.
- **First check:** link a small module exercising a runtime call, LO_EMPTY_STRING, a descriptor,
  indirect dispatch, and a shadow frame; then test evaluation order and actual moving GC.

Sources: supplied P1 §2.4 and LO grammar/semantics; [runtime ABI](../runtime-abi.md),
[frame implementation](../rust/src/shadow_stack.rs), [typed AST plan](type_checker_design.md).
This document specifies the intended lowering; emitter/toolchain validation is still pending.
