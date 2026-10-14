#!/usr/bin/env bash
# Course grading entry points for Project 2, including P1 WASM regression.
# Source this file in Bash; sourcing does not build or print.
#
#   source p2-grading-contract.sh
#   lo-build                                      # once
#   lo-check   prog.lo                            # front end only (exit 0 / 1)
#   lo-compile prog.lo /tmp/artifacts/prog.wasm   # WASM regression artifact
#   lo-wasmrun /tmp/artifacts/prog.wasm           # stdin and exit status pass through
#   lo-emit-asm prog.lo                          # native assembly on stdout
#   lo-dump-ir  prog.lo                          # verified IR on stdout
#
# LO_RUNTIME names the runtime archive to link against; it defaults to the Rust
# skeleton built by lo-build. Set it to link a different liblo_runtime.a.
set -uo pipefail

LO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LO_RUNTIME="${LO_RUNTIME:-$LO_ROOT/rust/target/release/liblo_runtime.a}"

lo-build() {
    cargo build --release --manifest-path "$LO_ROOT/compiler/Cargo.toml" || return 1
    cargo build --release --manifest-path "$LO_ROOT/rust/Cargo.toml" || return 1
    cargo build --release --target wasm32-unknown-unknown --lib \
        --config profile.release.lto=false --manifest-path "$LO_ROOT/rust/Cargo.toml"
}

lo-check() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --check "$1"
}

lo-compile() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --compile "$1" "$2"
}

lo-run() {
    "$1"
}

lo-wasmrun() {
    wasmrun "$1"
}

lo-emit-asm() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --emit-asm "$@"
}

lo-dump-ir() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --dump-ir "$1"
}
