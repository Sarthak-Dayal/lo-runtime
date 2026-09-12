# LO → WASM emission rules

This is a WIP design for the WASM emitter. It assumes the planned `TypedProgram`
and `ClassTable` provide checked types, resolved bindings, inheritance, and call
targets; the Rust interface can change as the checker is implemented. The emitter
will produce LLVM WASM assembly, which `llvm-mc` assembles into a relocatable object
and `wasm-ld` links with the runtime.

The rules below use the production numbers from the LO-4 grammar. They describe
emission steps, with names for locals and labels that the writer converts to
numeric indices. An expression leaves one `i32` on the operand stack; a statement
leaves no value, and `void` has no result. `emit e` recursively emits e, while
`save e as x` evaluates it once into a temporary local. CALL saves its optional
result; an expression rule loads its final saved value onto the operand stack.

```text
P1: <Program> → <ClassDecl>*
    pre-scan declarations to assign field offsets, vtable slots, signatures, and symbols
    emit descriptors and vtables
    visit each constructor and method body once
    emit lo_entry
    assemble program.s → program.o; link program.o + runtime → program.wasm

P4: class C extends P (...) [...] {...}
    fields(C) = inherited fields, followed by own fields in declaration order
    vtable(C) = parent's table, replacing overrides and appending new methods
    descriptor(C) = name, parent, instance size, pointer offsets, vtable
```

The declaration scan processes parents before children, so inherited offsets and
method slots are available when laying out a child. An override keeps its slot.
Vtable entries are function-table indices, resolved by the linker from symbolic
function references. The frontend supplies the I/O declarations exactly once.

Each body is visited once, as the project handout requires. Instructions are buffered
so local declarations and the frame size can be written before them. String bytes
go into a static-data buffer during that same traversal.

For P35–P39, values and defaults are represented as follows:

| LO type | WASM representation | Initial value | GC root? |
|---|---|---|---|
| int | i32 | 0 | No |
| bool | i32, 0 or 1 | 0 | No |
| String | i32 object address | address of `LO_EMPTY_STRING` | Yes |
| class | i32 object address | 0 (null) | Yes |
| void | no value | none | No |

Every field occupies four bytes after the 12-byte object header. Field i has offset
`12 + 4*i`, and instance size is `12 + 4*field_count`; the runtime rounds allocations.
The descriptor records all inherited and own String/class field offsets for GC.
Descriptor addresses, literal-byte addresses, and function-table indices are not
roots. The exact data structures follow [ABI §2](../runtime-abi.md).

A call can move objects, so saved references must remain in shadow-stack root slots
until they are consumed. This applies to user methods and constructors as well as
runtime calls, since a user call may allocate indirectly.

```text
CALL target(saved arguments) → optional saved result
    spill this, reference parameters/locals, and active reference temporaries to roots
    clear inactive temporary root slots
    load arguments in parameter order; emit call or call_indirect
    save the result immediately
    reload spilled references from their updated root slots

ENTER
    reserve an aligned shadow frame below __stack_pointer
    populate its roots; lo_push_frame(frame)

LEAVE
    spill references, including a reference return value; lo_pop_frame()
    reload the return value if needed; restore __stack_pointer
```

Named reference locals stay rooted for the whole function; temporary slots are
released after use and cleared before the next call. A new reference result becomes
an active temporary before another call, and reloading pre-call roots must not
overwrite it. In `r.m(new A(), new B())`, r and the first object must both survive
the second allocation. Runtime callees must protect their own incoming references.

Every generated function gets a frame in linear memory, including I/O wrappers and
lo_entry, using [ABI §3.3](../runtime-abi.md). ENTER and LEAVE rely on the supplied
push/pop implementations being non-allocating. The method rule initializes locals
before registering their roots and sends all returns through one exit.

```text
P7, P9: T m(T1 p1, ..., Tk pk) { declarations; statements }
    function (this: i32, p1: i32, ..., pk: i32) → wasm(T)
        initialize all hoisted locals to defaults; ENTER
        block method_exit
            emit statements
            unreachable if T is non-void
        end
        LEAVE; return saved_return_value if T is non-void

P11: T x, y; → allocate locals during function setup; emit nothing here
P12: <Block> → { <VarDecl>* <Stmt>+ }
    emit statements in order
```

Parameter 0 is `this`. A local declared anywhere in the body is initialized at function
entry, including one declared inside a skipped branch. Reaching its declaration
does not reset it, even on later loop iterations. Void methods and constructors
fall through to the exit; non-void fallthrough emits unreachable.

```text
P5–P6: C(formals) { delegation? declarations; statements }
    function (this, formals...) → ()
        initialize hoisted locals; ENTER
        emit delegation, if present; emit statements; LEAVE

this(a1, ..., ak);  → save actuals left-to-right; CALL ctor(current_class, k)(this, actuals)
super(a1, ..., ak); → save actuals left-to-right; CALL ctor(parent_class, k)(this, actuals)
```

Delegation operates on the same object, preserving its descriptor and existing field
values. An inheriting class must explicitly delegate with `this` or `super`. A root
class without an explicit constructor section gets parameters matching its fields
and a body that assigns them using the field-store rules.

```text
P13: return e;                 → save e as saved_return_value; br method_exit
P14: if (e) { a } else { b }    → emit e; if; emit a; else; emit b; end
P15: while (e) { body }         → block loop_exit
                                    loop loop_head
                                        emit e; i32.eqz; br_if loop_exit
                                        emit body; br loop_head
                                    end
                                end
P16: break;                    → br nearest_loop_exit
P18: ;                         → emit nothing
P19: receiver.m(actuals);       → method-call rule, with no result (void methods only)

P17: x = e; [local/formal]      → emit e; local.set x
     f = e; [int/bool field]    → save e as v; load this and v; i32.store offset(f)
     f = e; [reference field]   → save e as v; CALL lo_gc_write_barrier(this, offset(f), v)
```

Branch depths account for every enclosing WASM block, loop, and if. Field assignment
evaluates the RHS before loading this because that evaluation may move the object.
The barrier performs the store itself, including null stores and default String
initialization.

```text
P20: this              → local.get 0
P21: null              → i32.const 0
P28: (e)               → emit e
P31: local/formal x     → local.get x
     field f           → local.get this; i32.load offset(f)
P32, P40: integer n     → i32.const n
P41: true              → i32.const 1
P42: false             → i32.const 0
P43: string literal s  → place decoded UTF-8 bytes in static data
                        CALL lo_string_new(address(bytes), byte_length) → result
```

An empty literal may use `address(LO_EMPTY_STRING)`. Other literals pass raw bytes
and their byte length to the runtime to construct a String object. The remaining
name and token productions resolve upstream; receiver forms use the call rules below.

```text
P22: new C(a1, ..., ak)
    save actuals left-to-right
    CALL lo_alloc(address(descriptor(C))) → object
    initialize inherited/own String fields to LO_EMPTY_STRING through the barrier
    CALL ctor(C, k)(object, saved actuals)
    produce object

P10, P23, P46–P49: receiver.m(a1, ..., ak)
    save receiver as object; save actuals left-to-right
    if object is null: CALL lo_abort_null_receiver(method-name bytes, length); unreachable
    slot = m's slot in the receiver's static class
    target = object.descriptor.vtable[slot]
    CALL_INDIRECT signature(m)(object, saved actuals; target) → result

super.m(actuals)
    save this as object; save actuals left-to-right; apply the same null guard
    CALL nearest ancestor implementation resolved by the checker(object, actuals) → result
```

Allocation zeroes scalar and class-reference fields; every String default is set
before a constructor runs. The new object and reference arguments remain rooted
throughout construction, and subsequent uses read their updated values after calls.

A method call evaluates its receiver once, then its arguments, then checks for null,
following LO §3.4.3. Ordinary calls, including `this.m`, dispatch virtually; `super`
calls use the statically resolved ancestor implementation. CALL_INDIRECT uses CALL's
root handling and pushes the table index after the arguments. Its signature includes
this; void calls omit the result.

```text
P25: (c ? a : b) → emit c; if (result i32); emit a; else; emit b; end
P26: (a & b)     → emit a; if (result i32); emit b; else; i32.const 0; end
     (a | b)     → emit a; if (result i32); i32.const 1; else; emit b; end
```

These rules evaluate only the selected branch. The checker supplies the result type
for root tracking. Other binary operators evaluate left then right. For integer
operands, P26/P33 reduce as follows:

| Operator | Emission |
|---|---|
| +, -, * | i32.add, i32.sub, i32.mul (wrapping) |
| <, >, = | i32.lt_s, i32.gt_s, i32.eq |
| / | Save operands; produce -1 for divisor 0, INT_MIN for INT_MIN / -1, otherwise i32.div_s |
| % | Save operands; produce dividend for divisor 0, otherwise i32.rem_s (INT_MIN % -1 is 0) |

For String operations, save the operands in order and use CALL. Concatenation
`String + String` calls lo_string_concat, and repetition `String * int` calls
lo_string_repeat, which aborts for a negative count. String `<`, `>`, and `=`
call lo_string_compare and compare its result with zero.

Unary expressions in P27/P34 use i32.eqz for `(! bool)`; `(~ int)` emits zero,
the operand, then i32.sub with wrapping arithmetic. `(~ String)` saves the operand
and calls lo_string_reverse, which reverses Unicode code points. Other operand
combinations are checker errors.

```text
P29: ((T) e) [upcast/identity] → emit e
     ((T) e) [downcast]        → save e as object
                                CALL lo_cast_check(object, address(descriptor(T))) → result
P30: (e instanceof T)         → save e as object
                                CALL lo_instanceof(object, address(descriptor(T))) → result
```

The runtime returns null for a null cast and false for null instanceof, and aborts
on a failed downcast.

Synthetic I/O methods get functions and vtable entries identified by typed `IoOp`.
Input's read_int, read_bool, read_string, and eof call the corresponding `lo_*`
functions; stdout Output's print_int, print_bool, print_string, and println do
likewise. Each wrapper uses ENTER, CALL, and LEAVE, returning read results through
the shared exit.

```text
lo_entry: () → i32
    lo_runtime_init(); ENTER with persistent roots for in/out/err
    construct and root the I/O instances, then new Main()
    invoke Main.main(); save its integer result; LEAVE; return that result
```

Runtime initialization precedes ENTER because it resets the frame chain. I/O
instances exist before Main's constructor runs. Their pre-bound values live in
persistent entry-frame root slots, with static cells holding the slot addresses.
Reads, and assignments if permitted, use those slots so they observe GC updates.

The linker combines the object with the WASM runtime archive into one module with
one memory, exporting memory and lo_entry with no start section. Runtime, data, and
function references remain symbolic until assembly and linking resolve them.

Several parts still need confirmation. Output aliases must preserve their sink,
but the print ABI supports stdout and the host's stderr helper buffers aborts;
stderr output needs an agreed runtime solution. This design follows P1 and the ABI
on barriers for pointer field stores and frames for every function, where LO
chapter 7 suggests omissions. The non-allocating push/pop assumption also needs
confirmation against the runtime contract.

Assembler availability and linkage have not been tested in the course container,
and the local String/cast operations are stubs. The first implementation check
should link a small module using a runtime call, LO_EMPTY_STRING, a descriptor,
indirect dispatch, and a shadow frame, followed by evaluation-order tests and
allocation pressure that triggers moving GC.

The rules are based on the supplied P1 §2.4 and LO grammar/semantics, the
[runtime ABI](../runtime-abi.md), the [frame implementation](../rust/src/shadow_stack.rs),
and the [typed AST plan](type_checker_design.md).
