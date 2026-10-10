# LO → WASM emission rules

This WIP design describes how the emitter translates LO into WASM. Its inputs
are `TypedProgram` and `ClassTable` from the
[type checker in PR #5](../compiler/src/type_checker.rs), which supplies checked
types, bindings, inheritance, and call targets. The emitter visits each body
once, writes LLVM WASM assembly, and uses `llvm-mc` and `wasm-ld` to produce the module.

The [implementation](../compiler/README.md) now includes the runtime entry sequence,
calls, objects, GC frames, control flow, and all typed expression handlers. The
[production index](wasm-production-index.md) maps the rules to functions. The
[test report](wasm-test-results.md) records execution coverage and remaining
frontend/runtime dependencies.

For a first reading, follow the [worked example and review guide](wasm-emitter-review-guide.md).
It explains the operand stack, names and indices, binary encoding, and relocations.
Use this document to review a particular LO rule. Questions can cite a production
such as P23, a shared operation such as PUBLISH, or a guide stage such as E3.

<a id="notation"></a>

The blocks below are symbolic emission sketches. `$this`, `$receiver`, and `$value`
name WASM locals; `$method_exit` and `$loop_exit` identify branch targets in the
explanation. The writer emits numeric local indices and calculates numeric branch
depths from the nesting at each branch site, including enclosing WASM `if` blocks.
Thus P20 uses `local.get $this`, while the LLVM assembly writer will write
`local.get 0` for that parameter.

| Notation | Meaning |
|---|---|
| `emit e` | Visit an LO expression and emit code that leaves its value on the operand stack. |
| `save e as $value` | Emit e, then `local.set $value`. Give a reference-valued scratch local a root slot. |
| `local.get $value` | Copy the saved local's current value onto the operand stack. |
| `CALL f(arguments) as $result` | Publish roots, load saved arguments, call f, save its result, and reload roots. It leaves no result on the operand stack. |
| `CALL f(arguments)` | The same protocol for a function with no result. |
| `load32(address)` / `store32(address, value)` | Emit a four-byte memory load/store. These are instruction-writing helpers, not runtime calls. |
| `$x = operation` | Emit the operation's value, then `local.set $x`. Arithmetic on saved values expands into the corresponding WASM instructions. |
| `read_global(symbol)` / `write_global(symbol, value)` | Emit global.get/global.set for the named WASM global. |
| `address(symbol)` | Emit a symbolic data address for the assembler/linker to resolve. It performs no allocation. |
| `field_offset(f)`, `vtable_slot(m)`, `signature(m)` | Compiler lookups using the checked declaration/call. They generate constants or type references. |
| `[int field]`, `[T is non-void]` | Select an emission case using checked types. These conditions do not generate runtime branches. |

Each expression sketch finishes with one i32 on the operand stack. In the Rust
implementation, expression handlers save that value in an owned scratch local
and return its index; the consuming handler emits local.get and then releases it.
This keeps references out of the operand stack during other expression calls.
Each statement leaves no value on normal completion. A rule containing CALL loads its
saved result when it needs to produce an expression value. Named scratch locals
are assigned while compiling; creating one does not allocate an LO heap object.
load32 consumes an address and pushes the loaded value. store32 consumes an address
and a value, leaving no result; the helper emits the address before the value.
Loops over declarations, roots, or fields below describe compiler traversal and
expand into instructions. An emitted WASM `loop` describes runtime repetition.

<a id="coverage"></a>

The [production index](wasm-production-index.md) lists P1–P53 in order and links
each rule to its design and implementation. P2, P3, P8, and P24 belong to earlier
LO levels. The [I/O rules](#io) cover dynamic stdout/stderr selection.

<a id="program"></a>

P1 first assigns metadata needed by later calls and field accesses. The emitter
buffers each body's output so local declarations and frame size can precede the
instructions, even when scratch locals are discovered during that body's visit.
This is a declaration scan followed by one body-emission pass, as the project requires.
The output links a wasm32-unknown-unknown runtime archive into one module with one
linear memory, exports `memory` and `lo_entry: () → i32`, and has no start section.
The host invokes lo_entry to run the program.

```text
P1: <Program> → <ClassDecl>*
    pre-scan declarations for field offsets, vtable slots, signatures, and symbols
    emit class descriptors and vtables
    emit each constructor and method body once
    emit lo_entry
    write program.s
    assemble program.s into program.o
    link program.o with the WASM runtime archive into program.wasm

P4: class C (extends P)? (...) [...] {...}
    fields(C) = fields(P), followed by C's own fields
    vtable(C) = vtable(P), replacing overrides and appending new methods
    descriptor(C) = name, parent, size, pointer offsets, vtable
```

The checker resolves parents before children and preserves declaration order
within each class. The emitter consumes those completed layouts; assembly symbols
can refer forward to a parent's metadata.
A root class has no inherited fields or slots. An override keeps its slot; an
inherited method keeps its ancestor's implementation. The checker injects the
Input/Output declarations once. Use `ClassInfo.effective_fields` for inherited
field order, `vtable` and `method_slot` for method order, and `effective_methods`
to find each implementation's declaring class. Vtable entries hold function-table indices, which
are different from function indices and memory addresses; [guide E1](wasm-emitter-review-guide.md#e1)
distinguishes them.

<a id="types"></a>

P35–P39 determine both representation and GC treatment. All LO values use i32,
but only String/class references are roots. A numeric value that happens to equal
an object address remains an integer.

| LO type | Representation | Default | Root? |
|---|---|---|---|
| void | no result | none | No |
| class C | i32 object address | 0, meaning null | Yes |
| int | i32 | 0 | No |
| bool | i32, 0 or 1 | 0 | No |
| String | i32 object address | address of LO_EMPTY_STRING | Yes |

Every field occupies four bytes after the 12-byte object header. Field i has byte
offset `12 + 4*i`; instance size is `12 + 4*field_count`, with allocation rounding
performed by the runtime. Field count includes compiler-generated fields such as
Output's sink tag. Pointer offsets include inherited String/class fields.

The call rules use named ABI offsets to make their purpose visible:

| Named offset | Bytes from the containing structure's start |
|---|---|
| `offset(Object.class_descriptor)` | 0 |
| `offset(ClassDescriptor.vtable)` | 28 |
| `offset(ShadowFrame.parent)` | 0 |
| `offset(ShadowFrame.num_roots)` | 4 |
| `offset(ShadowFrame.roots)` | 8 |

These come from [ABI §§2–3.3](../runtime-abi.md) with four-byte WASM pointers.
Descriptor addresses, raw literal-byte addresses, and function-table indices are
metadata, not roots. The String bytes themselves live in the String object.

<a id="roots"></a>

Root publication copies an object address into a registered shadow-frame slot.
It does not copy the object. The runtime can inspect linear memory, but this
collector cannot discover or update pointers held only in WASM locals or on the
operand stack.

| Storage | Contents | Collector behavior |
|---|---|---|
| WASM locals and operand stack | Values, scratch results, frame addresses | Cannot scan or rewrite these directly |
| WASM global __stack_pointer | Linear-memory stack boundary | Used to reserve frames; not a root |
| WASM function table | Function references selected by call_indirect | Separate from LO's managed-object heap |
| Linear-memory stack | Shadow-frame headers and inline root slots | Walks registered frames and rewrites slots |
| Linear-memory heap | Objects, fields, String headers and bytes | Moves reachable objects and updates reference fields |
| Linear-memory static data | Descriptors, vtables, literal bytes, LO_EMPTY_STRING, runtime state | Fixed addresses; metadata describes heap fields |

The linker lays out static data and the stack; the runtime obtains heap storage
in the same memory. Generated and runtime functions share __stack_pointer.
WASM's own [call frames and locals](https://webassembly.github.io/spec/core/exec/runtime.html#call-frames)
are separate from the shadow frames that we allocate explicitly.

The compiler assigns one slot to `$this` when present, each reference parameter
and local, each reference scratch value, and a reference return value if needed.
The local-to-slot map exists in the compiler; emitted loads and stores use its
offsets. N includes slots used on either branch and, in lo_entry, the three
persistent I/O slots. Named locals remain active for the function's lifetime;
scratch slots become inactive once their values are consumed.
The implementation clears both the scratch local and its root slot at release.
Before registration it zeroes every slot, including slots discovered later in the
body. PUBLISH therefore only needs to store active references: inactive slots
already hold zero, including across loop iterations.

```text
frame layout in linear memory:
    $frame + 0          : parent frame address
    $frame + 4          : N, the number of root slots
    $frame + 8 + 4*i    : root i, an inline i32 object address

root_address($reference) = $frame + 8 + 4*slot($reference)
```

<a id="publish"></a>

PUBLISH updates root slots before a call. RELOAD copies their possibly changed
values back afterward. Each store is a plain i32.store: writing a root slot is
not a heap-field assignment and does not require the write barrier.

```text
PUBLISH
    for each active mapped reference $reference:
        store32(root_address($reference), $reference)
    clear inactive scratch and unused return slots to zero
    leave the persistent I/O slots alone

RELOAD(published_references)
    for each $reference in published_references:
        $reference = load32(root_address($reference))
```

For example, suppose `$a` uses slot 2 and `$frame` is 1024. Its slot address is
1040. If a call triggers collection, the copies of the address change as follows:

| Step | Local $a | Memory at 1040 |
|---|---|---|
| Allocation returns 4096 into $a | 4096 | 0 |
| PUBLISH before the next call | 4096 | 4096 |
| GC moves the object to 8192 and updates the root | 4096 | 8192 |
| RELOAD after the call | 8192 | 8192 |

Between calls a local can be newer than its slot. Before every ordinary call,
including a user call, all live references must be published. The collector
walks registered frames and then the fields listed in object descriptors;
an arbitrary pointer stored elsewhere in memory is not automatically a root.
The [provided collector](../rust/src/gc.rs) implements that traversal.

<a id="call-protocol"></a>

CALL performs the same sequence for runtime functions, constructors, and user
methods. “Saved arguments” means their expressions have already run; CALL never
reevaluates them. Keeping the set published before the call prevents a newly
returned reference from being overwritten by a reload of an older value.

```text
CALL target(saved arguments) as $result
    PUBLISH and remember the set of published references
    push each saved argument in parameter order
    emit call target
    local.set $result
    RELOAD(the pre-call set)
    mark $result active if its checked result type is a reference

CALL_INDIRECT signature(saved arguments; $table_index) as $result
    use the same sequence
    push $table_index after all arguments
    emit call_indirect with the selected signature instead of call
```

A void call omits the result local and its activation. A caller must save every
reference needed across later argument evaluation: in `r.m(new A(), new B())`,
the receiver and A object must survive B's allocation. Runtime callees must
protect their own incoming references during internal allocations.

<a id="frame-lifecycle"></a>

ENTER registers one frame per generated function. It runs after local defaults
are initialized. Global reads/writes below access __stack_pointer; the arithmetic
reserves aligned bytes in the linear-memory stack rather than allocating a heap object.

```text
ENTER
    $old_sp = read_global(__stack_pointer)
    $frame = $old_sp - align_up(8 + 4*N, 16)
    write_global(__stack_pointer, $frame)
    store32($frame + offset(ShadowFrame.parent), 0)
    store32($frame + offset(ShadowFrame.num_roots), N)
    zero all root slots
    PUBLISH the initialized reference parameters and locals
    call lo_push_frame($frame)
    RELOAD(the published references)

LEAVE
    PUBLISH, including the saved reference return value
    call lo_pop_frame()
    reload a reference return value from its slot, if present
    write_global(__stack_pointer, $old_sp)
```

The runtime's head pointer is static state in linear memory. lo_push_frame links
the new frame to the previous head; later PUBLISH operations only update slots.
The [supplied push/pop functions](../rust/src/shadow_stack.rs) do not allocate, so
ENTER/LEAVE call them directly without recursively expanding CALL. Frame memory
remains reserved through pop and the return reload. A replacement runtime must
preserve this lifetime and cannot collect after removing the only roots of a
return value. Every method, constructor, I/O wrapper, and lo_entry gets a frame.

<a id="functions"></a>

P7/P9 make the receiver an explicit first parameter named `$this`; source formals
follow it. The shared exit ensures a source return never bypasses LEAVE.

```text
P7, P9: T m(T1 p1, ..., Tk pk) { declarations; statements }
    emit function method_symbol(C, m)($this, $p1, ..., $pk) → wasm(T)
        initialize all hoisted locals to their type defaults
        ENTER
        block $method_exit
            emit each statement
            [T is non-void] emit unreachable for fallthrough
        end
        LEAVE
        [T is non-void] local.get $return_value
        end_function

P11: T x, y;
    [in a class header] add fields x and y to P4's layout
    [in a body] assign hoisted locals $x and $y during function setup
    emit no instructions at the declaration's source position

P12: { declarations; statements }
    emit each statement in source order
```

A local inside a skipped branch still receives its default at function entry;
reaching its declaration on a later loop iteration does not reset it. A source
block creates no additional shadow frame. Void methods and constructors fall
through to the shared cleanup. Non-void fallthrough is unreachable, as required
by the checked return-path contract.

<a id="constructors"></a>

Constructors receive an existing object and return no WASM value. Their symbols
are selected by class and arity. Delegation preserves the object's descriptor and
existing fields; argument expressions and constructor bodies can still allocate.

```text
P5/P6: C(formals) { delegation? declarations; statements }
    emit function ctor_symbol(C, arity)($this, formals...) → ()
        initialize hoisted locals
        ENTER
        emit delegation, if present
        emit statements
        LEAVE
        end_function

P5 [this delegation]: this(a1, ..., ak);
    save each actual left-to-right as $arg_1, ..., $arg_k
    CALL ctor_symbol(current_class, k)($this, $arg_1, ..., $arg_k)

P6 [super delegation]: super(a1, ..., ak);
    save each actual left-to-right as $arg_1, ..., $arg_k
    CALL ctor_symbol(parent_class, k)($this, $arg_1, ..., $arg_k)
```

An inheriting class must explicitly delegate with this or super. A root class
without a constructor section gets parameters matching its fields and a body
that stores those parameters into fields using P17, including reference barriers.
Those generated stores target fields directly, avoiding source-name shadowing.

<a id="statements"></a>

P13–P19 produce no operand-stack value on normal completion. In P14/P15 the emitted
if and br_if consume the condition. Labels describe which construct to exit;
[guide E1](wasm-emitter-review-guide.md#e1) shows how nesting determines branch depths.

```text
P13: return e;
    save e as $return_value
    br $method_exit

P14: if (condition) { then_statements } else { else_statements }
    emit condition
    if
        emit then_statements
    else
        emit else_statements
    end

P15: while (condition) { body }
    block $loop_exit
        loop $loop_head
            emit condition
            i32.eqz
            br_if $loop_exit
            emit body
            br $loop_head
        end
    end

P16: break;
    br the nearest enclosing $loop_exit
```

P17 chooses storage using the resolved binding. Save the RHS before accessing a
field, since evaluating it may move `$this`. The barrier function performs the
reference store itself, including null stores and String-default initialization.

```text
P17 [local/formal]: x = e;
    emit e
    local.set $x

P17 [int/bool field]: f = e;
    save e as $value
    store32($this + field_offset(f), $value)

P17 [String/class field]: f = e;
    save e as $value
    CALL lo_gc_write_barrier($this, field_offset(f), $value)

P17 [pre-bound name]: g = e;
    save e as $value
    store32(slot_address(g), $value)

P18: ;
    emit nothing

P19: receiver.m(actuals);
    use P23's call sequence with no result
```

Only void methods are allowed in P19. A non-void method's result cannot be silently
dropped as a statement; the checker rejects that source form.

<a id="simple"></a>

These reads and literals leave one value. load32 uses the address shown, loads four
bytes, and leaves the loaded value on the operand stack. Reading a pre-bound name
first obtains the address of its persistent root slot, as specified in the entry rule.

```text
P20: this
    local.get $this

P21: null
    i32.const 0

P28: (e)
    emit e

P31 [local/formal]: x
    local.get $x

P31 [field]: f
    load32($this + field_offset(f))

P31 [pre-bound name]: g
    load32(slot_address(g))

P32: <Expr> → <Literal>
    emit the corresponding P40–P43 rule

P40: integer n → i32.const n
P41: true      → i32.const 1
P42: false     → i32.const 0

P43: String literal s
    place s's decoded UTF-8 bytes in static data under a symbol
    CALL lo_string_new(address(that symbol), byte_length(s)) as $string
    local.get $string
```

An empty literal may use address(LO_EMPTY_STRING). Other literals pass raw bytes
and their byte length to the runtime to construct an object. The raw buffer and
the resulting String have different layouts and lifetimes.

<a id="names"></a>

P44/P45 resolve class and method identifiers to symbols and metadata. P50 resolves
a variable identifier to the binding used by P17/P31. P51–P53 are lexical rules:
the frontend checks numeric and identifier spelling and decodes String contents.
Their values reach P40/P43; they emit no separate instruction sequences.

<a id="allocation"></a>

P22 evaluates all actuals before allocation. The new object and reference actuals
stay active across initialization and the constructor call, so CALL updates their
locals after any collection. Allocation zeroes scalar/class fields; String fields
need their nonzero empty-string defaults before any constructor executes.

```text
P22: new C(a1, ..., ak)
    save actuals left-to-right as $arg_1, ..., $arg_k
    CALL lo_alloc(address(descriptor(C))) as $object
    for each inherited or own String field f:
        CALL lo_gc_write_barrier($object, field_offset(f), address(LO_EMPTY_STRING))
    CALL ctor_symbol(C, k)($object, $arg_1, ..., $arg_k)
    local.get $object
```

<a id="calls"></a>

P46–P49 determine how to obtain a receiver; P10 determines actual evaluation order.
Bare super is not a value expression. Its receiver rule passes `$this` while
selecting a direct ancestor call instead of virtual dispatch.

```text
P46: <ObjName> → <Var>
    read its binding using P31

P47: <ObjName> → this
    local.get $this

P48: <ObjName> → super
    local.get $this
    record direct ancestor dispatch as the compiler's call choice

P49: <ObjName> → (e)
    emit e once

P10: <Actuals> → a1, ..., ak
    save each actual left-to-right as $arg_1, ..., $arg_k
```

P23's sequence separates evaluation, the null guard, and dispatch. Actuals run
before the guard, following LO §3.4.3. All lookups of field offsets, slots, or
signatures are compile-time lookups; the descriptor and vtable loads happen at runtime.
The checker's `MethodResolution::Virtual` supplies the static class for slot lookup;
`Super` supplies the declaring class for a direct call. `Io` identifies a built-in
operation for the [I/O wrappers](#io), preserving receiver/argument evaluation and
the null check before calling the wrapper.

```text
P23 [ordinary call]: receiver.m(a1, ..., ak)
    save receiver as $receiver
    save actuals left-to-right as $arg_1, ..., $arg_k
    NULL_CHECK($receiver, m)
    $descriptor = load32($receiver + offset(Object.class_descriptor))
    $vtable = load32($descriptor + offset(ClassDescriptor.vtable))
    $table_index = load32($vtable + 4*vtable_slot(m))
    CALL_INDIRECT signature(m)($receiver, $arg_1, ..., $arg_k; $table_index) as $result
    local.get $result

P23 [super call]: super.m(a1, ..., ak)
    save this as $receiver
    save actuals left-to-right as $arg_1, ..., $arg_k
    NULL_CHECK($receiver, m)
    CALL resolved_ancestor_method(m)($receiver, $arg_1, ..., $arg_k) as $result
    local.get $result

NULL_CHECK($receiver, m)
    local.get $receiver
    i32.eqz
    if
        CALL lo_abort_null_receiver(address(method_name_bytes(m)), byte_length(m))
        unreachable
    end
```

Ordinary calls, including this.m, select the implementation from the object's
runtime descriptor. The slot is chosen from the receiver's static class. The
signature includes `$this` and every source parameter. For P19, omit `as $result`
and the final local.get. The original method name is retained for abort messages.

<a id="operators"></a>

P25 and boolean P26 use structured conditionals so only one branch executes.
A result-bearing if requires each normally completing branch to leave one i32;
the checker supplies its LO type for subsequent root tracking.

```text
P25: (condition ? then_value : else_value)
    emit condition
    if (result i32)
        emit then_value
    else
        emit else_value
    end

P26 [bool]: (left & right)
    emit left
    if (result i32)
        emit right
    else
        i32.const 0
    end

P26 [bool]: (left | right)
    emit left
    if (result i32)
        i32.const 1
    else
        emit right
    end
```

For the remaining integer operations, save left then right as `$left` and `$right`.
For +, -, *, <, >, and =, emit local.get for both values followed by the selected
instruction. This makes evaluation order explicit even when an operand contains a call.

| P26/P33 operator | Instruction |
|---|---|
| + | i32.add |
| - | i32.sub |
| * | i32.mul |
| < | i32.lt_s |
| > | i32.gt_s |
| = | i32.eq |

Addition, subtraction, and multiplication wrap. Division/remainder use the same
saved operands, but select these runtime cases with nested `if (result i32)` blocks:

| Operation | Condition, checked in this order | Value produced |
|---|---|---|
| / | $right == 0 | -1 |
| / | $left == INT_MIN and $right == -1 | INT_MIN |
| / | otherwise | local.get $left; local.get $right; i32.div_s |
| % | $right == 0 | $left |
| % | otherwise | local.get $left; local.get $right; i32.rem_s |

The guarded div_s must never execute in the first two cases. rem_s already returns
zero for INT_MIN % -1. These total arithmetic rules are part of LO semantics.

For String binary operations, save left then right and use CALL as follows. Each
CALL saves `$result`; load it to produce the expression value. Comparisons then
push zero and apply the indicated signed comparison.

| P26/P33 operand types and operator | Runtime call | After local.get $result |
|---|---|---|
| String + String | lo_string_concat($left, $right) | no further operation |
| String * int | lo_string_repeat($left, $right) | no further operation |
| String < String | lo_string_compare($left, $right) | i32.const 0; i32.lt_s |
| String > String | lo_string_compare($left, $right) | i32.const 0; i32.gt_s |
| String = String | lo_string_compare($left, $right) | i32.const 0; i32.eq |

The runtime rejects negative repetition counts. String comparisons use contents.
Class-reference equality, including comparisons with null, uses i32.eq on the
saved references. The latest test repo's [locked equality decision](https://github.com/Rahik-Sikder/lo-testing/blob/7851a5985495e885da6dc41195763e7608c7eab2/state-ledger.md#class-type-reference-equality--locked-2026-05-28)
clarifies this rule; the current checker still rejects those expressions. Class
references do not support ordering. Boolean equality is not a checked operator.

```text
P27/P34 [bool]: (! operand)
    emit operand
    i32.eqz

P27/P34 [int]: (~ operand)
    i32.const 0
    emit operand
    i32.sub

P27/P34 [String]: (~ operand)
    save operand as $string
    CALL lo_string_reverse($string) as $result
    local.get $result
```

Integer negation wraps. String reversal operates on Unicode code points in the
runtime, so the emitter does not manipulate UTF-8 bytes itself.

<a id="casts"></a>

The checker selects the cast direction. The runtime handles null and reports
failed downcasts; method-dispatch null guards do not apply to these operations.

```text
P29 [upcast/identity]: ((T) e)
    emit e

P29 [downcast]: ((T) e)
    save e as $object
    CALL lo_cast_check($object, address(descriptor(T))) as $result
    local.get $result

P30: (e instanceof T)
    save e as $object
    CALL lo_instanceof($object, address(descriptor(T))) as $result
    local.get $result
```

A null cast produces null, and null instanceof produces false. The checker treats
a literal-null cast as a no-check cast.

<a id="io"></a>

Synthetic I/O methods get normal functions and vtable entries, identified by
`IoOp`. Input's read_int/read_bool/read_string/eof call their corresponding lo_*
functions. Output's print_int/print_bool/print_string/println call the corresponding
runtime function with the receiver's destination tag.
Each wrapper uses ENTER, CALL, and LEAVE, with read results using the shared return slot.

The Output layout has a compiler-private i32 at offset 12: 0 for stdout,
1 for stderr. Both objects share one Output descriptor with instance size 16 and
no pointer fields. Print wrappers read this immutable tag from their receiver,
so aliases preserve the sink even when a pre-bound variable is reassigned.
The print-family ABI has a trailing `to_stderr: i32` argument. Each wrapper reads
the receiver's tag and passes it after the printed value, or as the sole argument
to `lo_println`. This handles `Output` values reached through fields, formals, and
aliases without selecting a different function at the call site.

```text
lo_entry: () → i32
    call lo_runtime_init()
    ENTER with persistent root slots for in/out/err
    for each pre-bound name g:
        store32(address(binding_cell(g)), $frame + 8 + 4*slot(g))
    construct and root the Input instance
    construct and root the stdout Output instance; initialize its sink tag to 0
    construct and root the stderr Output instance; initialize its sink tag to 1
    use P22 to construct Main() and save the result as $main
    use P23 to invoke $main.main() and save its result as $exit_code
    LEAVE
    local.get $exit_code
    end_function
```

Initialization precedes ENTER because it resets the frame chain. The I/O objects
exist before Main's constructor runs. Their persistent root slots start at zero
and remain registered for execution. `slot_address(g)` means
`load32(address(binding_cell(g)))`: read the static cell to find the root slot,
then read or assign the object reference in that slot. PUBLISH never overwrites
these canonical slots with a cached local or clears them as scratch storage.

The [type checker](../compiler/src/type_checker.rs) represents in/out/err with
`BindingInfo::Prebound` and permits assignments to them. Use the persistent root
slots for these bindings. `BindingInfo::Field` refers to object fields.

<a id="decisions"></a>

These requirements and choices determine the emitted ABI. Execution coverage and
remaining runtime dependencies are recorded in the [test report](wasm-test-results.md).

| Topic | Status and evidence |
|---|---|
| Every function gets a frame; every pointer field store gets a barrier | Required by P1 §2.4 and the authoritative ABI. Follow these despite LO §§7.2/7.5's suggested omissions. |
| Inline root slots and reload after GC | Required by ABI §3.3 and implemented by the supplied collector. |
| Push/pop do not allocate | Confirmed in the supplied source. Replacement runtimes must preserve the entering/exiting root lifetime. |
| Four-byte fields and named scratch locals | Emitter design choices, subject to review. |
| Per-object Output destination tag | Required to supply the print-family `to_stderr` selector through fields, formals, and aliases. |
| Typed AST and pre-bound binding representation | PR #5 supplies TypedProgram, ClassTable, and BindingInfo::Prebound. The emitter consumes their current representations. |
| Local String/cast stubs | ABI §4.4 says the grading runtime supplies implementations. Local execution of those paths still needs implementations. |

The rules follow supplied P1 §2.4, LO Appendix A.5 and dynamic semantics, and the
[runtime ABI](../runtime-abi.md). The [review guide](wasm-emitter-review-guide.md)
provides encoding references, focused checks, and a format for recording issues.
