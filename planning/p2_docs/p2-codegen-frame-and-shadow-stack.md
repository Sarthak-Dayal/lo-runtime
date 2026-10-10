# P2 codegen: frame and shadow stack

`Frame` (`codegen/frame.rs`) owns every stack offset and the prologue/epilogue. The selector never computes an offset; it asks the frame. This is the seam that stays stable while instruction selection and allocation change.

## 1. Inputs

| Input | Source |
|---|---|
| Root slot count | `FunctionIr.root_slots` (after `insert_gc_roots`) |
| Spill slot count | `FunctionAllocation` (spill-everything: one per virtual register) |
| Callee-saved registers used | `FunctionAllocation.used_callee_saved_registers` (none under spill-everything) |
| Whether the function calls | scan for `Call` (decides if there is a shadow frame) |
| Parameter count and types | `FunctionIr.params`, `register_types` |

## 2. Layout (stack grows down)

```
[rbp + 16 + 8*k]   stack argument k (arguments 7 and up)
[rbp + 8]          return address
[rbp]              saved rbp
[rbp - 8*i]        saved callee-saved registers (i = 1..n)
                   shadow frame (16 + 8*roots bytes):
                     +0  parent     (u64)
                     +8  num_roots  (u32, 4 bytes padding)
                     +16 roots[0..n]
                   spill slots (8 bytes each)
                   ret_save (8 bytes)
                   padding to keep rsp 16-byte aligned at every call
[rsp]
```

Shadow frame offsets come from `layout.rs::FrameLayout` (parent 0, num_roots 8, roots 16, size `16 + 8n`); reuse those constants rather than restating them. Every slot is 8 bytes regardless of type. `Int32`/`Bool` use the low 4 bytes.

Alignment: at function entry `rsp ≡ 8 (mod 16)`; after `push rbp`, `rsp ≡ 0`. Total frame size below `rbp` is rounded up to a multiple of 16 so `rsp` stays 16-aligned at every call. Calls with odd stack-argument counts add 8 bytes of padding (instruction-selection doc §5).

## 3. Prologue

1. `push rbp ; mov rbp, rsp ; sub rsp, N`
2. Save used callee-saved registers into their slots.
3. Move incoming parameters from `rdi, rsi, rdx, rcx, r8, r9` (and `[rbp+16+8*k]` for the rest) to their allocated locations, in three ordered phases: stores to memory, then the register-to-register moves as one parallel move (cycles broken through `r11`), then stack-passed arguments into registers. See [register allocation](p2-codegen-regalloc-integration.md) §4.
4. Write the shadow frame: `parent = 0` (the runtime fills it in), `num_roots = n`, every root slot `= 0`. Roots must be null before the first safepoint.
5. `lea rdi, [shadow] ; call lo_push_frame`

Step 5 is a call and clobbers the argument registers, so **incoming parameters must be moved out (step 3) before `lo_push_frame`**. The order above is therefore fixed.

**Exception: the startup function (`ProgramIr.startup`, `lo_entry`).** Its first instruction is `call lo_runtime_init`, which must run before any frame is registered. Build and null the frame in the prologue, but emit step 5 (`lea rdi,[frame] ; call lo_push_frame`) immediately *after* that call. See [startup doc](p2-codegen-startup.md) §4.

## 4. Epilogue (every `Return`)

1. Move the return value to `rax` (or `eax`).
2. `mov [ret_save], rax` — `lo_pop_frame` is a call and clobbers `rax`.
3. `call lo_pop_frame`
4. `mov rax, [ret_save]`
5. Restore callee-saved registers.
6. `leave ; ret` (or `mov rsp, rbp ; pop rbp ; ret`).

For `void` functions, skip the `ret_save` steps.

## 5. Shadow-stack obligations carried over from P1

- Every live pointer-typed value, temporaries included, is in a root slot at each safepoint; dead slots are null. `insert_gc_roots` already inserts the `RootStore`/`RootLoad` traffic as IR instructions, so codegen only has to map a root slot to an address and null the slots in the prologue.
- Every `lo_*` runtime call is a safepoint (ABI §1, §3.3). `Abort` is not treated as one by `gc_roots.rs`; the runtime exits.
- `lo_gc_write_barrier` at every pointer store into a heap object is already enforced by the verifier (`verify.rs`); the selector does not insert it.
- `lo_runtime_init` is the first instruction of `lo_entry` (built by lowering); the frame is registered right after it. The `main` wrapper only calls `lo_entry` and needs no shadow frame of its own. See the startup doc.
- `RootAddr` appears only in the startup function. It yields the address of a root slot, which is published into the `lo_binding_*` words (`statics.rs`). The startup frame must stay registered for the program's life.

## 6. When to register a frame

Rule (confirmed by the language reference via NotebookLM): **register a frame iff the function contains at least one `Call`**, even when `root_slots == 0`, because every call is a safepoint and the frame keeps the parent chain intact. A leaf function (no `Call`) skips `lo_push_frame`/`lo_pop_frame` entirely: no safepoint, so the GC cannot run inside it. A leaf has no shadow frame, no root slots, and no `ret_save` slot, and its epilogue is just `leave ; ret`.

`Frame` takes a `has_calls` flag (computed by scanning the function for `Call`) and omits the shadow frame, `ret_save`, and the `lo_push_frame`/`lo_pop_frame` calls when it is false. An `Abort`-only function counts as a leaf (confirmed, overview §6b).

**A leaf is not stack-less.** A function that only aborts still calls `lo_abort_*`, which runs libc `fprintf`; a misaligned `rsp` at that `call` can fault on SSE. Every function, leaf or not, keeps `push rbp ; mov rbp, rsp ; sub rsp, N` with `N` chosen so `rsp` is 16-byte aligned at every `call`/`Abort`. Test: an abort-only function and a leaf with spills, checking alignment.

## 7. Testing

- Unit-test `Frame` offsets against the table in `native-layout-reference-guide.md` (shadow frame `16 + 8n`).
- Assert 16-byte alignment: for generated frames with 0..8 roots, 0..8 spills and 0..3 stack args, `N + pushed bytes` keeps `rsp` aligned at calls.
- End to end: a program that allocates in a loop (forces collection) with live references across calls; run under the Cheney collector in the container.
