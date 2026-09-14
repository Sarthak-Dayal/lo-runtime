#!/usr/bin/env bash
# Course grading entry points. Sourcing this file does not build or print.
LO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

lo-build() {
    cargo build --release --manifest-path "$LO_ROOT/compiler/Cargo.toml" || return 1
    # Emit WASM objects, not LLVM bitcode tied to rustc's LLVM version.
    cargo build --release --target wasm32-unknown-unknown --lib \
        --config profile.release.lto=false --manifest-path "$LO_ROOT/rust/Cargo.toml"
}

lo-check() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --check "$1"
}

lo-compile() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --compile "$1" "$2"
}

lo-wasmrun() {
    wasmrun "$1"
}
