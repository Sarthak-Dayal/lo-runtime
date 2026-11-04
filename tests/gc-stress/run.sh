#!/usr/bin/env bash
# Run the contributed GC stress tests the way the P3 grader runs its stress stage:
# WASM at the default heap, native at the default heap, and native again with
# LO_HEAP_SIZE set to the test's `// gc stress heap:` value.
#
#   bash tests/gc-stress/run.sh [wasm|native|all]    # default: all; run lo-build first
#
# Needs the course container's tools: wasmrun, as, cc.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$here/../../p3-grading-contract.sh"
target="${1:-all}"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
failed=0

check() { # check <label> <command...>: exit 0 and stdout equal to <stem>.expected.out
    "${@:2}" <"$input" >"$work/out" 2>"$work/err"
    local rc=$?
    if [ "$rc" = 0 ] && cmp -s "$work/out" "$stem.expected.out"; then return 0; fi
    echo "FAIL $name ($1): exit $rc $(head -c 200 "$work/err")"; return 1
}

for src in "$here"/LO-*/ValidPrograms/*.lo; do
    stem="${src%.lo}"; name="$(basename "$stem")"; ok=1
    input=/dev/null; [ -f "$stem.input.txt" ] && input="$stem.input.txt"
    heap="$(sed -n 's|^// gc stress heap: ||p' "$src")"
    if [ "$target" != native ]; then
        lo-compile "$src" "$work/p.wasm" && check wasm lo-wasmrun "$work/p.wasm" || ok=0
    fi
    if [ "$target" != wasm ]; then
        if lo-emit-asm "$src" "$work/p.s" && as --64 -o "$work/p.o" "$work/p.s" &&
            cc -no-pie -o "$work/p" "$work/p.o" "$LO_RUNTIME_NATIVE_LIB" -lpthread -ldl -lm; then
            check native "$work/p" || ok=0
            check "native @ LO_HEAP_SIZE=$heap" env LO_HEAP_SIZE="$heap" "$work/p" || ok=0
        else
            echo "FAIL $name (native build)"; ok=0
        fi
    fi
    [ "$ok" = 1 ] && echo "PASS $name" || failed=1
done
exit "$failed"
