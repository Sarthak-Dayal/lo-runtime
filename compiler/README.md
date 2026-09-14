# LO compiler

The compiler lowers the LO-3/LO-4 typed AST to LLVM WASM assembly, assembles it
with `llvm-mc`, and links it with the Rust runtime using `wasm-ld`. It constructs
Main and the I/O preamble, emits methods and constructors, and publishes GC roots
around calls. See the [production index](../planning/wasm-production-index.md)
for the functions implementing each grammar rule.

This WIP is stacked on [type-checker PR #5](https://github.com/Sarthak-Dayal/lo-runtime/pull/5).
The [test report](../planning/wasm-test-results.md) records **75/102 course cases
passing**, identifies the frontend/runtime failures, and lists verification limits.

From the repository root, with Rust/cargo, the `wasm32-unknown-unknown` target,
`llvm-mc`, `wasm-ld`, and the course `wasmrun` on PATH:

```sh
source p1-grading-contract.sh
lo-build
lo-check compiler/tests/fixtures/return-42.lo
lo-compile compiler/tests/fixtures/return-42.lo /tmp/return-42.wasm
lo-wasmrun /tmp/return-42.wasm
```

The last command exits with 42. The grading functions preserve rejection status 1
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

Run the external LO-3/LO-4 corpus, including `contributed-tests`, and the six
emitter regressions separately:

```sh
python3 compiler/tests/conformance.py --suite /path/to/lo-testing
python3 compiler/tests/conformance.py --suite compiler/tests/fixtures --artifacts compiler/target/wasm-regressions
cargo test --manifest-path compiler/Cargo.toml
```

`lo-testing` supplies test cases and expectations; its README says the instructor's
grading harness is not included. Our Python runner checks frontend acceptance,
assembly, linking, and execution through the unchanged course host. It matches expected process status,
stdout bytes, and abort-message substrings. Each run keeps `report.json` and
per-stage artifacts under its artifact directory. Failure-layer labels are
initial localization hints; inspect the saved diagnostics before assigning a fix.
LO-2's procedural grammar is outside this emitter's scope and is not counted.

After `lo-build`, run the encoding and ABI checks with Node:

```sh
node compiler/tests/wasm.mjs
```

This uses the full emitter and runtime to check exact i32 results for 42, 64,
2147483647, and addition. It also checks frontend rejections, memory-stack
restoration, and non-null empty String defaults. The 89 Rust tests cover the
lexer, parser, and preamble; the six LO regressions cover moving GC, reference
returns, evaluation order, branch depths, arithmetic, and hoisted locals.
