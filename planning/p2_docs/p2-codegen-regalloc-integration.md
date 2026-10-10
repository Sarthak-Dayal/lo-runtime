# P2 codegen: spill-everything first, then linear scan

Plan: get a spill-everything backend correct and verified first (handout §6 calls this "the classic de-risking move"), then switch the producer of the allocation to the real linear-scan allocator. Codegen should not change beyond the call-site and prologue moves described here.

## 1. The seam

The selector consumes `FunctionAllocation` / `PhysicalLocation` (`register_allocator/mod.rs`): each virtual register maps to a physical register or a spill slot.

- **Now:** `spill_everything(&FunctionIr) -> FunctionAllocation` maps every virtual register to its own `Spill` slot. Place it next to `allocate_registers` so both producers live together and return the same type.
- **Later:** call `allocate_registers(&CheckedIr, &TargetConstraints)` instead.

The selector's `src`/`dst` helpers (instruction-selection doc §3) are the only code that inspects a location. With spill-everything they always produce memory operands plus scratch loads; with linear scan they produce registers and the redundant moves disappear. No emitter changes.

## 2. System V retarget (D4)

`register_allocator/target.rs` currently builds `TargetConstraints::microsoft_x64()`:

| | Current (Microsoft x64) | Required (System V AMD64) |
|---|---|---|
| Argument registers | `rcx, rdx, r8, r9` | `rdi, rsi, rdx, rcx, r8, r9` |
| Shadow space | 32 bytes | none |
| Caller-saved | per MS ABI | `rax, rcx, rdx, rsi, rdi, r8, r9, r10, r11` |
| Callee-saved | `rbx, rsi, rdi, r12-r15` | `rbx, rbp, r12, r13, r14, r15` |
| Reserved today | `rsp, rbp, rax, rcx, rdx, r10, r11` | keep: `rsp, rbp` frame; `rax, rcx, rdx, r10, r11` scratch |

Changes:
1. Add `TargetConstraints::system_v()` and make it the one used by the driver. Remove or demote the Microsoft constructor (a test may still use it; check `register_allocator/tests.rs`).
2. Allocatable set becomes the callee-saved `rbx, r12, r13, r14, r15` plus caller-saved `rsi, rdi, r8, r9` for call-free functions. `rdx` and `rcx` are scratch and excluded.
3. Update `validate_target` and the allocator tests that assert specific registers.
4. Keep the existing policy: a function containing any `Call`/`Abort` allocates only callee-saved registers. It is conservative (no save/restore around calls) and avoids caller-save handling entirely. Cost: only 5 registers (`rbx, r12-r15`) in call-containing functions, so spills increase. Acceptable; correctness first.

Because `gc_roots.rs` already forces every live `Ref` through a root slot around calls, callee-saved values surviving calls is a performance concern, not a correctness one.

## 3. Moves linear scan introduces

Spill-everything has no register-to-register conflicts: sources are memory or immediates. Linear scan needs two parallel-move sites.

1. **Call arguments.** Argument registers `rdi, rsi, r8, r9` can be allocated in call-free functions only, so in functions that contain a call no source is an argument register and arguments can be moved in sequence. Verify this holds, then rely on it. If the policy ever relaxes, switch to a proper parallel move (topological order with a scratch to break cycles).
2. **Prologue parameters.** Incoming parameters arrive in `rdi, rsi, rdx, rcx, r8, r9` and move to allocated locations. In call-free functions a destination can be another parameter's source register (for example parameter 0 allocated to `rsi` while parameter 1 arrives in `rsi`). Implement the prologue parameter move as a parallel move: emit moves whose destination is not a pending source first; break cycles with `r11`.

Add one unit test per case, including a swap cycle.

## 4. Callee-saved registers

Prologue saves, and epilogue restores, exactly the callee-saved registers the allocation uses. Requires `FunctionAllocation` to expose the set of registers used (or computable from the locations). These save slots sit above the shadow frame in `frame.rs`.

## 5. Verification

Switch only when spill-everything passes the full local corpus. Then:

1. Run the same corpus with the allocator on; diff stdout and exit codes against the spill-everything build, the interpreter and the WASM build.
2. Add register-pressure programs (handout §7: "register-pressure expressions that force spills") and many-argument calls (more than 6 arguments, stack-passed).
3. Keep a driver flag (`--spill-all`) permanently so any native divergence can be bisected between selection and allocation, as the handout recommends.

## 6. Risks

| Risk | Mitigation |
|---|---|
| Allocator tests encode Microsoft registers | Update alongside the retarget; run `cargo test` before touching codegen. |
| Unknown `FunctionAllocation` shape | Read `register_allocator/mod.rs` before writing `spill_everything`; adjust docs. |
| Time (linear scan on Sunday) | Cut line in the overview: ship spill-everything if the allocator is not fully verified. |
