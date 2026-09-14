# Reviewing and learning the WASM emitter

This companion to the [emission rules](wasm-emitter-design.md) follows one example
through the representations involved in compilation. Its stage names E1–E5 are
review locations, not LO productions. Use a production number for a language-rule
question, a helper name for shared mechanics, and a stage name for encoding/tooling.
The [compiler tests](../compiler/README.md) now run LO source through the real
frontend, runtime linking, and WASM execution. The [production index](wasm-production-index.md)
locates each handler; the [test report](wasm-test-results.md) distinguishes emitter
verification from frontend/runtime failures. The standalone byte exercise below
explains the encoding one byte at a time.

To locate a problem, find the first representation that disagrees with the language
rule. Keep the example small enough to predict its stack values, references, and
result before running a tool.

```text
LO source → checked AST → symbolic emission → LLVM assembly (.s)
          → relocatable WASM object (.o) → linked module (.wasm) → execution
```

| Start here | What to understand |
|---|---|
| [E1: values and indices](#e1) | What each instruction consumes, and what a number names |
| [E2: assembly](#e2) | How the readable reductions become LLVM assembly |
| [E3: binary encoding](#e3) | Opcodes, integer encodings, sections, and function bodies |
| [E4: object files and linking](#e4) | Symbols, relocations, and the shared runtime |
| [E5: testing and issue attribution](#e5) | Which artifact to inspect and how to record an issue |

<a id="e1"></a>

E1 starts with the distinction between a local and the operand stack. A local is
a saved value; local.get copies it onto the operand stack, and local.set consumes
the top stack value to update a local. An arithmetic instruction consumes its
operands and pushes its result. Consider this LO method excerpt:

```text
int add(int left, int right) {
    return (left + right);
}
```

P26 emits the addition; P13 saves its value and branches to the shared method exit.
The generated function has parameters `$this`, `$left`, and `$right`. Focusing on
just the arithmetic instructions after operand evaluation:

| Instruction | Operand stack afterward, with the top at the right |
|---|---|
| local.get $left | [left] |
| local.get $right | [left, right] |
| i32.add | [left + right] |
| local.set $return_value | [] |

This explains why source assignment and return do not leave stray values on the
stack. Operand order matters: with [left, right] on the stack, i32.sub produces
left minus right. The [WASM execution rules](https://webassembly.github.io/spec/core/exec/instructions.html)
define these effects.

A name such as `$this` improves the reduction's readability. For this LO method,
the parameter indices are this = 0, left = 1, right = 2; additional locals follow
the parameters. Named WAT identifiers are resolved to indices during text parsing.
Our LLVM assembly writer performs its own local-name mapping. See the
[WAT identifier rules](https://webassembly.github.io/spec/core/text/values.html#text-id).

Several unrelated things are represented as integers. Keeping their meanings
separate prevents errors that an i32 type check cannot detect:

| Quantity | What it selects or measures |
|---|---|
| Local index | A parameter/local within one function |
| Branch depth | An enclosing control construct, counting outward from zero |
| Function index | A function in the module's function index space; used by direct call |
| Type index | A signature in the module's type section |
| Vtable slot | A method's position within one class's vtable in linear memory |
| Function-table entry index | A function reference in the WASM table; loaded from a vtable entry for call_indirect |
| Memory address or field offset | A byte location in linear memory or relative to an object |
| Root slot number | A reference's position in a particular shadow frame |

Imported functions precede definitions in the function index space. Adding a
runtime import can shift function indices, while `$this` still has local index 0
inside each generated method.

In P23, `$table_index` means a function-table entry index. The call_indirect
instruction also has a type-index immediate and a table-number immediate; the
entry to call is supplied separately on the operand stack. Vtables are arrays of
i32 entries in linear memory; the WASM function table itself is a distinct engine
structure. The [module index rules](https://webassembly.github.io/spec/core/syntax/modules.html)
describe these separate index spaces.

Branch depths depend on the nesting at the branch site. In this example, the
branch exits the outer block:

```text
block                 ← depth 2 from the branch
    loop              ← depth 1
        if            ← depth 0
            br 2
```

A branch back to the loop after the if has ended would use `br 0`. The names in
the reductions identify the intended targets; emitted branches use numeric depths.
Count every enclosing WASM block, loop, and if at the branch site. Targeting a
loop restarts it; targeting its surrounding block exits it. This is the E1 check
for P15/P16.

<a id="e2"></a>

E2 converts the symbolic reductions into assembler syntax. Runtime operations
become instructions, compiler lookups become constants/symbols, and helpers such
as CALL expand into their specified instruction sequences. Instructions like
PUBLISH or load32 must be expanded before invoking an assembler.

This standalone addition example has just two integer parameters. It isolates
WASM arithmetic and encoding; a complete LO method additionally needs its receiver,
shadow frame, and shared return sequence. Its LLVM assembly is:

```asm
.text
.globl add
.type add,@function
add:
    .functype add (i32, i32) -> (i32)
    local.get 0
    local.get 1
    i32.add
    end_function
```

The writer supplies the signature, visibility, and chosen instructions. llvm-mc
encodes them. LLVM's [WASM assembler tests](https://github.com/llvm/llvm-project/blob/main/llvm/test/MC/WebAssembly/basic-assembly.s)
show the actual syntax, including .local declarations and end_function. WAT and
LLVM assembly are different textual formats; use the .s file with llvm-mc.

The file produced by LLVM may contain extra sections and different encodings of
relocatable operands. Compare instructions, signatures, and behavior rather than
expecting identical bytes.

<a id="e3"></a>

E3 explains bytes independently of the assembler. LLVM remains responsible for
binary encoding in the compiler; this small standalone example makes the fields visible.

First, its instruction sequence and function-body prefix are:

```text
00             no additional local-declaration groups (parameters are in the type)
20 00          local.get 0
20 01          local.get 1
6a             i32.add
0b             end
```

Instructions use opcodes followed by immediate operands where needed. Here 20 is
the opcode and the following 00 or 01 is a local index; 6a has no immediate operand.
The body occupies seven bytes including its initial local-declaration count.
See the [instruction encodings](https://webassembly.github.io/spec/core/binary/instructions.html).

The complete 41-byte module is:

```text
00 61 73 6d 01 00 00 00
01 07 01 60 02 7f 7f 01 7f
03 02 01 00
07 07 01 03 61 64 64 00 00
0a 09 01 07 00 20 00 20 01 6a 0b
```

| Line | Interpretation |
|---|---|
| Header | WASM magic bytes followed by binary format version 1 |
| Type | Section 1, payload length 7; one function type: two i32 parameters, one i32 result |
| Function | Section 3, payload length 2; one defined function using type index 0 |
| Export | Section 7, payload length 7; one export named add (61 64 64), kind function, index 0 |
| Code | Section 10, payload length 9; one body of length 7, containing the sequence above |

Within the type payload, 60 marks a function type and 7f denotes i32. The 02 counts
parameters and the later 01 counts results. These are [type encodings](https://webassembly.github.io/spec/core/binary/types.html).
Decode each field in context: 7f as a type tag means i32, while 7f as a signed
integer immediate means -1. Searching for opcode bytes without respecting field
boundaries can mistake an immediate, a name, or data for an instruction.

Section lengths count payload bytes; list counts count entries. Function declarations
and code bodies are separate: the function section assigns types, and the code
section supplies bodies in corresponding order. These fields follow the
[binary module format](https://webassembly.github.io/spec/core/binary/modules.html).

Larger integers require more than one byte. WASM uses LEB128 for many integer
fields: seven payload bits per byte and a continuation bit. Counts and indices
use unsigned LEB128; i32.const uses signed LEB128. For example:

| Encoded value | Bytes |
|---|---|
| Unsigned index 128 | 80 01 |
| Signed constant 65 | c1 00 |
| Signed constant -1 | 7f |

The signed final byte carries sign information, which is why constant 65 needs
another byte. Encoding an immediate as four fixed bytes would corrupt subsequent
instructions. The [integer encoding rules](https://webassembly.github.io/spec/core/binary/values.html#integers)
cover bounds and permitted padding. Fixed-width data in linear memory is different:
our four-byte fields use [little-endian layout](https://webassembly.github.io/spec/core/exec/numerics.html#byte-conversion).
That layout is relevant to objects
and descriptors, while LEB128 is relevant to their encoded instruction operands.

Changing `6a` (add) to `6b` (subtract) keeps the module structurally valid but
changes its behavior. Changing the code payload length `09` to `08` instead makes
the module invalid. These are different failure classes even though each changes
one byte.

<a id="e4"></a>

E4 is where the course's runtime linkage matters. program.o contains symbols and
relocation records in addition to WASM code/data. A relocation identifies a field
that needs a symbol-derived value once linking determines its final placement.
The assembler emits those records from symbolic references; the linker resolves them.
They use custom sections such as linking, reloc.CODE, and reloc.DATA alongside
the core type, code, and data sections.

| Emitted reference | Typical wasm32 relocation |
|---|---|
| call lo_alloc | R_WASM_FUNCTION_INDEX_LEB |
| i32.const LO_EMPTY_STRING | R_WASM_MEMORY_ADDR_SLEB |
| Descriptor address in four-byte static data | R_WASM_MEMORY_ADDR_I32 |
| Method reference in a four-byte vtable entry | R_WASM_TABLE_INDEX_I32 |

A data address and a function-table entry therefore require different relocations,
even though both occupy four-byte vtable/descriptor fields. Instruction relocations
can use padded LEB operands, so a relocatable call need not use the shortest byte
sequence. The [WASM linking convention](https://github.com/WebAssembly/tool-conventions/blob/main/Linking.md)
specifies the linking metadata and relocation kinds.

For LO, the emitter must provide correct function signatures, data layouts, symbols,
and runtime references. The linked module must share one linear memory with the
runtime, export memory and lo_entry, and contain no start section. The host imports
must match the supplied harness. A misspelled lo_* symbol must not accidentally
survive as a new import merely because a link command allows undefined symbols.

Keep program.s, program.o, and program.wasm so each step can be inspected. Use:

```sh
llvm-readobj --sections --symbols --relocations program.o
wasm-tools validate program.wasm
wasm-tools print program.wasm
```

[llvm-readobj](https://llvm.org/docs/CommandGuide/llvm-readobj.html) exposes symbols
and relocations; [wasm-tools](https://github.com/bytecodealliance/wasm-tools) validates
and prints the linked module. Check expected symbols at .o, and resolved imports,
exports, signatures, and calls at .wasm. The [WASM linker documentation](https://lld.llvm.org/WebAssembly.html)
explains linking flags and undefined-symbol behavior. These tools still need to be
verified in the course image.

<a id="e5"></a>

E5 describes how to investigate a question or failure. Name the rule and expected
behavior, then find the first artifact that disagrees. WASM validation checks
structure and types; executing the program checks its behavior for that input.

The [compiler README](../compiler/README.md) lists test commands. The conformance
runner saves each pipeline stage; the [test report](wasm-test-results.md) records
coverage, failures, and verification limits.

| Observation | First places to inspect | Check that distinguishes the causes |
|---|---|---|
| llvm-mc rejects the .s | E2 | Did symbolic helpers leak through? Are directives and operand spellings valid LLVM assembly? |
| wasm-ld cannot resolve a runtime symbol | E4 | Does the WASM archive define it with the expected signature? Is the symbol spelling correct? |
| A branch/result fails validation | P7, P13–P16, P25; E1/E3 | Trace stack height/type through every branch and check the target depth. |
| call_indirect traps | P4/P23; E1/E4 | Check the object guard, loaded vtable entry, function-table contents, and call signature. |
| Valid module returns the wrong result | The relevant LO production | Compare source semantics, typed operands, and chosen instructions before examining byte encodings. |
| Failure appears only after collection | PUBLISH, RELOAD, CALL, P4 layout | Record root-slot addresses and references before/after GC; inspect inherited pointer offsets. |
| err traps | Synthetic I/O and its runtime dependency | Check whether the print-destination ABI update has landed in both runtime and host; the current wrapper explicitly defers this path. |

These checks help narrow down the cause. An incorrect field offset can look like
a GC failure, and an incorrect table index can look like a signature failure.
Keep the smallest reproducer and inspect the actual values.

For learning, add one feature at a time:

| Exercise | Predict before running | Review locations |
|---|---|---|
| Integer addition and return | Operand stack and local indices | P13, P26, E1–E3 |
| Ternary and nested loop/break | Which branch executes; each branch's depth/result | P14–P16, P25 |
| Null receiver with an argument that prints | Argument output occurs before the null-dispatch abort | P10, P23, NULL_CHECK |
| Child override and super call | Virtual call selects child; super selects the resolved ancestor | P4, P23, E4 |
| Earlier reference argument followed by allocation | Every saved reference survives a real collection | P22, PUBLISH, RELOAD, CALL |
| String and cast operations | Defaults, null casts, contents comparison, abort behavior | P22, P26–P30, runtime |

The emitter now has handlers for these exercises. String operations, downcasts,
and instanceof still need their runtime implementations for execution checks.
Use allocation pressure or the runtime's test-only forced-collection hook to test
GC; several allocations do not necessarily cause collection. Compare against the
interpreter when available, while checking the language reference independently:
shared frontend mistakes can make both execution paths agree on the wrong behavior.

A PR comment can use this compact format without asserting a bug prematurely:

```text
Location: P23 / NULL_CHECK, or CALL / RELOAD, or E3 / body length
Kind: question, missing case, suspected contradiction, or observed failure
Small example: the LO construct or exact assembly/bytes involved
Expected: result, order, stack shape, or memory update; cite the relevant contract
Unclear/observed: the step I cannot justify or the result I actually see
Evidence: typed node, instruction trace, object dump, or root-slot trace
```

For example, an issue about P23 should distinguish “why do actuals run before the
null check?” from “the generated code checks null before evaluating actuals.” The
first asks for the language rationale; the second can be demonstrated by a small
program and an instruction trace. State which kind of issue you are reporting
so the reviewer knows what evidence or explanation is needed.
