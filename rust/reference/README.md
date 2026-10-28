# Reference collector

`cheney.rs` and `semispace_alloc.rs` preserve the provided semispace runtime
before the Lisp-2 integration. Lisp-2 remains the default. The optional
`reference-cheney` Cargo feature selects both reference modules instead, without
initializing Lisp-2 scratch storage. Only one collector is compiled at a time.

Run native tests with:

```sh
cargo test --manifest-path rust/Cargo.toml --features reference-cheney
```

Run native tests, both release builds, and the shared WASM stress probe with:

```sh
WASMRUN=/path/to/wasmrun bash tests/run-collector.sh --features reference-cheney
```

Omit the feature to test Lisp-2. Rebuild without it before submission. Shared
tests check runtime behavior; each collector keeps its own phase tests. Cheney
has only half the configured heap available for live objects, so the Lisp-2
full-capacity test is intentionally not compiled for Cheney.
