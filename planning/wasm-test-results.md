# WASM test results and fixes by layer

The latest fetched [lo-testing revision](https://github.com/Rahik-Sikder/lo-testing/commit/7851a5985495e885da6dc41195763e7608c7eab2)
contains 102 LO-3/LO-4 cases, including all 25 contributed cases. The
[conformance runner](../scripts/test-conformance.py) reads those files unchanged
and checks parsing, semantic analysis, assembly, linking, status, stdout, and
abort diagnostics through the course `wasmrun` host.

With the updated type checker and temporary local implementations of the runtime
String/cast stubs, the current checkout passes **97/102 cases as the suite is
written**:

| Remaining result | Count | Owning layer |
|---|---:|---|
| Parser diagnostic-code mismatch | 4 | Parser |
| Stale `lo_alloc` valid-test expectation | 1 | Test corpus |
| Emitter, linker, or execution failure | 0 | — |

The instructor has since reserved the `lo_` prefix for runtime ABI entry points.
`contributed-tests/LO-3/ValidPrograms/contributed-tests-19.lo` still declares a
user method named `lo_alloc` and expects acceptance. The compiler now correctly
rejects it with `E_RESERVED_VARIABLE_NAME`; after that test is updated or moved,
the result is **98/102**. The four remaining failures are all programs that the
frontend rejects with a different parser diagnostic code than the test header
requests.

The committed Rust runtime intentionally retains its assignment stubs. A clean
checkout linked to that skeleton fails 13 additional runtime-dependent cases.
The temporary local implementations used to expose later emitter paths are not
part of this PR; the grading handout says the compiler is linked with the
instructor's complete runtime.

## Changes exposed by the updated type checker

Type-checker commit `30cf34e` adds legal equality between class references and
`null`, and reports `E_BREAK_OUTSIDE_LOOP` for an out-of-loop break. The break
test and five null-equality programs passed immediately. Four larger null-using
programs then reached assembly for the first time and exposed an emitter issue:

- `LO-3/ValidPrograms/test_30.lo`
- `LO-3/ValidPrograms/test_32.lo`
- `LO-3/ValidPrograms/test_35.lo`
- `LO-3/ValidPrograms/test_42.lo`

Each contains nested `while` loops in a non-void function. LLVM rejected the
inner unconditional back edge with `br: insufficient values on the type stack`.
P15 now emits a constant-true `br_if` for the loop back edge. This has the same
runtime behavior as an unconditional branch and keeps LLVM's nested control-flow
stack inference valid. All four programs now assemble and return their declared
values.

## Remaining parser diagnostics

All four programs are rejected at compile time; only the diagnostic category
differs from the suite contract.

| LO-3 invalid case | Current code | Expected code |
|---|---|---|
| `test_0` | `E_RESERVED_KEYWORD_AS_IDENTIFIER` | `E_PARSE_PHASE_OTHER` |
| `test_13` | `E_PARSE_PHASE_OTHER` | `E_RESERVED_KEYWORD_AS_IDENTIFIER` |
| `test_5` | `E_PARSE_PHASE_OTHER` | `E_NULL_LITERAL_RECEIVER` |
| `test_6` | `E_MALFORMED_CLASS_DECL` | `E_PARSE_PHASE_OTHER` |

These require parser diagnostic-precedence changes and are separate from the
WASM emitter.

## Verification environment

The run used Rust 1.87.0, LLVM 18.1.8, and the unchanged course `wasmrun` host
with Wasmtime 33.0.2 on macOS. The runtime archive targeted
`wasm32-unknown-unknown` with LTO disabled. All **91/91** compiler unit tests
pass. The grading entry points also work from a directory outside the repository:
`lo-check` preserves rejection status 1, `lo-compile` writes the requested path,
and `lo-wasmrun` preserves the program status.

The runner stores commands, exit statuses, stdout, stderr, generated assembly,
and linked modules under `compiler/target/conformance`; its `report.json` records
each result. LO-2 uses a separate procedural grammar and is outside this
emitter's current scope.
