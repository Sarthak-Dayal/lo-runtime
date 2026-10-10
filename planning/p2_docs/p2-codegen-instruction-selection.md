# P2 codegen: instruction selection

Decision: **no low-level IR. Select directly from `InstructionKind`/`Terminator` into a typed `Inst` enum.**

## 1. The question

The handout (§2.2) says: "Tree-pattern and BURS-style selection are covered in lecture; the requirement on your implementation is that selection is systematic and legible — a per-operator emission discipline you can state — not an ad-hoc case explosion."

Does that imply a second, lower-level IR? No.

## 2. Why direct IR matching

1. **The handout defines one IR.** §2.1 asks for a three-address IR with explicit operands and basic blocks. No LIR or machine IR appears anywhere in the spec or the grading criteria. Code review looks at "IR design coherence" and "legibility of the selector", not at a second representation.
2. **Our IR is already low-level.** `Load`/`Store` carry base + offset, `Call` carries explicit args, root-slot traffic is explicit (`RootStore`, `RootLoad`, `RootAddr`), runtime helpers are ordinary calls, and `Abort` is a terminator. Nothing remains to lower before x86.
3. **BURS and tree patterns need trees.** They pay off when nested expressions can fold into one addressing mode or instruction. Our TAC is flat; tree matching would require rebuilding trees first. No optimization is graded (§2.5), so the payoff is zero.
4. **Schedule.** A second IR is a third lowering pass plus its dump, verifier and tests, with 3 days left.

What the handout actually asks for is a *discipline you can state*. That is a property of how the selector is written, not of an extra data structure.

## 3. The discipline (state this in the design note)

1. **One emitter per operator.** A flat `match` on `InstructionKind`, then on `BinaryOp`/`UnaryOp`. No cross-operator special cases.
2. **One template.** *Fetch operands, operate, write back.* Operands that x86 cannot encode directly are loaded into a scratch register first. Two helpers hold every memory-operand rule, so no emitter reasons about it:
   - `src(operand, scratch)`: returns register, immediate or memory operand; loads into `scratch` only when the instruction cannot take the form.
   - `dst(vreg)`: returns the destination location, or the scratch plus a pending write-back.
3. **Reserved scratch registers:** `rax`, `rcx`, `rdx`, `r10`, `r11`. The allocator already reserves exactly these (`target.rs`), so the selector cannot clobber an allocated value. `rax`/`rdx` double as the `cdq`/`idiv` registers.
4. **Width discipline.** `Int32` and `Bool` use 32-bit registers (`eax`); `Ref`, `Ptr`, `CodePtr` use 64-bit (`rax`). Width comes from the register or operand type, never from the call site. Writing a 32-bit register zero-extends, so no extra instruction is needed.
5. **Typed output.** The selector produces `Inst` values. The enum contains only subset instructions, so out-of-subset code cannot be emitted. Tests compare instruction lists structurally; `print.rs` is the only place that formats text.

## 4. Emission table

Spill-everything locations are written `[rbp-k]`; with linear scan the same templates apply and `src`/`dst` simply return registers, so fewer loads and stores appear. `W` is the register width from the type.

| IR | Emission |
|---|---|
| `Copy d, s` | `mov r11W, s ; mov d, r11W` (memory-to-memory is not encodable) |
| `Binary Add` / `Sub` | `mov eax, l ; add/sub eax, r ; mov d, eax` |
| `Binary Mul` | `mov eax, l ; imul eax, r ; mov d, eax` (immediate `r` uses three-operand `imul eax, l, imm`) |
| `Binary Div` | `mov eax, l ; cdq ; mov r10d, r ; idiv r10d ; mov d, eax`. Lowering guarantees the divisor is never 0 or -1 here (LO division is total; `lower.rs::division` branches around those cases), so `idiv` cannot fault. The selector does not re-check. |
| `Binary Mod` | same as `Div`, then `mov d, edx` |
| `Binary Eq` / `Lt` / `Gt` | `mov eax, l ; cmp eax, r ; sete/setl/setg al ; movzx eax, al ; mov d, eax` |
| `Eq` on `Ref` | as above with 64-bit `rax`; `Null` is immediate 0 |
| `Unary Neg` | `mov eax, s ; neg eax ; mov d, eax` |
| `Unary Not` | `mov eax, s ; xor eax, 1 ; mov d, eax` (Bool is 0 or 1) |
| `Load d, [b+off]` | `mov r10, b ; mov eax/rax, [r10+off] ; mov d, eax/rax` (width from `d`'s type) |
| `Store [b+off], v` | `mov r10, b ; mov eax/rax, v ; mov [r10+off], eax/rax` (width from `ty`) |
| `RootStore slot, v` | `mov rax, v ; mov [rbp - shadow + 16 + 8*slot], rax` |
| `RootLoad d, slot` | `mov rax, [rbp - shadow + 16 + 8*slot] ; mov d, rax` |
| `Call Direct` | see §5 ; `call sym` ; store `eax`/`rax` to `dst` |
| `Call Indirect` | callee to `r11` ; `call r11` |
| `Jump L` | `jmp L` (omitted when `L` is the next block) |
| `Branch c, T, E` | `cmp c, 0 ; jne T ; jmp E` (`jmp E` omitted when `E` is next) |
| `Return v` | `mov eax/rax, v` ; epilogue ; `ret` |
| `Abort sym, args` | args as for `Call` ; `call sym` ; nothing follows |
| `Operand::Symbol` | `lea reg, [rip + sym]` (RIP-relative, required by the subset) |

`Operand::Int` and `Bool` are 32-bit immediates; `Null` is 0.

**Operand types.** `Binary Eq` applies to Int32, Bool and Ref, so the selector needs each operand's `IrType` to choose `eax` vs `rax`. Use `ProgramIr::operand_type` (currently private in `verify.rs`; make it `pub(crate)`). `Operand::Symbol` is `Ptr` for data, `Ref` for a `StaticRef`, and `CodePtr` for a function.

## 5. Calls

- Argument registers in order: `rdi, rsi, rdx, rcx, r8, r9`; arguments 7 and up go on the stack, pushed right to left.
- `rsp` must be 16-byte aligned at the `call`. The frame is sized so it is aligned at function body level; when an odd number of stack arguments are pushed, pad with `sub rsp, 8` first, and clean up after the call.
- `Int32` and `Bool` arguments are passed in the 32-bit register half. `Ref`/`Ptr` are 64-bit.
- **Bool returns:** System V leaves bits 8-31 of `eax` undefined for a `bool`-returning callee. After any call whose result type is `Bool` (for example `lo_instanceof`), emit `movzx eax, al`.
- Sequential argument moves are correct because a function that makes a call is allocated only callee-saved registers, so no operand is ever in an argument register. The selector returns an error if that invariant is ever violated. Incoming *parameters* are a different case (they arrive in the argument registers) and are handled by the parallel move in the prologue; see [register allocation](p2-codegen-regalloc-integration.md).
- Indirect callee goes to `r11`, which is never an argument register.

## 6. Out of scope / notes

- Compare-and-branch fusion and other peepholes are non-goals (§2.5). `Branch` on a materialized boolean is correct and legible; a fused form is a possible post-tag improvement.
- The subset has no `ud2`. Nothing is emitted after an `Abort` call (runtime exits).
- `cmov`, SIMD, and unsigned jumps are outside the subset and not used. `shl`/`sar` are available but nothing in the current IR needs them.
- `inc`/`dec`, `push`/`pop`, `lea` are used by prologues and addressing only.

## 7. Testing

- Per-operator golden tests: IR instruction in, expected `Inst` list out (structural), plus a small set of printed-text checks for `print.rs`.
- Subset conformance is by construction: `Inst` has no variant outside `instruction_subset.md`, and `Inst::check` rejects unencodable operand forms (memory-to-memory, width mismatch, 64-bit immediates outside `mov reg64, imm`).
- Every emitted program is assembled, linked and run in an amd64 Linux container, and its stdout, stderr and exit status must match the interpreter's.
