# LO compiler

The compiler lowers the LO-3/LO-4 typed AST to LLVM WASM assembly, assembles it
with `llvm-mc`, and links it with the Rust runtime using `wasm-ld`. It constructs
Main and the I/O preamble, emits methods and constructors, and publishes GC roots
around calls. See the [production index](../planning/wasm-production-index.md)
for the functions implementing each grammar rule.

This WIP is stacked on [type-checker PR #5](https://github.com/Sarthak-Dayal/lo-runtime/pull/5).
The [test report](../planning/wasm-test-results.md) records **97/102 course cases
passing** with the complete local test runtime (**98/102** after accounting for
one stale test that still treats the newly reserved `lo_` prefix as valid). It
identifies the four remaining parser diagnostic mismatches.

From the repository root, with Rust/cargo, the `wasm32-unknown-unknown` target,
`llvm-mc`, `wasm-ld`, and the course `wasmrun` on PATH:

```sh
source p1-grading-contract.sh
lo-build
lo-check ../lo-testing/LO-3/ValidPrograms/test_15.lo
lo-compile ../lo-testing/LO-3/ValidPrograms/test_15.lo /tmp/test_15.wasm
lo-wasmrun /tmp/test_15.wasm
```

The last command exits with 10. The grading functions preserve rejection status 1
and program exit codes. `lo-check` runs only the frontend and writes no files.
`lo-compile` writes the module to its second argument; on failure it reports the
stage and retains intermediate artifacts. `LLVM_MC`, `WASM_LD`, and
`LO_RUNTIME_ARCHIVE` can override tool/archive paths. The compiler itself exposes:

```sh
compiler/target/release/lo-compiler --check program.lo
compiler/target/release/lo-compiler --emit-wasm program.lo > program.s
compiler/target/release/lo-compiler --compile program.lo program.wasm
```

`check_program` owns preamble injection and supplies both `TypedProgram` and
`ClassTable`. The emitter uses the table's field order, resolved method owners,
and stable vtable slots. Source bodies are visited once; buffering their output
lets scratch-local declarations and frame sizes precede the instructions.

Run the external LO-3/LO-4 corpus, including `contributed-tests`, from a checkout
next to this repository:

```sh
python3 scripts/test-conformance.py --suite ../lo-testing/
```

The runner checks frontend acceptance,
assembly, linking, and execution through the unchanged course host. It matches expected process status,
stdout bytes, and abort-message substrings. Each run keeps `report.json` and
per-stage artifacts under its artifact directory. Failure-layer labels are
initial localization hints; inspect the saved diagnostics before assigning a fix.
LO-2's procedural grammar is outside this emitter's scope and is not counted.

The 89 Rust tests cover the lexer, parser, and preamble.
