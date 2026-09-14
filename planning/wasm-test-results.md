# WASM test results and fixes by layer

The latest fetched [lo-testing revision](https://github.com/Rahik-Sikder/lo-testing/commit/7851a5985495e885da6dc41195763e7608c7eab2)
passes **75 of 102 LO-3/LO-4 cases** with this emitter. The 102 cases include all
25 contributed cases. Their source files and expected outputs are read unchanged
from the test-repo checkout by the [conformance runner](../scripts/test-conformance.py).
The test repo documents the harness contract but does not ship the instructor's
grading harness; this is a local run of its corpus using the course `wasmrun` host.
LO-2 has a different, procedural grammar and is outside this compiler's scope.

| Result | Count |
|---|---:|
| Passing external cases | 75 |
| Parser diagnostic mismatches | 4 |
| Type-checker failures | 10 |
| Runtime stub failures | 13 |
| Existing Rust frontend/preamble tests passing | 89/89 |

Of the 67 valid/abort programs, 9 stop in the checker. The other 58 all emit,
assemble, link, validate, and instantiate; 45 execute as expected and 13 reach
runtime stubs. Of the 35 invalid programs, 30 produce the expected rejection and
diagnostic; 5 are rejected with a different diagnostic. No remaining failure in
this run was attributed to the emitter, assembler, or linker. This does not prove
the execution paths hidden behind frontend/runtime failures.

The emitter is stacked directly on type-checker PR #5 at `f27fc67`; the checks
below were rerun on that base. The run used Rust 1.87.0, LLVM 18.1.8, and the
unchanged course `wasmrun` source
with Wasmtime 33.0.2, built locally on macOS. The runtime archive was built for
`wasm32-unknown-unknown` with LTO disabled. The exact course Docker image remains
unverified: its build exhausted disk space, and Docker failed to recover after
space was freed. The compiler, checker, and runtime sources were compared with
the fetched branches; the checker and runtime were left unchanged as requested.

Run the commands in the [compiler README](../compiler/README.md) to reproduce the
checks. The [runner](../scripts/test-conformance.py) preserves the exact command,
exit status, stdout, and stderr for each stage in `compiler/target/conformance`.
Its `report.json` contains every individual result. A failed stage stops that case;
later stages are not counted as passing.

## Frontend fixes

The largest group is class/null equality: **nine valid programs** fail in
[`check_binop`](../compiler/src/type_checker.rs). Its initial match rejects
`NullLiteral`, and its class-type cases do not allow equality. The test repo's
[locked language decision](https://github.com/Rahik-Sikder/lo-testing/blob/7851a5985495e885da6dc41195763e7608c7eab2/state-ledger.md#class-type-reference-equality--locked-2026-05-28)
allows reference equality, including null/null and class/null comparisons, while
forbidding reference ordering. Add those `Binop::Eq` cases before the generic
null rejection and return `Type::Bool`. The current typed AST can represent the
result without an interface change; the emitter already selects i32.eq.

Affected cases are LO-3 ValidPrograms `test_17`, `test_24`, `test_30`, `test_32`,
`test_35`, and `test_42`; LO-4 ValidPrograms `test_5_cast_null_passes` and
`test_9_null_field_default`; and contributed LO-4 ValidPrograms
`contributed-tests-6`. Fixing the checker exposes later execution paths, including
cast stubs, so these are not nine guaranteed additional passes.

The tenth checker failure is LO-3 InvalidPrograms `test_20_break_outside_loop`.
The checker correctly rejects it, but reports `E_WELL_FORMEDNESS_OTHER`.
Add `E_BREAK_OUTSIDE_LOOP` to its error vocabulary and use it in `check_stmt`'s
out-of-loop `Break` case.

All four parser cases are rejected; their diagnostic codes differ from the test
contract. They need changes in [parser.rs](../compiler/src/parser.rs), not WASM:

| LO-3 invalid case | Current / expected code | Suggested change |
|---|---|---|
| `test_0` | `E_RESERVED_KEYWORD_AS_IDENTIFIER` / `E_PARSE_PHASE_OTHER` | Recognize the repeated type after a comma in the malformed class field list as declaration syntax, rather than an attempted keyword identifier. |
| `test_13` | `E_PARSE_PHASE_OTHER` / `E_RESERVED_KEYWORD_AS_IDENTIFIER` | Review diagnostic precedence for `while waiter;`. The parser treats it as a malformed loop before reaching the reserved class name. Add contextual detection for the intended illegal type name, or clarify this multi-error test's expected first error. |
| `test_5` | `E_PARSE_PHASE_OTHER` / `E_NULL_LITERAL_RECEIVER` | Recognize a method-call suffix after literal null and route it through the existing null-receiver rejection. Currently the parser stops at the dot while expecting a semicolon. |
| `test_6` | `E_MALFORMED_CLASS_DECL` / `E_PARSE_PHASE_OTHER` | Distinguish a top-level method from a malformed declaration that actually begins with `class`. |

## Runtime fixes

The Rust runtime still has `unimplemented!()` bodies in
[string_ops.rs](../rust/src/string_ops.rs) and [cast.rs](../rust/src/cast.rs).
The assignment says the grading runtime supplies these operations; locally we
must either link that complete archive or implement the stubs in the runtime
workstream. The emitter should continue calling the documented ABI.

| First runtime failure | Cases | Required runtime work |
|---|---:|---|
| `lo_string_new` panic | 8 | Allocate/copy UTF-8 literal bytes into a String object. Other String operations may fail next. |
| `lo_string_compare` panic | 2 | Compare UTF-8 contents lexicographically and return a negative/zero/positive i32. |
| `lo_instanceof` panic | 1 | Walk descriptor parents; null yields false. |
| `lo_cast_check` stub, incomplete abort message | 2 | Implement the checked cast and emit `lo_cast_check: cannot cast Dog to Cat` on these failures. |

The eight `lo_string_new` cases are LO-3 RuntimeAbortPrograms
`test_1_string_repeat_negative`; LO-3 ValidPrograms `test_14_string_reverse_codepoint`,
`test_20`, `test_33`, and `test_45`; LO-4 ValidPrograms `test_3_hello_world`; and
contributed LO-3 ValidPrograms `contributed-tests-1` and `contributed-tests-24`.
The compare cases are LO-3 ValidPrograms `test_43` and contributed LO-4
ValidPrograms `contributed-tests-5`. The instanceof case is LO-4 ValidPrograms
`test_10_instanceof_unrelated_classes`. The cast cases are LO-4 RuntimeAbortPrograms
`test_1_cast_failure` and contributed LO-4 RuntimeAbortPrograms `contributed-tests-4`.

The cast tests already exit with 101 because the host recognizes the function
name in a trap. They still fail correctly: the host's fallback message is only
`lo_cast_check: cast failure`, which does not satisfy the required message.
An exit-code-only test would have hidden the missing implementation.

Ordinary stderr is a separate deferred dependency, not one of these 27 failures.
The latest test repo [records a print-destination selector](https://github.com/Rahik-Sikder/lo-testing/blob/7851a5985495e885da6dc41195763e7608c7eab2/state-ledger.md#runtime-abi--print-family-destination-selector--locked-2026-09-12),
but the fetched course runtime main (`ab140f6`) still has the old signatures.
Once the runtime and host update, append the Output receiver's sink tag to the
print calls in `io_wrapper`, update their declarations, and remove its temporary
stderr trap. No runtime files were changed here.

## Emitter checks

The grading functions also passed a direct check: sourcing is silent, lo-build
succeeds, lo-check rejects with status 1, and lo-compile writes and executes a
requested module path containing spaces from a different working directory.

The 89 Rust tests exercise the existing frontend. The
[production index](wasm-production-index.md) maps a failing rule to its handler.

Changed Rust files pass rustfmt; the crate-wide formatting check still fails in
the unchanged type checker. Strict Clippy still reports the two inherited
checker issues: the `ErrorCode` enum's common `E` prefix and identical branches
in `check_binop`. Those are separate from the conformance failures and were left
unchanged with the rest of the checker. Allowing only those two existing Clippy
lints on the command line produces a clean check of all targets.
