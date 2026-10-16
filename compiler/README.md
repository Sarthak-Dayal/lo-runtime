# LO compiler

**Team:** Group 6 — Neha Reddy, Sarthak Dayal, Jay Shim, Rahik Sikder
**Toolchain language:** Rust

One binary, `lo-compiler`, with four back ends over a shared front end (lexer,
parser, type checker, I/O-preamble injection):

| Mode | Back end | Output |
|---|---|---|
| `--check` | front end only | nothing; exit 0 or 1 |
| `--run` | tree-walking interpreter | program's stdout; exit status is `main`'s return value or the abort code |
| `--emit-wasm` / `--compile` | P1 WASM emitter | LLVM WASM assembly / linked `.wasm` |
| `--dump-ir` / `--emit-asm` / `--native` | P2 native pipeline | verified IR / x86-64 assembly / linked executable |

The native pipeline lowers the typed AST to a TAC-style IR (with explicit GC roots in
shadow-stack frame slots), verifies it, selects x86-64 instructions, allocates
registers (linear scan by default, spill-everything as a fallback), and emits
GNU assembler text. See [`../design_doc/p2-design.pdf`](../design_doc/p2-design.pdf)
for the design note and [`../planning/wasm-production-index.md`](../planning/wasm-production-index.md)
for the P1 WASM emitter.

## Building

From the repository root, inside the course container (x86-64 Linux; on ARM Macs
that is the Rosetta-accelerated amd64 image). Required: Rust/cargo with the
`wasm32-unknown-unknown` target, `as` and `gcc` (native linking), `llvm-mc`,
`wasm-ld`, and the course `wasmrun` (WASM mode only).

```sh
source p2-grading-contract.sh
lo-build        # builds compiler/ and the Rust runtime (native .a and wasm32 .a)
```

`lo-build` leaves the compiler at `compiler/target/release/lo-compiler` and the
native runtime at `rust/target/release/liblo_runtime.a`.

## Running

### Native (Project 2)

```sh
lo-emit-asm prog.lo                       # assembly on stdout
lo-dump-ir  prog.lo                       # verified IR for every function on stdout
```

or, directly:

```sh
lo-compiler --dump-ir  FILE.lo
lo-compiler --emit-asm FILE.lo [OUT.s]    # stdout if OUT.s is omitted
lo-compiler --native   FILE.lo OUTPUT [--runtime LIB.a]
./OUTPUT                                  # stdin/stdout/exit status are the program's
```

- `--native` assembles with `as` and links statically with `gcc` against the
  runtime archive given by `--runtime` or `$LO_RUNTIME` (the `lo-*` shell
  functions default `LO_RUNTIME` to `rust/target/release/liblo_runtime.a`).
- `--spill-all` or `--linear-scan` (default) may be appended to any native mode
  to choose the register allocator. Spill-all is kept as a differential-testing
  oracle for the allocator.

### WASM and interpreter (Project 1, still supported)

```sh
lo-check   prog.lo                        # exit 0 silently, or 1 with a diagnostic
lo-compile prog.lo /tmp/prog.wasm
lo-wasmrun /tmp/prog.wasm                 # stdin and exit status pass through
lo-compiler --run prog.lo                 # interpreter
```

`lo-compiler --emit-wasm prog.lo` prints the LLVM WASM assembly. `LLVM_MC`,
`WASM_LD`, and `LO_RUNTIME_ARCHIVE` override tool and archive paths for WASM mode.

## Testing

```sh
cargo test --release --manifest-path compiler/Cargo.toml   # 183 unit tests
python3 scripts/test-conformance.py --suite ../lo-testing/ # WASM conformance
```

The conformance runner checks frontend acceptance, assembly, linking, and
execution through the course host, matching expected exit status, stdout bytes,
and abort-message substrings. Each run keeps `report.json` and per-stage
artifacts. LO-2's procedural grammar is outside this compiler's scope and is
not counted.

Differential check (interpreter vs. WASM vs. native on the same program):
TODO(team): name the script/command used, if one is checked in.

## Known failures

The numbers below are what each report last measured; re-run before submitting.

**WASM back end** (from [`../planning/wasm-test-results.md`](../planning/wasm-test-results.md)):
97/102 course cases pass as the suite was written, 98/102 once the stale test
below is accounted for.

- `LO-3/ValidPrograms/contributed-tests-19.lo`: declares a method named
  `lo_alloc` and expects acceptance, but the instructor has since reserved the
  `lo_` prefix, so the compiler correctly rejects it with
  `E_RESERVED_VARIABLE_NAME`. The test is stale, not the compiler.
- Four `InvalidPrograms` cases (including `test_0` and `test_13`) are rejected
  at compile time but with a different parser diagnostic code than the header
  requests; `test_0` and `test_13` swap `E_RESERVED_KEYWORD_AS_IDENTIFIER` and
  `E_PARSE_PHASE_OTHER`. The report lists all four.
- Against the committed Rust runtime with its assignment stubs left in place,
  13 further runtime-dependent cases fail. The grading environment links the
  instructor's complete runtime.
