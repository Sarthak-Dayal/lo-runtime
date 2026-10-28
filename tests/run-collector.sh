#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

cargo test --manifest-path rust/Cargo.toml
LO_HEAP_SIZE=1024 LO_GC_POISON=1 cargo test --manifest-path rust/Cargo.toml --test collector_probe
cargo build --manifest-path rust/Cargo.toml --release --lib
cargo build --manifest-path rust/Cargo.toml --release --lib --target wasm32-unknown-unknown
probe_dir=$(mktemp -d)
trap 'rm -rf "$probe_dir"' EXIT
rustc --edition=2021 --target wasm32-unknown-unknown --crate-type cdylib \
    rust/tests/collector_probe.rs \
    --extern lo_runtime=rust/target/wasm32-unknown-unknown/release/liblo_runtime.rlib \
    -L dependency=rust/target/wasm32-unknown-unknown/release/deps \
    -O -o "$probe_dir/collector.wasm"
"${WASMRUN:-wasmrun}" "$probe_dir/collector.wasm"
echo 'Collector tests and native/WASM release builds passed.'
