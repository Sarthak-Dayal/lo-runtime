# LO Runtime Skeletons

This is the runtime-skeleton mono-repo for **CS 378H — Compilers** (UT Austin,
Fall 2026). It provides three parallel implementations of the LO runtime — one
each in **Rust**, **Zig**, and **modern C++** — that share a single C ABI,
specified in [`runtime-abi.md`](runtime-abi.md). In the course's P3 project, each
team picks one skeleton, fills in the stubbed entry points (garbage collection,
string operations, type operations), and builds a working LO runtime on top. The
parts that don't change between projects are pre-implemented so teams can focus on
the interesting work.

## The three skeletons

Each skeleton is an independently buildable and testable library that produces
both a native static library (for AOT-linked `x86_64-linux` programs) and a WASM
module (imported by student WASM modules). All three implement the exact same
ABI, so an LO program linked against any one of them behaves identically.

- **[`rust/`](rust/)** — Cargo project producing `staticlib`, `cdylib`, and
  `rlib`. `unsafe` is contained at the FFI boundary and in the bump allocator.
  Stubs use `unimplemented!()`. Build and test with `cargo`.
- **[`zig/`](zig/)** — `build.zig` project producing a static library and a WASM
  module. ABI-visible types are `extern struct`; every public entry point is an
  `export fn`. Stubs use `@panic("not implemented")`. Build and test with `zig
  build`.
- **[`cpp/`](cpp/)** — CMake project producing a static library (and a WASM
  target where the toolchain is available). The exported surface is `extern "C"`;
  the implementation uses modern C++20 freely. Stubs use `throw
  std::runtime_error("not implemented")`. Build and test with `cmake` + `ctest`.

The three are kept readable as parallel implementations: same module names
(translated to each language's conventions), same function organization. A reader
who has studied one should find the other two predictable.

## Shared test corpus

The [`tests/`](tests/) directory at the repo root holds **language-agnostic** LO
program fixtures ([`tests/lo_programs/`](tests/lo_programs/)) and their expected
outputs ([`tests/expected/`](tests/expected/)). These are not auto-runnable until
a student's LO compiler exists — they document the conformance shape the eventual
harness will exercise. What verifies each skeleton *today* is its own
language-native unit-test suite (under `rust/tests/`, `zig`'s `zig build test`,
and `cpp/tests/`), which drives the provided entry points directly through the C
ABI.

## Repository layout

```
lo-runtime/
├── README.md            # this file
├── runtime-abi.md       # the shared ABI specification — read this first
├── LICENSE
├── scripts/
│   └── install-hooks.sh # installs the pre-commit hook (fmt + lint + secrets)
├── tests/
│   ├── lo_programs/     # LO programs that exercise the runtime
│   └── expected/        # expected outputs (language-agnostic)
├── rust/                # Rust skeleton (Cargo)
├── zig/                 # Zig skeleton (build.zig)
├── cpp/                 # C++ skeleton (CMake)
└── tools/
    └── wasmrun/         # WASM host harness (course toolchain), see below
```

## Picking a skeleton

All three reach the same destination; the choice is about which language your
team wants to write a garbage collector and string routines in.

- **Pick Rust** if you want the strongest compile-time guarantees and don't mind
  reasoning about `unsafe` at the FFI and allocator boundaries. The borrow checker
  helps most once you start writing the collector. Best-documented `wasm32`
  story of the three.
- **Pick Zig** if you want C-like control with no hidden allocations and a small,
  explicit standard library. `extern struct` makes the ABI layout transparent, and
  `comptime` is handy for the descriptor tables. WASM is a first-class target.
- **Pick C++** if your team is already fluent in modern C++ and wants RAII,
  templates, and a mature tooling ecosystem (Catch2, sanitizers, clang-tidy). The
  C ABI is a thin `extern "C"` veneer over an idiomatic C++ implementation.

If your team has no strong preference, Rust is the most-trodden path for this
kind of runtime work and has the gentlest WASM setup.

## The `wasmrun` host harness

[`tools/wasmrun/`](tools/wasmrun/) is a standalone crate providing `wasmrun`,
the WASM host harness for the course toolchain: it supplies the runtime's
`host` I/O imports (not WASI) via the [wasmtime](https://wasmtime.dev/)
embedding API and runs a linked LO module, per the P1 handout. Build and
install it with:

```
cargo install --path tools/wasmrun
```

You shouldn't normally need to do this yourself — the course Dockerfile
installs `wasmrun` automatically as part of the standard build environment.

## Getting started

1. Read [`runtime-abi.md`](runtime-abi.md) — it is the contract every skeleton
   implements and the thing your compiler emits calls against.
2. Install the toolchain for your chosen skeleton (see that skeleton's own
   `README.md`).
3. Run `bash scripts/install-hooks.sh` once to enable the pre-commit hook.
4. Build and test your skeleton; everything provided should pass out of the box.
5. In P3, implement the stubbed entry points (GC, strings, casts). The
   instructor's acceptance baseline is Cheney's semispace with all shared tests
   passing — see [`runtime-abi.md`](runtime-abi.md) §4.5.
