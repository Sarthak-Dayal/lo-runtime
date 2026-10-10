# What the new AST-to-IR lowering produces (primer and evaluation)

Source: the `lo-runtime-wt` worktree (branch `feat/ast-to-ir-lowering`, uncommitted as of 2026-10-09), `planning/ast-to-ir-lowering-plan.md`, and the code in `compiler/src/ir/lower.rs`, `ir/lower/{layout,program,runtime}.rs`. I read the code and the diffs; the worktree's own 192 tests pass. I did not run it against this branch. When it lands here, re-check the file references below.

## 1. The pipeline, end to end

```
 source text
     |  lexer, parser, type checker (existing, unchanged)
     v
 TypedProgram + ClassTable
     |  lower_program(program, classes, TargetLayout)          <-- NEW (this PR)
     v
 CheckedIr  (verified ProgramIr: functions, symbols, data, startup = lo_entry)
     |  insert_gc_roots          (feat/gc-roots, already here)
     v
 ProgramIr  (+ RootStore/RootLoad around every Call)
     |  allocate_registers       (linear scan; spill-everything first)
     v
 ProgramAllocation
     |  codegen::emit_program    <-- what we are building
     v
 .s text  --as-->  .o  --gcc -static + liblo_runtime.a-->  executable
```

`lower_program` is not called from `main.rs` yet; today only tests call it. The WASM back end still reads the AST directly and never touches this IR, so nothing in lowering is shaped by WASM.

## 2. What one `lower_program` call returns

```
ProgramIr
 |
 |-- symbols[]       every name the program or the runtime uses
 |     lo_class_6_Circle          Data       class descriptor
 |     lo_class_6_Circle_pointers Data       GC pointer-offset array
 |     lo_class_6_Circle_vtable   Data       function-address array
 |     lo_method_6_Circle_4_area  Function   (Ref) -> Int32
 |     lo_ctor_6_Circle_1         Function   (Ref, Int32) -> void
 |     lo_bytes_0 ...             Data       interned string / class-name bytes
 |     lo_binding_in/out/err      Data       writable pointer-sized words
 |     LO_EMPTY_STRING            StaticRef  provided by the runtime
 |     lo_alloc, lo_string_*, ... Function   runtime entry points (declared, not defined)
 |     lo_entry                   Function   () -> Int32   == ProgramIr.startup
 |
 |-- signatures[]    interned function types
 |-- data[]          one DataDef per Data symbol defined here (descriptors, vtables, bytes, bindings)
 `-- functions[]     one FunctionIr per constructor, method, and lo_entry
```

Every body is lowered with `root_slots = 0` (except `lo_entry`, which has 3). Rooting comes later from `insert_gc_roots`.

## 3. Memory layout it bakes into the IR (x86-64)

Offsets are constants inside `Load`/`Store` instructions and inside descriptor data, so codegen never recomputes them.

```
Object                                    Descriptor (56 bytes, read-only)
  +0   class_descriptor  (8) ----------->   +0   name            (ptr) -> lo_bytes_N ("Circle\0")
  +8   gc_bits           (4)                +8   name_len        (u32)
  +12  flags             (4)                +16  parent          (ptr) -> parent descriptor / 0
  +16  first field ...                      +24  instance_size   (u32)
                                            +32  pointer_offsets (ptr) -> lo_..._pointers
                                            +40  pointer_count   (u32)
                                            +44  vtable_size     (u32)
                                            +48  vtable          (ptr) -> lo_..._vtable
```

**Decision (user, 2026-10-09): use the `layout.rs` rule.** Every field takes one pointer-sized slot, in `effective_fields` order (inherited fields first, as a prefix): `offset(i) = header + ptr * i`, `instance_size = header + ptr * (fields + 1 if Output)`. The *access* width is still set by the IR type (`Store.ty`, `Load.dst`): `dword` for Int32/Bool, `qword` for references, so the upper half of an Int/Bool slot is just unused. The first draft of the worktree packed 4-byte fields instead; it is being changed to match.

```
class Shape ( int id; )                       Shape   size 24
class Circle extends Shape ( String name; )      +16 id (slot, 4 used)
                                              Circle  size 32
class A ( int a; int b; )                        +16 id  +24 name
   -> a at +16, b at +24, size 32
```

Packing would not have broken anything on x86-64 (unaligned access is legal, `lo_alloc` zero-fills whole instances, the GC only reads `instance_size` and `pointer_offsets`). It was changed to keep **one layout** shared by lowering, `layout.rs` and `statics.rs`, to match the P1 convention of one word per field, and to keep offsets trivial to reason about (`16 + 8*i`).

`Output` additionally reserves a 4-byte stream selector right after the header (+16): 0 for `out`, 1 for `err`.

## 4. Virtual dispatch

```
receiver (Ref)
   |  Load  [receiver + 0]          -> descriptor   (Ptr)
   |  Load  [descriptor + 48]       -> vtable       (Ptr)
   |  Load  [vtable + 8 * slot]     -> code pointer (CodePtr(signature))
   v
 Call Indirect(code pointer)(receiver, args...)
```

Before the first `Load`, the lowering emits a null check: `Eq receiver, Null`, branch to an `Abort` block calling `lo_abort_null_receiver(name_bytes, len)`. Arguments are evaluated before that check (matches the WASM back end). `super.m()` calls use `Call Direct(lo_method_<owner>_...)` instead of the vtable.

## 5. Prebound `in` / `out` / `err`: the static `lo_bindings` frame

LO lets a program assign to `out` (`out = someOutput;`). The collector can move objects and only knows about **shadow-stack root slots**, not bare globals. So the three bindings are the roots of one writable static `ShadowFrame`, registered once at startup:

```
 .data (writable)                     frame chain (runtime's current_frame)
 +----------------------------+
 | lo_bindings  (ShadowFrame) |<---- lo_push_frame(lo_bindings) in lo_entry
 |  parent                    |----> lo_entry's stack frame ----> (runtime start: null)
 |  num_roots = 3             |
 |  roots[0]  in   ---------------> Input object   (heap)
 |  roots[1]  out  ---------------> Output object  (heap, selector = 0)
 |  roots[2]  err  ---------------> Output object  (heap, selector = 1)
 +----------------------------+     the collector rewrites roots[k] when it moves an object
```

- **Read** `out`: `Load Ref [Symbol lo_bindings + 24]`.
- **Assign** `out = x`: `Store Ref [Symbol lo_bindings + 24], x`. The verifier exempts `Ref` stores whose base is a `Data` symbol from the write-barrier rule: a static root slot is scanned on every collection, exactly like a stack slot, so no barrier is needed. A `Ptr`-typed register base is **not** exempt.
- The loaded value is an ordinary `Ref`; `insert_gc_roots` roots it across later calls like any other.

This replaced an earlier version in which each global held the address of a slot in `lo_entry`'s frame and writes needed a special `RootStoreIndirect` instruction (and `RootAddr` to publish the address). Design record: [p2-bindings-static-frame.md](p2-bindings-static-frame.md).

## 6. Consequences for the backend

| Lowering fact | What codegen does |
|---|---|
| Offsets are fixed in `Load`/`Store`; field widths 4 (Int32, Bool) or 8 (Ref, Ptr, CodePtr) | Width of a memory access comes from the `Load` destination type or the `Store.ty`: `dword` or `qword`. |
| `Binary Eq` is used for Int, Bool and Ref, including `Eq x, Null` | The selector must know operand types to pick `cmp eax` vs `cmp rax`. `ProgramIr::operand_type` exists in `verify.rs` but is private; make it `pub(crate)` or add a helper in `ir/`. |
| `Store Ref [Symbol lo_bindings + k]` | Generic `Store` with a `Symbol` base: `lea r10, [rip + lo_bindings]` then the move. No barrier. |
| `lo_gc_write_barrier(obj, offset, value)` performs the store (ABI §3.4) | Treat as an ordinary call. Do not also emit a store. |
| `Terminator::Abort` for null dispatch | Aligned call, nothing after it. |
| Bool encoding at runtime calls is left to the backend | Pass 0/1 in the 32-bit register; after a call returning Bool, `movzx eax, al`. |
| `lo_entry` contains `Call lo_runtime_init` first | Register the shadow frame **after** that call; see the startup doc. |
| Tables with zero entries get an addressable sentinel (`Zero(4)` or `Zero(ptr)`), not a null pointer | `data.rs` emits them like any other `DataDef`. |

## 7. Evaluation

What I checked and found consistent with our design:
- **No WASM spillover.** `TargetLayout::{Wasm32, X86_64}` is only a pointer-width switch. The only places that pass `Wasm32` are tests (`lower_tests.rs`, `program_tests.rs`); nothing in `main.rs` calls lowering. For native we pass `X86_64`, whose numbers (header 16, descriptor 56, vtable at 48, selector at 16) match the ABI and `layout.rs`.
- Layout, descriptors, vtable slot arithmetic and the null-guard ordering are self-consistent. The descriptor offsets are `0/8/16/24/32/40/44/48`.
- Heap `Ref` writes go through `lo_gc_write_barrier`, except stores of `LO_EMPTY_STRING` (a `StaticRef`) which are exempt in the verifier. This matches the ABI.
- The GC handoff is consistent with `gc_roots.rs`: it assigns slots from `root_slots`, so `lo_entry`'s three slots stay reserved.

Points for you to decide or propose (none blocks codegen):

| # | Item | Suggestion |
|---|---|---|
| 1 | Duplicate infrastructure: lowering has its own layout, descriptor/static-data builder, runtime table and symbol declarations, parallel to `layout.rs`, `ir/statics.rs`, `ir/runtime.rs`, `ir/declare.rs` from PR #17. | **Field layout: decided** (§3), lowering switches to the `layout.rs` rule. The remaining duplication is deferred: merge now, deduplicate after the tag (PR #17 reviewers removed `Wasm32`, so the survivor should be x86-64 only). |
| 2 | `lower_program` still takes `TargetLayout` with a `Wasm32` variant. | Leave for now. If you want it gone: replace the parameter with a constant and drop the `Wasm32` branches in `program_tests.rs`. |
| 3 | `operand_type` is private to the verifier. | **Done** on `feat/p2-codegen` (`pub(crate)` in `ir/verify.rs`). |
| 4 | `this`/`super` receivers get a null check that can never fire. | **Deferred (user, 2026-10-09):** a small optimization for later; skip the guard when the receiver is `this`. |
| 5 | Every local read emits a `Copy` snapshot. | Fine for correctness. Doubles virtual registers, which matters only for spill-everything frame size. |
| 6 | The startup function is named `lo_entry`, but the process entry must be `main`. | Codegen emits `main` (calls or aliases `lo_entry`). |

I found no correctness bug by reading, but I did not fuzz it or run any native output. The risky areas worth extra tests once native output exists: string comparison operators, nested `And`/`Or`/ternary joins, and constructor delegation chains.
