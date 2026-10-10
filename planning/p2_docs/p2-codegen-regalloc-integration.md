# P2 codegen: register allocation

Linear scan (Poletto-Sarkar) over the System V register file is the default allocator. Spill-everything, where every virtual register lives in its own stack slot, is kept as a reference producer of the same output. Both are selectable from the command line (`--spill-all` selects the reference).

## 1. The seam

The selector and the frame consume `FunctionAllocation` / `PhysicalLocation` (`ir/register_allocator/mod.rs`): each virtual register maps to a physical register or a spill slot.

```
allocate_registers(&CheckedIr, &TargetConstraints::system_v())     linear scan
spill_everything_program(&CheckedIr)                               reference
        both return ProgramAllocation { function_allocations: SymbolId -> FunctionAllocation }

FunctionAllocation {
    register_locations: VirtualRegId -> Register(reg) | Spill(slot),
    spill_slot_count,
    used_callee_saved_registers,        // what the prologue must save and the epilogue restore
}
```

The selector's `src` / `fetch` helpers are the only code that inspects a location. With spill-everything they produce memory operands plus scratch loads; with linear scan they produce registers and most of the moves disappear. Because both producers return the same type, a program that behaves differently under the two points at the allocator or at its integration, not at instruction selection.

## 2. The register file (System V AMD64)

| Role | Registers |
|---|---|
| Argument registers, in order | `rdi, rsi, rdx, rcx, r8, r9` (further arguments go on the stack) |
| Return value | `rax` (`eax` for Int32 and Bool) |
| Caller-saved | `rax, rcx, rdx, rsi, rdi, r8, r9, r10, r11` |
| Callee-saved | `rbx, rbp, r12, r13, r14, r15` (and `rsp`, restored by the frame) |
| Reserved: frame | `rsp, rbp` |
| Reserved: scratch for the back end | `rax, rcx, rdx, r10, r11`. These carry accumulators, `cdq` / `idiv`, shift counts, address computation, call setup and indirect call targets, and the parallel-move cycle breaker (`r11`). The allocator never hands them out. |
| Allocatable | `rsi, rdi, r8, r9` (caller-saved, listed first) then `rbx, r12, r13, r14, r15` (callee-saved) |

The caller-saved registers come first in the pool: a leaf function can use them without saving anything.

## 3. Calls and the callee-saved registers

**Policy.** A function that contains a `Call` or an `Abort` is allocated only from the callee-saved registers (`rbx, r12–r15`). A value living in one of them survives every call without interval splitting or save/restore moves around the call, and argument setup (which writes `rdi, rsi, …`) can never clobber an allocated value. The cost is that a function that calls has five registers; the rest of its values spill. A leaf function has all nine.

**Saving them.** The callee-saved registers a function actually uses are reported in `used_callee_saved_registers`. `Frame` stores each into a slot just below the saved `rbp` in the prologue and reloads it in the epilogue, after `lo_pop_frame` (a call, which would otherwise be free to clobber caller-saved registers). A function that uses none saves nothing.

**GC roots.** The allocator needs no knowledge of the collector. `insert_gc_roots` has already stored every `Ref` that is live across a call into a shadow-stack root slot and reloads it after the call, so a collection that moves an object updates the root slot and the reload picks up the new address. A `Ref` that sits in `rbx` across the call is simply overwritten by that reload.

## 4. Parameter moves

Incoming arguments arrive in `rdi, rsi, rdx, rcx, r8, r9` (and `[rbp+16+8k]` beyond six). The allocator assigns each parameter wherever it likes, and in a leaf function it may pick the arrival register of a *different* parameter. For example parameter 0 allocated to `rsi` while parameter 1 arrives in `rsi`:

```
param 0 (edi)  ->  esi        param 1 (rsi)  ->  rdi        a swap: naive sequential moves
                                                              destroy one of the values
```

`Frame::prologue` therefore moves parameters in three ordered phases so nothing is overwritten before it is read:

1. **Stores to memory.** Argument registers destined for spill slots only read their source. Stack-passed arguments destined for memory go through `rax`, which is nobody's source.
2. **Register to register, as one parallel move** (`codegen/moves.rs`). A move is emitted once no other pending move still reads its destination. If every remaining destination is still a source, there is a cycle: one destination's current value is parked in `r11`, its readers take it from there, and the cycle unravels.
3. **Stack-passed arguments into registers.** Their sources are memory and are never clobbered, but their destinations may be argument registers that phase 2 still needed.

```
swap of esi/edi, widths kept per move:
    mov r11, rsi          ; park rsi, the value parameter 1 brought in
    mov esi, edi          ; parameter 0 lands in esi
    mov rdi, r11          ; parameter 1 lands in rdi
```

The same file is the only place that reasons about this; the exhaustive test enumerates every injective assignment of the argument registers to a pool of destinations and executes the emitted moves against a register-file model.

**Call arguments need no parallel move.** In a function that makes a call, the policy above means no value is in an argument register, so the arguments can be moved into place one at a time. The selector enforces this: a call whose operand lives in an argument register is an error rather than silent miscompilation.

## 5. Output for hello world

See the [walkthrough](p2-hello-world-walkthrough.md) for the same program under both producers, with the saves, restores and moves marked.

## 6. Not implemented

- **Interval splitting and live-range holes.** An interval is a single `[start, end]` range; when registers run out the interval that ends furthest away is spilled for its whole life.
- **Call-site save/restore.** The call policy replaces it, at the price of five registers in functions that call.
- **Coalescing and move elimination.** Copies between virtual registers that end up in different registers stay as `mov`s.
- **Graph coloring.** Not built; it is a paper comparison in the design note.
