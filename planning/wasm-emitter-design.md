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

The coverage table accounts for all P1–P53 in order. The LO-4 appendix includes
49 of them; P2, P3, P8, and P24 apply only to earlier levels. “Specified” means a
lowering rule is present here, not that the emitter has been implemented or tested.
The [synthetic I/O rule](#io) remains partial for stderr.

| Production | Construct | Design coverage |
|---|---|---|
| P1 | Class-based program | [Specified](#program) |
| P2 | Method-only program | Not in LO-4 |
| P3 | Bare-body program | Not in LO-4 |
| P4 | Class / extends | [Specified](#program) |
| P5 | Constructor / this delegation | [Specified](#constructors) |
| P6 | Constructor / super delegation | [Specified](#constructors) |
| P7 | Method declaration | [Specified; synthetic stderr partial](#functions) |
| P8 | Standalone Body | Not in LO-4 |
| P9 | Formals | [Specified](#functions) |
| P10 | Actuals | [Specified](#calls) |
| P11 | Variable declaration | [Specified](#functions) |
| P12 | Block | [Specified](#functions) |
| P13 | Return | [Specified](#statements) |
| P14 | If / else | [Specified](#statements) |
| P15 | While | [Specified](#statements) |
| P16 | Break | [Specified](#statements) |
| P17 | Assignment | [Specified, including pre-bound names](#statements) |
| P18 | Empty statement | [Specified](#statements) |
| P19 | Void method call | [Specified; I/O uses wrappers](#calls) |
| P20 | this expression | [Specified](#simple) |
| P21 | null | [Specified](#simple) |
| P22 | new | [Specified](#allocation) |
| P23 | Value-returning method call | [Specified](#calls) |
| P24 | Unqualified method call | Not in LO-4 |
| P25 | Ternary | [Specified](#operators) |
| P26 | Binary expression | [Specified](#operators) |
| P27 | Unary expression | [Specified](#operators) |
| P28 | Parenthesized expression | [Specified](#simple) |
| P29 | Cast | [Specified](#casts) |
| P30 | instanceof | [Specified](#casts) |
| P31 | Variable expression | [Specified](#simple) |
| P32 | Literal expression | [Delegates to P40–P43](#simple) |
| P33 | Binary operator | [Selected by checked operand types](#operators) |
| P34 | Unary operator | [Selected by checked operand type](#operators) |
| P35 | void type | [Representation specified](#types) |
| P36 | Class type | [Representation specified](#types) |
| P37 | int type | [Representation specified](#types) |
| P38 | bool type | [Representation specified](#types) |
| P39 | String type | [Representation specified](#types) |
| P40 | Number literal | [Specified](#simple) |
| P41 | true | [Specified](#simple) |
| P42 | false | [Specified](#simple) |
| P43 | String literal | [Specified](#simple) |
| P44 | ClassName | [Resolved upstream; symbol/descriptor lookup](#names) |
| P45 | MethodName | [Resolved upstream; method/symbol lookup](#names) |
| P46 | Variable receiver | [Specified](#calls) |
| P47 | this receiver | [Specified](#calls) |
| P48 | super receiver | [Specified](#calls) |
| P49 | Computed receiver | [Specified](#calls) |
| P50 | Var → Identifier | [Resolved upstream; binding read/store](#names) |
| P51 | Num token | [Validated upstream; P40 emits its value](#names) |
| P52 | String token | [Decoded upstream; P43 emits its value](#names) |
| P53 | Identifier token | [Validated upstream; no independent emission](#names) |

The reductions follow below. [Root publication](#roots) defines the shared call
protocol; [confirmed decisions and remaining dependencies](#decisions) close the document.

<a id="program"></a>

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

<a id="types"></a>

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
The pre-scan includes compiler-generated fields, such as Output's sink tag below.
The descriptor records all inherited and own String/class field offsets for GC.
Descriptor addresses, literal-byte addresses, and function-table indices are not
roots. The exact data structures follow [ABI §2](../runtime-abi.md).

<a id="roots"></a>

Root publication means copying an object reference into a slot the collector knows
how to find. A WASM local holding `4096` and a root slot holding `4096` are two
copies of the address of one object. Publishing copies the address, not the object.

| Storage | What lives there | How this collector uses it |
|---|---|---|
| WASM locals and operand stack | Integers, reference values, temporary results, frame addresses | Cannot scan or update these directly |
| WASM global `__stack_pointer` | Address of the current linear-memory stack boundary | Used to reserve frames; it is not a root |
| Linear-memory stack | Shadow-frame headers and inline root slots | Follows registered frames and rewrites their slots |
| Linear-memory heap | Objects, fields, String headers and bytes | Moves reachable objects and updates their reference fields |
| Linear-memory static data | Descriptors, vtables, literal bytes, `LO_EMPTY_STRING`, runtime state | Remains at fixed addresses; metadata tells GC which heap fields to scan |

The linker lays out static data and the stack; the runtime obtains heap storage
in that same memory. Reserving a shadow frame uses the shared `__stack_pointer`, so
generated functions and runtime functions use the same stack convention.
WASM's own [call frames and locals](https://webassembly.github.io/spec/core/exec/runtime.html#call-frames)
are distinct from these explicitly allocated shadow frames.

For each function the emitter assigns one root slot to `this` when present, each reference
parameter/local, each reference temporary, and a reference return value if needed.
It keeps the local-to-slot map in the compiler; emitted loads/stores use its fixed
offsets. N is the total slot count, including slots needed on either branch.
For lo_entry, N also includes the three persistent I/O slots described below.
Named locals keep their slots throughout the function; temporary slots become
inactive after consumption. Unused slots contain zero.

```text
frame + 0            : parent frame address, i32
frame + 4            : N, i32
frame + 8 + 4*i      : root i, i32 object address (inline in the frame)

PUBLISH
    for each active mapped reference x:
        mem32[frame + 8 + 4*slot(x)] = local x
    write 0 to inactive temporary/unused return slots

RELOAD
    for each reference published before the call:
        local x = mem32[frame + 8 + 4*slot(x)]

ENTER
    old_sp = global.get __stack_pointer
    frame = old_sp - align_up(8 + 4*N, 16)
    global.set __stack_pointer = frame
    mem32[frame] = 0; mem32[frame + 4] = N; zero all root slots
    PUBLISH initialized this, reference parameters, and defaulted reference locals
    call lo_push_frame(frame); RELOAD

CALL target(saved arguments) → optional saved result
    PUBLISH; load arguments in parameter order; emit call or call_indirect
    save the result into a fresh result local; RELOAD the pre-call references
    make a reference result active before any further call

LEAVE
    PUBLISH, including the saved reference return value
    call lo_pop_frame()
    reload a reference return value from its slot
    global.set __stack_pointer = old_sp
```

Here `mem32[address] = value` means `i32.store`; the reverse assignment uses
`i32.load` and `local.set`. Root stores do not use the heap-field write barrier.
`lo_push_frame` sets the frame's parent to the previous head and makes this frame
the new head. The runtime's head pointer lives in its static state in linear memory.
It registers the frame once; PUBLISH only updates slots before subsequent calls.

Suppose a is in slot 2 and frame is 1024, so its root slot is at address 1040.
The following is an illustrative collection during a call:

| Step | WASM local a | mem32[1040] |
|---|---|---|
| Allocation returns address 4096 into a | 4096 | 0 |
| Caller publishes a before its next call | 4096 | 4096 |
| GC moves the object to 8192 and rewrites the slot | 4096 | 8192 |
| Caller reloads a after the call | 8192 | 8192 |

Between calls, locals may be newer than their slots. At every allocating call,
all live references must be published; after it, locals must be reloaded before
use. A new result must not be overwritten by a pre-call reload. The collector
walks every registered frame, then follows object reference fields using descriptor
offsets; merely storing a pointer somewhere in linear memory does not make it a root.
This is how the [provided collector](../rust/src/gc.rs) finds and updates pointers.

In `r.m(new A(), new B())`, the saved receiver and A object are published before
allocating B. A reference left only on the operand stack would be lost to GC.
User calls use the same protocol as runtime calls, and runtime callees must protect
their own incoming references during internal allocations.

Every generated function gets a frame, including wrappers and lo_entry. The
[provided push/pop functions](../rust/src/shadow_stack.rs) only link/unlink frames
and do not allocate; they use the explicit ENTER/LEAVE sequences, not recursive
CALL expansion. Frame memory stays reserved through pop and the return reload.
A replacement runtime must preserve that lifecycle: it cannot collect before
publishing an entering frame or after removing an exiting frame's last live roots.

<a id="functions"></a>

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

<a id="constructors"></a>

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

<a id="statements"></a>

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
P17: x = e; [local/formal]      → emit e; local.set x
     f = e; [int/bool field]    → save e as v; load this and v; i32.store offset(f)
     f = e; [reference field]   → save e as v; CALL lo_gc_write_barrier(this, offset(f), v)
     g = e; [pre-bound name]    → save e as v; mem32[slot_address(g)] = v
P18: ;                         → emit nothing
P19: receiver.m(actuals);       → method-call rule, with no result (void methods only)
```

Branch depths account for every enclosing WASM block, loop, and if. Field assignment
evaluates the RHS before loading this because that evaluation may move the object.
The barrier performs the store itself, including null stores and default String
initialization.

<a id="simple"></a>

```text
P20: this              → local.get 0
P21: null              → i32.const 0
P28: (e)               → emit e
P31: local/formal x     → local.get x
     field f           → local.get this; i32.load offset(f)
     pre-bound g       → i32.load slot_address(g)
P32: <Expr> → <Literal> → emit the corresponding P40–P43 rule
P40: integer n          → i32.const n
P41: true              → i32.const 1
P42: false             → i32.const 0
P43: string literal s  → place decoded UTF-8 bytes in static data
                        CALL lo_string_new(address(bytes), byte_length) → result
```

An empty literal may use `address(LO_EMPTY_STRING)`. Other literals pass raw bytes
and their byte length to the runtime to construct a String object.

<a id="names"></a>

P44/P45 resolve class and method identifiers to descriptor/function symbols and
method metadata. P50 resolves a variable identifier to its checked binding for
P17/P31. P51–P53 are lexical rules: the frontend validates numeric and identifier
spelling and decodes String contents; their values reach P40/P43. They need no
separate WASM instruction sequences.

<a id="allocation"></a>

```text
P22: new C(a1, ..., ak)
    save actuals left-to-right
    CALL lo_alloc(address(descriptor(C))) → object
    initialize inherited/own String fields to LO_EMPTY_STRING through the barrier
    CALL ctor(C, k)(object, saved actuals)
    produce object
```

<a id="calls"></a>

```text
P46: <ObjName> → <Var>    → read its binding as in P31
P47: <ObjName> → this     → local.get 0
P48: <ObjName> → super    → local.get 0; select a direct ancestor call
P49: <ObjName> → (e)      → emit e once
P10: <Actuals> → a1, ..., ak → save each actual left-to-right

P23: receiver.m(a1, ..., ak)
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

<a id="operators"></a>

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

<a id="casts"></a>

```text
P29: ((T) e) [upcast/identity] → emit e
     ((T) e) [downcast]        → save e as object
                                CALL lo_cast_check(object, address(descriptor(T))) → result
P30: (e instanceof T)         → save e as object
                                CALL lo_instanceof(object, address(descriptor(T))) → result
```

The runtime returns null for a null cast and false for null instanceof, and aborts
on a failed downcast.

<a id="io"></a>

Synthetic I/O methods get functions and vtable entries identified by typed `IoOp`.
Input's read_int, read_bool, read_string, and eof call the corresponding `lo_*`
functions; stdout Output's print_int, print_bool, print_string, and println do
likewise. Each wrapper uses ENTER, CALL, and LEAVE, returning read results through
the shared exit.

For sink identity, the proposed Output layout adds one compiler-private i32 at
offset 12: 0 for stdout, 1 for stderr. Both objects use the same Output descriptor,
with instance size 16 and no pointer fields. Entry initialization stores the tag;
print wrappers read it from their receiver. Aliases therefore retain their sink
even if a pre-bound variable is reassigned. The stderr branch still lacks its
runtime write operation, as recorded below.

```text
lo_entry: () → i32
    lo_runtime_init(); ENTER with persistent roots for in/out/err
    for each pre-bound g: mem32[address(binding_cell(g))] = frame + 8 + 4*slot(g)
    construct and root the I/O instances, then new Main()
    invoke Main.main(); save its integer result; LEAVE; return that result
```

Runtime initialization precedes ENTER because it resets the frame chain. I/O
instances exist before Main's constructor runs. The three pre-bound values live in
dedicated entry-frame root slots, initially zero, with static cells holding the
slot addresses: `slot_address(g) = mem32[address(binding_cell(g))]`.
Reading g loads its slot address and then the reference; assigning
g updates that same slot. These slots remain registered for the entire execution.
PUBLISH never clears them or copies a cached reference over them.

The [checker implementation plan](type_checker_implementation.md) represents these
bindings as `Field { owner: "<preamble>", ... }`. The emitter must recognize that
binding before applying heap-field rules. The plan permits assignment to them;
their storage is always the persistent root slot, not a field on `this`.

The linker combines the object with the WASM runtime archive into one module with
one memory, exporting memory and lo_entry with no start section. Runtime, data, and
function references remain symbolic until assembly and linking resolve them.

<a id="decisions"></a>

Checking the supplied documents and runtime sources on 2026-09-12 resolves the following:

| Topic | Finding and design decision |
|---|---|
| Frames and barriers | P1 §2.4 explicitly requires a frame in every function and a barrier on every pointer field store, and names the ABI as authoritative. Follow those requirements despite LO §§7.2/7.5's omissions; no emitter decision remains open. |
| Publishing and reloading | ABI §3.3 defines inline root slots; the supplied collector rewrites those slots. The explicit ENTER/PUBLISH/RELOAD/LEAVE rules above implement that mechanism. |
| Push/pop | Source inspection confirms the supplied functions do not allocate. This settles the current implementation; the frame lifecycle described above is a requirement on replacement runtimes. |
| Typed input | The teammate-owned interface remains an integration dependency, as agreed. Binding kinds, expression types, resolved call owners, and cast directions are the required facts. Codegen assigns offsets and vtable slots. |
| String/cast stubs | ABI §4.4 explicitly says the grading runtime supplies these operations. Their local stubs limit local execution, not the required lowering. |

Two dependencies remain unresolved. The LO reference §3.4.6 requires ordinary
stderr output, including calls through an Output formal. ABI §3.7 provides no sink
argument to the print functions, and [wasmrun's stderr import](../tools/wasmrun/src/main.rs)
buffers an abort message rather than writing an ordinary stream. This remains true
on the current remote main branch. We need the supported course runtime/host path
for stderr; preserving the sink tag alone cannot supply it. The concrete question
for course staff is which entry point should implement `err.print_*` and whether
the grading host supports ordinary stderr writes. No ABI change is assumed here.

The course Dockerfile/image is also needed for toolchain verification. Local
inspection found neither llvm-mc nor wasm-ld, and Apple clang lacks a WASM target.
The handout promises wasm-ld and wasmrun in the course environment but does not
explicitly promise llvm-mc. Once that environment is available, verify
`llvm-mc --version` lists WASM support and assemble/link a module exercising a
runtime call, LO_EMPTY_STRING, a descriptor, indirect dispatch, and a shadow frame.
Then check evaluation order and trigger an actual moving collection. These checks
have not run; the assembler route is supported by the
[LLVM assembler](https://llvm.org/docs/CommandGuide/llvm-mc.html) and
[WASM linker](https://lld.llvm.org/WebAssembly.html) documentation, but availability
in the course image is an environment question.

The rules and coverage inventory are based on supplied P1 §2.4 and LO Appendix A.5,
the LO dynamic semantics, and the [runtime ABI](../runtime-abi.md).
