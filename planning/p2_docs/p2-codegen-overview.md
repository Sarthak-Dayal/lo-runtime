# P2 native codegen: overview and decisions

Status: planning, 2026-10-09. Deadline: Mon 10/12 11:59pm Central (midterm the same day, in class).

Sub-docs (read after this one):
- [Instruction selection](p2-codegen-instruction-selection.md)
- [Frame and shadow stack](p2-codegen-frame-and-shadow-stack.md)
- [Register allocation integration](p2-codegen-regalloc-integration.md)
- [Lowering primer and evaluation](p2-lowering-primer.md)
- [Startup function (`lo_entry` and `main`)](p2-codegen-startup.md)
- [Bindings as a static shadow frame (proposal)](p2-bindings-static-frame.md)
- [Source structure refactor](p2-source-structure-refactor.md)

## 1. What exists and what is missing

| Piece | State |
|---|---|
| IR (`compiler/src/ir/mod.rs`) | Done. Plain three-address code, mutable (non-SSA) virtual registers, explicit blocks. |
| AST to IR lowering (`ir/lower.rs`) | Scalar code only. Calls, objects, casts, strings and fields return `Err`. A teammate and the user are finishing these. **This plan assumes they land.** |
| Liveness, linear scan (`ir/register_allocator/`) | Done, but targets Microsoft x64. Must become System V. |
| GC root insertion (`ir/gc_roots.rs`) | Done. Spills live `Ref`s to root slots around every `Call`. |
| Static data (`ir/statics.rs`, `layout.rs`, `symbols.rs`) | Done. Produces `DataDef`s for descriptors, vtables, names. |
| Native backend | **Missing.** `ProgramIr` + allocation to Intel-syntax `.s`, then `as` and `ld`. `main.rs` still calls `wasm::p1_program`. |

## 2. Sources of truth

- `p2-native-codegen.pdf` §2.1–2.5: one IR (TAC), selector must be "systematic and legible", linear scan, System V AMD64, static link against `liblo_runtime.a`, Intel syntax, no optimizations graded.
- `instruction_subset.md` (extracted from NotebookLM; stands in for the missing `architecture-reference.md`): the ~40-instruction subset, System V register roles, 16-byte alignment, RIP-relative addressing.
- `runtime-abi.md` §2–3, §4.2: object/descriptor layout, shadow stack, write barrier, abort exit codes, linkage.
- `planning/p2_docs/native-layout-reference-guide.md`: offsets (16-byte header, field `16+8*i`, shadow frame `16+8*n`), symbol names, runtime function table.

## 3. Decisions

| # | Decision | Where detailed |
|---|---|---|
| D1 | **No low-level IR.** Select directly from IR instructions into a typed `X86Inst` enum (restricted to the subset), then print. | instruction-selection |
| D2 | **Two seams** decouple selection from storage: a *locations* seam (`FunctionAllocation`/`PhysicalLocation`) and a *frame* seam (`FrameLayout`). Spill-everything is a trivial producer of the same allocation type the linear-scan allocator produces. | frame, regalloc |
| D3 | New code lives in a flat `compiler/src/codegen/` of new files only. One concept per file; type and impls together; no `use super::*`. | this doc §4 |
| D4 | Retarget `register_allocator/target.rs` from Microsoft x64 to System V AMD64. | regalloc |
| D6 | **Startup comes from lowering** (`lo_entry`, user decision 2026-10-09). Codegen registers its shadow frame after `lo_runtime_init` and emits a `main` wrapper. Supersedes the earlier "codegen builds startup" idea. | startup |
| D5 | Source-structure consolidation happens **after the `p2-final` tag**, as mechanical PRs. | refactor |

## 4. Architecture

```
CheckedIr --insert_gc_roots--> ProgramIr
ProgramIr --producer--> ProgramAllocation        (spill_everything now; allocate_registers later)
(ProgramIr, ProgramAllocation) --codegen--> .s text
.s --as--> .o --ld + liblo_runtime.a--> executable
```

Planned files in `compiler/src/codegen/`:

| File | Responsibility |
|---|---|
| `mod.rs` | `emit_program(&ProgramIr, &ProgramAllocation) -> String`; function loop |
| `x86.rs` | `Reg`, `Width`, `Operand`, `Mem`, `X86Inst` (subset only) |
| `print.rs` | `X86Inst` to Intel text. The only text-formatting code. |
| `frame.rs` | `FrameLayout`: offsets, prologue, epilogue, shadow frame |
| `select.rs` | One emitter per IR operator; the legible selector |
| `data.rs` | `DataDef` to `.rodata`/`.data` with relocations |
| `driver.rs` | `as` then `gcc -static` against `liblo_runtime.a`, `--emit-asm`, `--dump-ir` |

## 5. Schedule

| Day | Work |
|---|---|
| Fri 10/9 | `x86.rs`, `print.rs`, `frame.rs`, spill-all producer, scalar selector (Copy, Binary, Unary, Jump, Branch, Return). Golden-text tests per operator. |
| Sat 10/10 | Load/Store/Root*/Call/Abort, `data.rs`, `driver.rs`. Assemble, link, run in the amd64 container. Differential script (interpreter vs WASM vs native, including exit codes). |
| Sun 10/11 | System V retarget; wire linear scan; parallel-move handling; 5 contributed tests; design note. |
| Mon 10/12 | Buffer, README known-failures, tag `p2-final` (user commits, tags, pushes). |

Cut line if time runs short: ship spill-everything natively (handout §6 treats it as the de-risking step) and document linear scan as in progress. Do not ship a partially wired allocator.

## 6. Resolved questions (answers from the user's NotebookLM, 2026-10-09; ABI)

| Question | Answer | Consequence |
|---|---|---|
| Who emits `main`? | **Codegen** emits a `.globl main` wrapper around `ProgramIr`: `lo_runtime_init`; `lo_alloc(&Main_descriptor)`; run `Main`'s 0-arg constructor; call `Main.main`; `lo_pop_frame`; optional `lo_runtime_shutdown`; return the `int` in `eax`. | Add `main` synthesis to `codegen/mod.rs`. Lowering does not produce it. Symbol names come from `symbols.rs` (`lo_ctor_4_Main_0`, `lo_method_4_Main_4_main`). |
| Link command | `as -o prog.o prog.s` then `gcc -static -o prog prog.o liblo_runtime.a`. Requires libc and the crt objects; the C runtime provides `_start` and passes `main`'s `eax` to `exit`. | `driver.rs` shells out to `as` and `gcc -static`, not bare `ld`. `main` is entered with the standard C alignment. |
| Division | LO division is **total**: `x/0 = -1`, `x%0 = x`, `INT_MIN/-1 = INT_MIN`, `INT_MIN%-1 = 0`. Hardware `idiv` faults on those inputs, so they must be guarded. | **Already handled in lowering** (`lower.rs::division` / `division_special`, checked 2026-10-09): constant 0 and -1 divisors become `Copy`/`Unary Neg`, and non-constant divisors get a branch diamond. The selector therefore emits a plain `cdq ; idiv` and may assume the divisor is never 0 or -1. Add a golden test that mirrors the four rules. |
| Shadow frame on zero roots | Any function with at least one call **must** register a frame, even with `num_roots == 0`. Leaf functions (no calls) **may skip** it. | Rule: register iff the function contains a `Call`. See frame doc §6. |
| `ShadowFrame.parent` | Filled in by the runtime in `lo_push_frame` (ABI §3.3). | Codegen stores null. |

## 6b. Startup, frames, abort (resolved with the user, 2026-10-09)

1. **Startup runs before `Main`'s constructor.** Bindings (`in`/`out`/`err`) are published before `Main` is allocated and constructed; the exact order among those setup steps does not matter as long as all of it precedes the constructor call.
2. **Startup comes from lowering (revised 2026-10-09).** The lowering worktree builds `lo_entry` (`ProgramIr.startup`) with the full sequence and tests; codegen only frames it and emits the `main` wrapper. The one rule: register the shadow frame *after* `lo_runtime_init`, as P1 does (`wasm/function.rs`, `finish`). Details and diagrams: [startup doc](p2-codegen-startup.md); what lowering emits: [primer](p2-lowering-primer.md). The earlier blockers (constructor bodies, I/O wrappers) are resolved by that worktree once merged.
3. **Abort-only functions are leaves.** No frame. Confirmed against the language reference.
4. **16-byte alignment at abort calls (from the language reference).** `lo_abort_*` runs libc `fprintf`, which can fault if `rsp` is misaligned at the `call`. A leaf function that only aborts still needs the normal `push rbp ; mov rbp, rsp ; sub rsp, N` with `N` keeping `rsp` 16-aligned. In `FrameLayout`, a frame-less leaf is not stack-less: it still gets the aligned frame, just no shadow frame or `ret_save`. Add a test with an abort-only function.
5. **Where runtime functions are declared.** `compiler/src/ir/runtime.rs` (branch `feat/native-layout`) has the typed `FUNCTIONS` table, including `lo_runtime_init`, `lo_push_frame (Ptr)`, `lo_pop_frame ()`, `lo_alloc`, `lo_gc_write_barrier`, and the abort helper. It only *declares* external symbols and signatures. The *calls* are emitted by codegen in the prologue/epilogue (P1 does the same by hand in `wasm/function.rs` and `wasm/module.rs`). Codegen should obtain these symbols through `runtime_function(name)` rather than hard-coding strings.

## 6c. Branch state (checked 2026-10-09, read-only)

Current branch `feat/p2-codegen` is at `7366fcc`. Three lines of work have not met:

| Line | Contains | Not in your branch |
|---|---|---|
| `feat/p2-codegen` (HEAD) | lowering, GC roots, linear scan | `layout.rs`, `symbols.rs`, `ir/runtime.rs`, `ir/statics.rs`, `ir/declare.rs` |
| `feat/native-layout` = `origin/feat/ir-foundation` (`00d07f3`, PR #17), branched from `063d227` | the five files above (class layout, descriptors, vtables, runtime table) | lowering, GC roots, linear scan |
| `origin/main` (`2ed64c8`, PR #14) | IR foundation merge, type-checker refactor, "magic numbers" cleanup, small `wasm/` changes (new `wasm/abi.rs`) | everything above |

**Correction to the plan assumption:** `2ed64c8` does *not* contain the class-layout work. That is in PR #17 (`00d07f3` / `feat/native-layout`), which is not yet on `origin/main`. `2ed64c8` only brings in an earlier IR-foundation state that your branch already has (via `defea0c`), plus the `wasm/` and type-checker changes.

Dry-run merges (`git merge-tree`, nothing changed):
- `HEAD` + `feat/native-layout`: **one trivial conflict** in `compiler/src/ir/mod.rs`. Both sides added adjacent `mod` lines (yours: `gc_roots`, `lower`, `register_allocator`, `lower_tests`; theirs: `declare`, `runtime`, `statics`). Keep all of them. Also take their `Clone/Debug/PartialEq` derives on `Section` and `DataItem`; yours adds `#[derive(Debug)]` on `InstructionKind`.
- `HEAD` + `origin/main`: clean.

**Update:** at the user's request, tracked changes were stashed (`pre-main-merge: planning/ deletions`) and `origin/main` was merged (`0f02751`; 203 tests pass). PR #17 then landed on `main` (`9094026`) and was merged too (`f28629d`; one trivial `ir/mod.rs` conflict, both `mod` lists kept; 211 tests pass). `layout.rs`, `symbols.rs`, `ir/runtime.rs`, `ir/statics.rs` and `ir/declare.rs` are now in the branch. The commands below are kept for reference only:

```sh
git fetch
git merge origin/feat/ir-foundation     # brings in layout/statics/runtime; resolve ir/mod.rs by keeping both mod lists
git merge origin/main                   # clean
cargo test --manifest-path compiler/Cargo.toml
```

Merge `feat/ir-foundation` first so the layout code is there before codegen starts. Tell me if you want me to run the merges instead.

## 7. Process

Per the repo CLAUDE.md files: no commits, branches or pushes from Claude. Changes stay in the working tree for the user to review.
