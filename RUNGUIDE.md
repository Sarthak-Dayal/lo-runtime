# Run guide

How to build the LO compiler, compile and run programs with each back end, and what is known not to work. The repository README describes the layout; this file is only about running things.

## 1. Build

```sh
cargo build --release --manifest-path compiler/Cargo.toml     # compiler/target/release/lo-compiler
```

For quick iteration use `cargo build` (binary in `compiler/target/debug/`) and `cargo test --manifest-path compiler/Cargo.toml`.

## 2. Compiler modes

All modes are `lo-compiler MODE FILE.lo [...]`.

| Mode | What it does |
|---|---|
| `--check FILE.lo` | Front end only. Exit 0 if well-formed, exit 1 (error code on stderr) if not. |
| `--run FILE.lo` | Interpret the program. Its exit status is `Main.main`'s result (or the abort code). |
| `--emit-wasm FILE.lo` | Print the WASM assembly (Project 1 back end). |
| `--compile FILE.lo OUT.wasm` | Compile to a linked `.wasm` module (Project 1 back end). |
| `--dump-ir FILE.lo` | Print the readable IR for every function, as the native back end sees it. |
| `--emit-asm FILE.lo [OUT.s]` | Print (or write) the Intel-syntax x86-64 assembly. |
| `--native FILE.lo OUT [--runtime LIB.a]` | Assemble and link a native executable. `LIB.a` defaults to `$LO_RUNTIME`. |

Native modes take an allocation flag: the default is **linear scan**; `--spill-all` keeps every value in a stack slot, which is slower but a useful reference when you suspect the register allocator.

`--emit-asm` and `--dump-ir` run anywhere. `--native` needs a Linux x86-64 `as` and `gcc` (see §3), because the output is a Linux executable.

## 3. Running native code from macOS (Docker)

The native target is x86-64 Linux, so on an Apple Silicon Mac the assembling, linking and running happen in an amd64 container.

```sh
# one time: the image (same base as the course image, native toolchain plus Rust)
docker build --platform=linux/amd64 -t lo-native-dev -f compiler/scripts/native-dev.Dockerfile compiler/scripts

# build the Linux runtime archive into a Docker volume (nothing is written into the repo)
docker run --rm --platform linux/amd64 -v "$PWD":/work -v lo-rt-target:/cache \
    -e CARGO_TARGET_DIR=/cache -w /work/rust lo-native-dev cargo build --release

# compile on the host, link and run in the container
./compiler/target/debug/lo-compiler --emit-asm hello.lo hello.s
docker run --rm --platform linux/amd64 -v "$PWD":/work -v lo-rt-target:/cache -w /work lo-native-dev sh -c \
    'as -o /tmp/hello.o hello.s && gcc -static -o /tmp/hello /tmp/hello.o /cache/release/liblo_runtime.a -lm -lpthread -ldl && /tmp/hello; echo "exit=$?"'
```

`gcc` prints two linker warnings about `getpwuid_r` and `getaddrinfo` in static glibc. They come from Rust's standard library inside the runtime and are harmless.

On a Linux x86-64 machine with `as`, `gcc` and Rust installed, skip Docker:

```sh
cargo build --release --manifest-path rust/Cargo.toml
./compiler/target/release/lo-compiler --native hello.lo hello --runtime rust/target/release/liblo_runtime.a
./hello
```

The runtime reads `LO_HEAP_SIZE` (bytes) at start-up; a small value forces frequent collections.

## 4. Grading contracts

Both files are safe to `source` and define shell functions.

| File | Target | Functions |
|---|---|---|
| `p1-grading-contract.sh` | WASM | `lo-build`, `lo-check`, `lo-compile SRC OUT.wasm`, `lo-wasmrun` |
| `p2-grading-contract.sh` | native x86-64 | `lo-build`, `lo-check`, `lo-compile SRC OUT` (an executable), `lo-run OUT` |

`p2-grading-contract.sh` links against `$LO_RUNTIME` (default `rust/target/release/liblo_runtime.a`, which its `lo-build` produces), so a different `liblo_runtime.a` can be linked by setting that variable. No instructor template for Project 2 was available, so `lo-run` and the argument conventions follow the Project 1 file; adjust if the grader's names differ.

```sh
source p2-grading-contract.sh
lo-build
lo-compile hello.lo /tmp/artifacts/hello
lo-run /tmp/artifacts/hello
```

## 5. Test scripts

Both need Docker running and the `lo-native-dev` image.

| Script | What it checks |
|---|---|
| `compiler/scripts/native-diff.sh [FILE.lo ...]` | For each program (default: `tests/lo_programs/` and `compiler/examples/`), stdout, stderr and exit status of the interpreter against native code built with `--spill-all` and with linear scan. `FILE.input.txt` is fed to stdin; `FILE.expected.out`, `.expected.err`, `.expected.code` override the interpreter where it is known to be wrong. |
| `compiler/scripts/run-suite.sh [--ref REF] [SUITE_DIR]` | A conformance suite in the header-comment format (valid, invalid and runtime-abort tests). Defaults to the team suite next to this repository, read from `origin/p2-tests`; the suite repository is never modified. |

Programs under `compiler/examples/` cover register pressure, calls with values live across them, eight-argument calls (two on the stack), recursion, objects and dispatch, GC stress, strings (UTF-8), casts and `instanceof`, stdin, total division, and every runtime abort (exit codes 101, 102, 110, 111, 112, 120, 137).

## 6. Known failures and limitations

No test currently fails: all 21 example programs match the interpreter under both strategies, and the 11 team suite tests pass. What follows is what has **not** been exercised or is known to be rough.

- **Only the Rust runtime has been linked.** The Zig and C++ runtimes stub the string operations and the cast functions, so programs that use them cannot run natively against those skeletons.
- **Trimmed container.** `lo-native-dev` has the course image's native toolchain (same Debian base, `as`, `gcc`, Rust) but not its other layers. The full course image has not been used for a test run.
- **Instructor grading runtime unavailable.** Output matches the Rust skeleton runtime and the interpreter; behavior against the instructor's runtime is unverified.
- **Interpreter bug on prebound assignment.** `--run` exits 101 on a program that assigns to `out` or `err` (the type checker accepts it). Native code follows the language specification; `compiler/examples/prebound.*` carries the hand-written expectation.
- **`runtime-abi.md` §4.4 is stale.** It lists the string operations and `lo_cast_check` / `lo_instanceof` as stubs; the Rust runtime implements them.
- **Untested:** very deep recursion (stack exhaustion), programs with more than eight arguments, and large programs beyond the examples.
- **Performance:** no optimization passes. A function that makes a call is allocated only five registers (`rbx`, `r12`–`r15`) and spills the rest; leaf functions use nine.
- **Duplicated layout code.** Class layout and static data exist both in lowering and in `layout.rs` / `ir/statics.rs`. They agree (one pointer-sized slot per field); consolidating them is planned.
- **Linux only.** `--native` produces x86-64 Linux executables and cannot run on macOS without the container.
