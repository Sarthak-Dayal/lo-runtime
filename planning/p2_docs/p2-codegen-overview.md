# P2 native codegen: overview and decisions

Deadline: Mon 10/12 11:59pm Central (midterm the same day, in class).

Sub-docs:
- [Hello-world walkthrough](p2-hello-world-walkthrough.md): the IR and both assembly outputs for one small program, annotated. Start here to see what the pieces produce.
- [Instruction selection](p2-codegen-instruction-selection.md)
- [Frame and shadow stack](p2-codegen-frame-and-shadow-stack.md)
- [Register allocation](p2-codegen-regalloc-integration.md)
- [Startup function (`lo_entry` and `main`)](p2-codegen-startup.md)
- [Lowering primer](p2-lowering-primer.md)
- [Bindings as a static shadow frame](p2-bindings-static-frame.md)
- [Source structure refactor](p2-source-structure-refactor.md) (post-tag plan)

## 1. Pipeline

```
source
  | lex, parse, type check
  v
TypedProgram + ClassTable
  | lower_program                       one IR function per method/constructor, plus lo_entry
  v
CheckedIr
  | insert_gc_roots                     shadow-stack saves/reloads around every call
  v
ProgramIr
  | allocation                          linear scan (default) or spill-everything (--spill-all)
  v
ProgramAllocation                       virtual register -> physical register or spill slot
  | codegen::emit_program               frame, selection, data, main wrapper
  v
Intel-syntax x86-64 assembly (System V AMD64)
  | as, then gcc -static against liblo_runtime.a
  v
executable
```

## 2. State

| Piece | State |
|---|---|
| IR, verifier, dump (`ir/`) | Done. Three-address code, mutable (non-SSA) virtual registers, explicit blocks. |
| AST to IR lowering (`ir/lower*`) | Done, including objects, dispatch, strings, casts, constructors, I/O wrappers and `lo_entry`. |
| GC root insertion (`ir/gc_roots.rs`) | Done. |
| Linear scan, System V register file (`ir/register_allocator/`) | Done, and the default. |
| Spill-everything (`register_allocator/spill_all.rs`) | Done. The reference for debugging. |
| Native back end (`codegen/`) | Done: frames, selection, data, `main` wrapper, parallel parameter moves, driver and CLI. |

## 3. Sources of truth

- `p2-native-codegen.pdf` §2.1–2.5: one IR (TAC), a selector that is "systematic and legible", linear scan, System V AMD64, static link against `liblo_runtime.a`, Intel syntax, no optimizations graded.
- `instruction_subset.md` (from the course reference): the ~40-instruction subset, System V register roles, 16-byte alignment, RIP-relative addressing.
- `runtime-abi.md` §2–3, §4.2: object and descriptor layout, shadow stack, write barrier, abort exit codes, linkage. (§4.4 still lists the string and cast functions as stubs; the Rust runtime implements them.)
- `planning/p2_docs/native-layout-reference-guide.md`: offsets (16-byte header, field `16+8*i`, shadow frame `16+8*n`), symbol names, runtime function table.

## 4. Decisions

| # | Decision | Detail |
|---|---|---|
| D1 | **No low-level IR.** Select directly from IR instructions into a typed `Inst` enum restricted to the subset, then print. | instruction selection |
| D2 | **Two seams** keep selection independent of storage: the *locations* seam (`FunctionAllocation` / `PhysicalLocation`) and the *frame* seam (`Frame`). Spill-everything and linear scan produce the same allocation type, so the selector cannot tell them apart. | frame, register allocation |
| D3 | New code lives in a flat `compiler/src/codegen/`. One concept per file; a type and its impls together; explicit imports. | §5 |
| D4 | **Startup comes from lowering** (`lo_entry`). Codegen registers its shadow frame after `lo_runtime_init` and emits a `main` wrapper. | startup |
| D5 | **Linear scan is the default**; `--spill-all` stays as the reference. Any divergence between the two points at the allocator or its integration, not at selection. | register allocation |
| D6 | Source-structure consolidation happens after the project works, as mechanical PRs. | refactor |

## 5. Files in `compiler/src/codegen/`

| File | Responsibility |
|---|---|
| `mod.rs` | `emit_program(&ProgramIr, &ProgramAllocation) -> String`; the `main` wrapper |
| `x86.rs` | `Reg`, `Width`, `Operand`, `Mem`, `Inst` (subset only), `Inst::check` |
| `print.rs` | `Inst` to Intel text. The only text-formatting code. |
| `frame.rs` | `Frame`: offsets, prologue, epilogue, shadow frame, parameter moves |
| `moves.rs` | Parallel register moves (cycle breaking through `r11`) |
| `select.rs` | One emitter per IR operator; the legible selector |
| `data.rs` | `DataDef` to `.rodata` / `.data` directives |
| `driver.rs` | The pipeline, `as` then `gcc -static`, and the CLI modes |

## 6. Rules the back end relies on

| Topic | Rule |
|---|---|
| `main` | Codegen emits `.globl main`, which calls `lo_entry`; `gcc -static` supplies `_start` and passes `main`'s `eax` to `exit`, so `Main.main`'s result is the exit status. |
| Link line | `as -o prog.o prog.s`, then `gcc -static -o prog prog.o liblo_runtime.a -lm -lpthread -ldl`. |
| Division | LO division is total: `x/0 = -1`, `x%0 = x`, `INT_MIN/-1 = INT_MIN`, `INT_MIN%-1 = 0`. Lowering routes divisors 0 and -1 around `idiv`, so the selector emits a plain `cdq ; idiv`. |
| Shadow frame | A function that contains a `Call` registers a frame (even with zero roots); a leaf does not. An abort-only function is a leaf, but still keeps an aligned frame because abort helpers call libc. The runtime fills in `ShadowFrame.parent`; codegen stores null. |
| Startup frame | `lo_runtime_init` resets the runtime's frame chain, so `lo_entry`'s frame is registered right after that call, then the static `lo_bindings` frame is pushed. |
| Runtime functions | Declared in `ir/runtime.rs`; codegen emits the `lo_push_frame` / `lo_pop_frame` calls itself in prologues and epilogues. |

## 7. Not done

- Source-structure consolidation, including de-duplicating the layout and static-data code that exists both in lowering and in `layout.rs` / `ir/statics.rs`.
- The design note, and a graph-coloring comparison.
- Interval splitting and call-site save/restore in the allocator (the conservative call policy stands in for both).
- Zig and C++ runtimes (only the Rust skeleton has been linked against).
