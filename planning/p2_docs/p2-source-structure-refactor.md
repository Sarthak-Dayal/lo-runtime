# Source structure consolidation

Decision: **do it after the `p2-final` tag**, as a short series of mechanical, behavior-neutral PRs. Until then, new code follows the convention below so the problem does not grow.

## 1. The complaint

Too many files and subdirectories, and definitions of a type are far from its implementations. Measured on `compiler/src/ir/` (12,929 Rust lines in the crate):

| Type | Definition | Where its methods live |
|---|---|---|
| `ProgramIr` | `ir/mod.rs:81` | `intern_signature` in `mod.rs`; verification (about 15 private helpers) in `verify.rs`; `dump()` and text helpers in `dump.rs`; declaration helpers in `declare.rs` |
| `CheckedIr` | `mod.rs:77` | `program()`/`into_program()` in `verify.rs` (private field means only `verify.rs` can construct it) |
| `FunctionIr` | `mod.rs:107` | `new_register` in `mod.rs`; verification as free functions in `verify.rs`; a separate `FunctionDump` struct in `dump.rs` |
| `InstructionKind`, `Terminator` | `mod.rs` | use/def/successor helpers in `mod.rs` (good) |
| `BasicBlock` | `mod.rs:137` | none; `liveness.rs` takes it as a parameter |
| `Operand`, `BinaryOp`, `IrType` | `mod.rs` | text rendering as free functions in `dump.rs`; type checks as `ProgramIr` methods in `verify.rs` |

Other findings:
- Every sibling file begins `use super::*;`, so a name's origin is invisible where it is used.
- `register_allocator/` is five small files (mod 74, liveness 90, live_intervals 89, allocation 246, target 114) plus an 819-line test file. `LivenessInfo` is `pub` in a private module and cannot be named outside it.
- `allocation.rs:1` has a stray `#[cfg(test)] use` above a normal import.
- Two unrelated functions share the name `value_type` (`lower.rs:76` AST type to `IrType`; `verify.rs:387` register to its type).
- `gc_roots.rs` recomputes per-instruction liveness that the allocator's liveness module does not expose.
- `main.rs` has `#![allow(dead_code)]` and declares `mod ir;` that nothing outside `ir/` uses yet.

## 2. Principle

**Group by concept, not by operation.** A reader should open one file to see a type and everything it can do. Analyses and passes (verify, dump, liveness, allocation) stay separate files but are exposed as functions over the types, not as `impl` blocks scattered on types owned elsewhere.

## 3. Target shape (proposal; confirm after the tag)

```
ir/
  types.rs        ids, IrType, Signature, Operand, BinaryOp, UnaryOp          (+ their text rendering)
  program.rs      ProgramIr, CheckedIr, Symbol, DataDef, declaration helpers
  function.rs     FunctionIr, BasicBlock, Instruction, InstructionKind, Terminator, use/def helpers
  verify.rs       pub fn verify(&ProgramIr) -> Result<CheckedIr, _>  (passes, not impls on foreign types)
  dump.rs         pub fn dump(&ProgramIr) -> String
  lower.rs, gc_roots.rs, runtime.rs, statics.rs   unchanged
  register_allocator.rs (or dir)   liveness + intervals merged; allocation; target
codegen/          new code, already follows the convention
```

Open design point for the user: `CheckedIr` has a private field so only the verifier can build it. Keeping it in `verify.rs` is legitimate (it is the proof-of-verification token); moving it to `program.rs` needs `pub(super)` construction. Pick during the refactor.

P1 directories (`wasm/`, `interpreter/`, `type_checker/`) and the large `parser.rs`/`lexer.rs` are out of scope.

## 4. Convention for new code now

Applies to `codegen/` and any new file:
- One concept per file; the type and all its inherent `impl` blocks together.
- Explicit imports; no `use super::*`.
- No new file under 50 lines unless it is a distinct concept.
- Tests in the same file under `#[cfg(test)] mod tests`, unless over about 300 lines.

## 5. Timing and why

| Option | Verdict |
|---|---|
| Before codegen | **No.** A teammate and the user are editing `ir/lower.rs` and `ir/mod.rs`; moving type definitions now causes conflicts and risks the Mon 10/12 deadline. |
| During codegen | No. Mixing moves with feature work makes review and bisection harder. |
| **After `p2-final` tag** | **Yes.** Behavior-neutral work is easiest to verify when nothing else is changing. |

Window: Tue 10/13–Wed 10/14, before Project 3 is assigned (Wed 10/14). If that is too tight, fold it into the first days of P3, before any P3 code lands in `ir/`.

## 6. Steps (each is one PR, user commits)

1. **Move, don't change.** Split `ir/mod.rs` into `types.rs`, `program.rs`, `function.rs`; keep `mod.rs` as a re-export facade so call sites compile unchanged. Gate: `cargo test` count identical before and after; `cargo clippy` clean.
2. **Make dependencies explicit.** Replace `use super::*` with explicit imports in every `ir/` file; delete the facade re-exports that become unused; remove the stray `cfg(test)` import; rename one of the two `value_type` functions.
3. **Merge and expose.** Merge `liveness.rs` and `live_intervals.rs`; export `LivenessInfo`; have `gc_roots.rs` use a shared per-instruction liveness helper if one is added. Turn `verify`/`dump` into functions over the types.

## 7. Verification

- Before step 1: record `cargo test --manifest-path compiler/Cargo.toml` pass count and the `--dump-ir` output for the corpus.
- After each step: same test count, byte-identical IR dumps, `cargo clippy --all-targets -- -D warnings` (the repo's pre-commit standard).
