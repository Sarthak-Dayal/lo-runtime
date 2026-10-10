# Prebound bindings: current design vs a static shadow frame

Status: **adopted** in the lowering worktree (reviewed 2026-10-09; `RootStoreIndirect` and `RootAddr` are gone, `lo_bindings` and the Data-base store exemption are in). This doc remains the design record and the diagrams. §5's prompt is historical.

## 1. Where things live in memory

Every function's shadow frame, including `lo_entry`'s and `Main.main`'s, is allocated on the **native stack** by the prologue (`sub rsp, N`, frame at an `rbp`-relative offset). Nothing about a normal frame is in `.data`. What `.data` holds today is only the three binding words.

```
CURRENT DESIGN

  .data (writable)            native stack (grows down)                   heap
  +-----------------+         +-------------------------------+        +-------------+
  | lo_binding_in   |--+      | Main.main frame               |        |             |
  | lo_binding_out  |--|--+   |-------------------------------|        | Output obj  |
  | lo_binding_err  |--|--|-+ | lo_entry frame  (registered)  |        |             |
  +-----------------+  |  | | |   parent | num_roots=3+k      |        +-------------+
   each word holds    |  | | |   root0 <---------------------+--------- in   object
   the ADDRESS of a   |  | | |   root1 <---------------------+--------- out  object
   root slot in the   |  | +-+-> root2 <---------------------+--------- err  object
   lo_entry frame     +--+---->  ...                          |
                         |      +-------------------------------+
                         +--->  (all three words point up into the stack frame)

  read  out : a = Load [lo_binding_out]  (Ptr)   ;  v = Load [a]  (Ref)      2 loads
  write out : a = Load [lo_binding_out]  ;  RootStoreIndirect {a, v}         special instruction
  lo_entry  : root_slots = 3 reserved; RootAddr (startup-only) publishes the addresses
```

```
PROPOSED (static frame)

  .data (writable)                          native stack
  +-------------------------------+         +-------------------------------+
  | lo_bindings  (a ShadowFrame)  |         | Main.main frame               |
  |   parent    (set by runtime)  |<-chain->| lo_entry frame (registered)   |
  |   num_roots = 3               |         |   (ordinary: only gc_roots    |
  |   roots[0]  in   ----------------+      |    slots, no reserved three)  |
  |   roots[1]  out  ----------------+----> +-------------------------------+
  |   roots[2]  err  ----------------+            heap objects
  +-------------------------------+

  read  out : v = Load [Symbol lo_bindings + root(1)]                         1 load
  write out : Store Ref [Symbol lo_bindings + root(1)] = v                    ordinary Store
```

The runtime does not care where a registered frame lives. `lo_push_frame` only links it (`frame->parent = current; current = frame`, `rust/src/shadow_stack.rs`), and the collector reads `num_roots` and the inline root array at a fixed offset (`rust/src/gc.rs` ~lines 138-146). Both work identically on static memory. The runtime's own documentation says the frame is "typically stack-allocated", not required to be.

## 2. The design

**Data.** One writable `DataDef`, symbol `lo_bindings` (name your choice), aligned to pointer size, laid out as the ABI `ShadowFrame` for the target: `parent` (pointer, zero), `num_roots` (u32 = 3, padded), then three zeroed root slots. Offsets come from the same source as codegen's frames: `layout::Target::frame()` (`parent 0 / num_roots 8 / roots 16`, size 40 on x86-64; 0 / 4 / 8, size 20 on Wasm32).

**Reads.** `Prebound` read lowers to `Load { dst: Ref, base: Symbol(lo_bindings), offset: roots + k*ptr }`. A `Data` symbol operand already types as `Ptr`, so the verifier accepts it as a base.

**Writes.** `Prebound` assignment lowers to `Store { base: Symbol(lo_bindings), offset, value, ty: Ref }`. The verifier currently rejects any `Ref` store that is not a barrier call or a `StaticRef` value. Add one parallel exemption: a store whose **base is a `Data` symbol operand** needs no barrier. Soundness: a write barrier exists to record old-to-young pointers *from heap objects*; a static root slot is scanned as a root on every collection, exactly like a stack slot, and a `Symbol` base is never a managed object.

**Startup.** `lo_entry` becomes:

```
call lo_runtime_init
   [codegen registers lo_entry's own frame here]
call lo_push_frame(Symbol lo_bindings)          ; IR call; links the static frame on top
for in, out, err:
    o = lo_alloc(...); ctor(o); (Output) Store [o+16] = 0|1
    Store Ref [Symbol lo_bindings + root(k)] = o
m = lo_alloc(Main); defaults; ctor(m)
r = Main.main(m)                                 ; dispatch
call lo_pop_frame()                              ; IR call; pops the static frame
Return r                                         ; codegen epilogue pops lo_entry's frame
```

Push order is entry frame, then static frame; pops are the reverse, so the stack discipline holds. `lo_entry` itself needs `root_slots = 0` initially; `insert_gc_roots` still adds slots for the objects live across constructor calls.

## 3. What changes

| Where | Change |
|---|---|
| `ir/mod.rs`, `dump.rs`, `verify.rs`, `tests.rs` | **Remove** `RootStoreIndirect` (variant, `destination`/`operands` arms, dump arm, verifier arm, its 4 tests). Optionally remove `RootAddr` too: it exists only for startup (the verifier restricts it to the startup function). |
| `ir/verify.rs` | **Add** the static-base exemption in the `Store` arm, plus a test for accept (Data symbol base) and reject (a `Ptr`-typed register base). |
| `ir/lower/program.rs` | In `declarations`, replace the three `lo_binding_*` words with the one frame `DataDef`. Declare `lo_push_frame (Ptr)` and `lo_pop_frame ()`. |
| `ir/lower.rs::lower_startup` | Drop the `RootAddr`/binding-address loop and `root_slots = 3`. Add the push/pop calls and the per-binding `Store`s. |
| `ir/lower/runtime.rs` | `read_binding` / `assign` `Prebound` arms: single `Load` / `Store` against the symbol. Delete `binding_address`. |
| `ir/program_tests.rs` | Test machine: treat `lo_push_frame`/`lo_pop_frame` as no-ops; model the frame as ordinary writable memory. Update the "startup roots" and "prebound reassignment" tests. |
| `planning/ast-to-ir-lowering-plan.md` | Rewrite "Persistent bindings and GC handoff". |
| later, with the layout/static-data dedupe | `ir/statics.rs::define_static_data` emits the same single frame instead of three words. |

**Net effect on the IR:** one fewer instruction kind, no startup-only instruction, no reserved-slot rule, one verifier rule added. Prebound access gets cheaper (one load).

**Net effect on codegen:** drops the `RootStoreIndirect` and `RootAddr` selector rows, and the "bindings point into the startup frame" invariant. The special rule "register `lo_entry`'s frame after `lo_runtime_init`" stays. Binding access is `lea r10, [rip + lo_bindings] ; mov rax, [r10 + off]` (and the mirror `mov [r10 + off], rax`), already covered by the generic `Load`/`Store` rows with a `Symbol` base.

## 4. Checks done and risks

- Runtime evidence, Rust skeleton: `lo_push_frame` only links; `lo_pop_frame` only unlinks; the collector reads `num_roots`/roots by offset. No assumption that frames are on the stack. The C++ version has the same shape (`cpp/src/shadow_stack.cpp`). I did not read the Zig one.
- `lo_runtime_init` resets the head to null (`shadow_stack::reset`), which confirms that pushes must follow init, for both frames.
- A moving collector updates root slots in place; static memory is fine.
- **Unverified:** the instructor's private grading runtime. The ABI text does not require stack-allocated frames, but run the finished native build against all three skeleton runtimes (and ask on the course channel if the grading runtime is available) before relying on it.
- The generational barrier question is settled by the rule above: roots are always scanned.
- Minor: the new verifier rule must stay tied to `Symbol` bases. Do not extend it to any `Ptr`-typed register, which could point into the heap.

## 5. Prompt for the worktree

```
Replace the persistent-binding mechanism in the lowering with a static shadow frame.
Do not commit or push.

Goal: the prebound objects in/out/err live in the roots of ONE writable static data
symbol laid out as an ABI ShadowFrame, registered with lo_push_frame; reads and writes
are plain Load/Store on Symbol(that frame). RootStoreIndirect and RootAddr go away.

1. Data: in ir/lower/program.rs::declarations, replace the three lo_binding_* words by a
   single Section::Writable DataDef "lo_bindings" (align = pointer bytes) laid out as a
   ShadowFrame for the target: parent (pointer-sized zero), num_roots as U32(3), padding
   up to the roots offset, then 3 pointer-sized zeros. Offsets:
   x86-64: parent 0, num_roots 8, roots 16 (+8*k), total 40.
   Wasm32: parent 0, num_roots 4, roots 8 (+4*k), total 20.
   (Same as layout::Target::frame() after origin/main is merged.)
   Declare runtime functions lo_push_frame (Ptr)->void and lo_pop_frame ()->void.
2. lower/runtime.rs: Prebound read = Load{Ref, base: Symbol(lo_bindings), offset: root(k)};
   Prebound assign = Store{base: Symbol(lo_bindings), offset: root(k), value, ty: Ref}
   (k = 0,1,2 for in,out,err). Delete binding_address and the RootStoreIndirect use.
3. lower.rs::lower_startup: root_slots = 0 (drop RootAddr and the address-publishing
   loop). Order: call lo_runtime_init; call lo_push_frame(Symbol lo_bindings); for each
   of in/out/err allocate+construct, set the Output selector (0 out, 1 err), Store the
   object into its root; then allocate/construct Main, dispatch main, call lo_pop_frame,
   Return the result.
4. ir/verify.rs, Store arm: a Ref store needs no write-barrier call when its base is an
   Operand::Symbol whose symbol kind is Data (a static root, never a heap object), in
   addition to the existing StaticRef-value exemption. Do NOT exempt Ptr-typed register
   bases. Add tests: accepted with a Data-symbol base; still rejected with a Ptr register base.
5. Remove InstructionKind::RootStoreIndirect from ir/mod.rs (variant, destination(),
   operands()), dump.rs, verify.rs and its tests in ir/tests.rs. Remove RootAddr and its
   startup-only verifier rule if nothing else uses it.
6. program_tests.rs: test machine treats lo_push_frame/lo_pop_frame as no-ops and the
   frame as ordinary writable memory; update the startup-roots and prebound-reassignment
   tests; keep the aliasing/I-O destination tests.
7. Update planning/ast-to-ir-lowering-plan.md "Persistent bindings and GC handoff".

Run cargo test, fmt check on changed files, clippy.
```
