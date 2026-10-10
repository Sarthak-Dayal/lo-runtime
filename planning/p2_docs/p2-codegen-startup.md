# P2 codegen: the startup function (`lo_entry` → `main`)

**Decision (user, 2026-10-09): follow lowering.** The new `lower_program` (worktree branch `feat/ast-to-ir-lowering`) already builds the startup function `lo_entry` and records it in `ProgramIr.startup`. Codegen does not build startup; it wraps and frames it. This replaces the earlier plan to build startup in the codegen phase. Background on everything lowering emits: [lowering primer](p2-lowering-primer.md).

## 1. What `lo_entry` does (from `ir/lower.rs::lower_startup`, static-frame version)

```
lo_entry() -> Int32                      root_slots = 0 from lowering
 |
 |  Call lo_runtime_init()                         <-- first instruction
 |  Call lo_push_frame(Symbol lo_bindings)         <-- links the STATIC frame in .data
 |
 |  for name in [in, out, err]:
 |      o = lo_alloc(&lo_class_N_<Input|Output>)
 |      Call lo_ctor_N_<Class>_0(o)
 |      (Output) Store Int32 [o + 16] = 0 (out) or 1 (err)
 |      Store Ref [Symbol lo_bindings + 16 | 24 | 32] = o      ; no barrier: static root
 |
 |  m = lo_alloc(&lo_class_4_Main)
 |  Store Ref [m + off] = LO_EMPTY_STRING   per Main String field
 |  Call lo_ctor_4_Main_0(m)
 |  r = Main.main via vtable dispatch (null check, 3 loads, Call Indirect)
 |  Call lo_pop_frame()                            <-- unlinks the static frame
 |  Return r
```

`lo_bindings` is a writable `.data` object laid out as a `ShadowFrame` (parent 0, num_roots 3, roots at +16/+24/+32). See [bindings doc](p2-bindings-static-frame.md). `lo_entry` has no reserved root slots; `insert_gc_roots` only adds slots for references live across its own calls (4 in the sample, one per allocated object).

Two different frames are registered, in this order: `lo_entry`'s own frame (codegen, on the native stack) and then `lo_bindings` (the IR call above). Pops are the reverse: the IR `lo_pop_frame` pops `lo_bindings`, then `lo_entry`'s epilogue pops its own frame.

## 2. What varies per program

Almost nothing. The prebound objects and the call sequence are fixed. Only these change:
- `Main`'s String fields, which get `LO_EMPTY_STRING` stores after `lo_alloc`.
- Whether `lo_ctor_4_Main_0` is explicit or implicit (lowering now emits both).
- The vtable slot of `Main.main` (since `Main` cannot extend a class, `main` has a fixed owner but a slot that depends on `Main`'s other methods).

## 3. Where startup sits in the pipeline

```
lower_program ──► CheckedIr ──► insert_gc_roots ──► allocate ──► emit
   builds lo_entry              adds root slots        │          │
   and lo_bindings              for refs live across   │          ├─ frame setup for lo_entry
                                calls (nulls others)   │          │   (registration AFTER lo_runtime_init)
                                                       │          └─ `main` wrapper
                                                       ▼
                                                 normal locations
```

## 4. The one ordering rule codegen must respect

`lo_runtime_init` is the **first instruction** of `lo_entry`, but a normal prologue registers the shadow frame **before** the first instruction. Registering a frame before runtime init is unsafe: init may reset the runtime's current-frame pointer, dropping the frame. P1 avoided this by calling `lo_runtime_init` before the frame push (`wasm/function.rs`, `finish`).

```
normal function                         lo_entry (startup)
---------------                         ------------------
push rbp ; mov rbp, rsp ; sub rsp, N    push rbp ; mov rbp, rsp ; sub rsp, N
save params                             (no params)
null roots, set num_roots               null roots, set num_roots      } memory only,
lea rdi,[frame]; call lo_push_frame     (deferred)                     } no runtime call
<body>                                  <body: instruction 1>
                                          call lo_runtime_init
                                        lea rdi,[frame]; call lo_push_frame   <-- right here
                                        <rest of body>
...                                     ...
call lo_pop_frame ; leave ; ret         call lo_pop_frame ; leave ; ret
```

Implementation rule in `select.rs`/`frame.rs`: for the function equal to `ProgramIr.startup`, the frame object is built in the prologue but `lo_push_frame` is emitted immediately after the `Call lo_runtime_init` instruction (the first `Call` in the entry block), not in the prologue. Any `RootStore` that `insert_gc_roots` places before that call only writes frame memory, which is valid whether or not the frame is registered. If no `lo_runtime_init` call is found, fall back to normal registration and report a codegen error rather than guessing.

## 5. The `main` wrapper

The process entry is the C symbol `main`; `gcc -static` supplies `_start`/crt and passes `main`'s `eax` to `exit`. The simplest correct wrapper:

```
        .globl main
main:
        push rbp
        mov  rbp, rsp
        call lo_entry          ; frame registered, bindings built, Main.main run, frame popped
        pop  rbp
        ret                    ; eax = Main.main's result = process exit status
```

`rsp` is 16-aligned at the `call` (after `push rbp` it is 0 mod 16 because `main` is entered with `rsp ≡ 8`). Rename-in-place (emitting `lo_entry` as `main` directly) is equivalent; the wrapper keeps the IR symbol name intact for dumps and tests. `lo_runtime_shutdown` is optional on native and is not called.

## 6. Prebound bindings at the machine level

```
.data:   lo_bindings : 40 bytes, writable, ShadowFrame-shaped
            +0  parent     (runtime fills it in lo_push_frame)
            +8  num_roots  = 3
            +16 roots[0]   in     <- Store Ref [Symbol lo_bindings + 16]
            +24 roots[1]   out
            +32 roots[2]   err

Load / Store with Symbol base ==>  lea r10, [rip + lo_bindings]
                                   mov rax, qword ptr [r10 + 24]      (or mov [r10 + 24], rax)
```

These are the generic `Load`/`Store` rows; nothing special for the backend. The collector finds `in`/`out`/`err` because `lo_bindings` is on the frame chain while `Main.main` runs, and updates them in place. A `Ref` store with a `Data`-symbol base is verifier-exempt from the write barrier.

## 7. Testing

- Golden check: for `class Main () { int main() { return 0; } }`, assert the emitted `lo_entry` has `call lo_runtime_init` followed immediately by `lea rdi, [..]` / `call lo_push_frame`, and no `lo_push_frame` before it.
- Alignment check on the `main` wrapper and on `lo_entry`'s calls.
- End to end (container): `Main.main` returning `n` yields exit status `n`; a program that reassigns `out` keeps printing to the new object after a forced collection.

## 8. Dependencies

Lowering must be merged first (`lower_program`, `lo_bindings`, the verifier exemption). Until then the selector can be developed and tested on hand-built IR.
