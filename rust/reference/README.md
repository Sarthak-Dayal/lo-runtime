# Reference collector

`cheney.rs` and `semispace_alloc.rs` preserve the provided semispace runtime
before the Lisp-2 integration. They are reference source, not compiled modules.
The active Rust collector is `src/gc.rs`.
