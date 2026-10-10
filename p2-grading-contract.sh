#!/usr/bin/env bash
# Course grading entry points for Project 2 (native x86-64 Linux executables).
# Sourcing this file does not build or print. The Project 1 entry points, which
# target WASM, stay in p1-grading-contract.sh.
#
#   source p2-grading-contract.sh
#   lo-build                                      # once
#   lo-check   prog.lo                            # front end only (exit 0 / 1)
#   lo-compile prog.lo /tmp/artifacts/prog        # native executable at $2
#   lo-run     /tmp/artifacts/prog                # run it; stdin and exit status pass through
#
# LO_RUNTIME names the runtime archive to link against; it defaults to the Rust
# skeleton built by lo-build. Set it to link a different liblo_runtime.a.
set -uo pipefail

LO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LO_RUNTIME="${LO_RUNTIME:-$LO_ROOT/rust/target/release/liblo_runtime.a}"

lo-build() {
    cargo build --release --manifest-path "$LO_ROOT/compiler/Cargo.toml" || return 1
    cargo build --release --manifest-path "$LO_ROOT/rust/Cargo.toml"
}

lo-check() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --check "$1"
}

lo-compile() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --native "$1" "$2" --runtime "$LO_RUNTIME"
}

lo-run() {
    "$1"
}
