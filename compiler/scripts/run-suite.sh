#!/usr/bin/env bash
# Runs a conformance suite (the header-contract format) against the native back end.
#
#   usage: compiler/scripts/run-suite.sh [--ref GIT_REF] [SUITE_DIR]
#   SUITE_DIR  default: ../lo_group_6_testing/lo_group_6_test_cases (next to this repo)
#   --ref      export that git ref of SUITE_DIR into a scratch directory instead of
#              reading its working tree (default: origin/p2-tests when it exists,
#              which carries the P1 and P2 tests). SUITE_DIR itself is never modified.
#
# Contract (REFERENCE.md in the suite), read from each test's leading // comments:
#   valid   : "main method return value: N" -> exit status N (mod 256);
#             FILE.expected.out is byte-matched, FILE.input.txt is piped to stdin
#   abort   : "expected exit code: N" and "expected stderr substring: S"
#   invalid : "expected compile error: E_CODE" -> `--check` must exit 1 with E_CODE
#             in stderr
# Valid and abort tests are compiled natively (--spill-all and the default linear
# scan) and run in the amd64 container. Valid tests must also pass `--check`.
set -u
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT" || exit 2
IMAGE=lo-native-dev
VOLUME=lo-rt-target
BIN=compiler/target/debug/lo-compiler
WORK=compiler/target/run-suite

REF=""; SUITE="$ROOT/../lo_group_6_testing/lo_group_6_test_cases"
while [ $# -gt 0 ]; do
    case "$1" in
        --ref) REF="$2"; shift 2 ;;
        *) SUITE="$1"; shift ;;
    esac
done
[ -d "$SUITE" ] || { echo "suite directory not found: $SUITE" >&2; exit 2; }
if [ -z "$REF" ] && git -C "$SUITE" rev-parse --verify -q origin/p2-tests >/dev/null 2>&1; then
    REF=origin/p2-tests
fi

command -v docker >/dev/null && docker info >/dev/null 2>&1 \
    || { echo "Docker is not running. Start Docker Desktop and retry." >&2; exit 2; }
docker image inspect "$IMAGE" >/dev/null 2>&1 \
    || docker build --platform=linux/amd64 -t "$IMAGE" -f compiler/scripts/native-dev.Dockerfile compiler/scripts \
    || exit 2
cargo build --quiet --manifest-path compiler/Cargo.toml || exit 2
docker run --rm --platform linux/amd64 -v "$ROOT":/work -v "$VOLUME":/cache \
    -e CARGO_TARGET_DIR=/cache -w /work/rust "$IMAGE" cargo build --release --quiet || exit 2

rm -rf "$WORK"; mkdir -p "$WORK/suite"
if [ -n "$REF" ]; then
    git -C "$SUITE" archive "$REF" | tar -x -C "$WORK/suite" || exit 2
    echo "suite: $SUITE @ $REF"
else
    cp -R "$SUITE"/. "$WORK/suite"/; echo "suite: $SUITE (working tree)"
fi

header() { sed -n "s|^// *$2: *||p" "$1" | head -1; }

: > "$WORK/names.txt"
fail=0
declare -a ROWS=()
while IFS= read -r f; do
    rel=${f#"$WORK/suite/"}
    case "$rel" in */Valid/*|*/ValidPrograms/*) kind=valid ;; */Invalid/*|*/InvalidPrograms/*) kind=invalid ;;
                   */RuntimeAbort/*) kind=abort ;; *) continue ;; esac
    n=$(echo "${rel%.lo}" | tr '/' '_'); d="$WORK/$n"; mkdir -p "$d"
    base="${f%.lo}"
    if [ "$kind" = invalid ]; then
        code=$(header "$f" "expected compile error")
        "$BIN" --check "$f" > /dev/null 2> "$d/check.err"; status=$?
        if [ $status -ne 1 ]; then r="FAIL(check exit $status)"
        elif [ -n "$code" ] && ! grep -qF -- "$code" "$d/check.err"; then r="FAIL(no $code)"
        else r=PASS; fi
        [ "$r" = PASS ] || fail=1
        ROWS+=("$(printf '%-52s %-8s %-13s %s' "$rel" invalid "$r" '-')"); continue
    fi
    if ! "$BIN" --check "$f" > /dev/null 2> "$d/check.err"; then
        fail=1; ROWS+=("$(printf '%-52s %-8s %-13s %s' "$rel" "$kind" "FAIL(--check)" '-')"); continue
    fi
    if [ "$kind" = valid ]; then
        want=$(( $(header "$f" "main method return value") & 255 )); sub=""
    else
        want=$(header "$f" "expected exit code"); sub=$(header "$f" "expected stderr substring")
    fi
    echo "$want" > "$d/want.code"; printf '%s' "$sub" > "$d/want.sub"
    [ -f "$base.input.txt" ] && cp "$base.input.txt" "$d/input.txt" || : > "$d/input.txt"
    [ -f "$base.expected.out" ] && cp "$base.expected.out" "$d/want.out"
    echo "$n $kind $rel" >> "$WORK/names.txt"
    "$BIN" --emit-asm "$f" "$d/spill.s" --spill-all 2> "$d/spill.cerr" || echo COMPILE-FAIL > "$d/spill.status"
    "$BIN" --emit-asm "$f" "$d/linear.s" 2> "$d/linear.cerr" || echo COMPILE-FAIL > "$d/linear.status"
done < <(find "$WORK/suite" -name '*.lo' | sort)

docker run --rm --platform linux/amd64 -v "$ROOT":/work -v "$VOLUME":/cache -w /work "$IMAGE" sh -c '
W=compiler/target/run-suite
while read -r n kind rel; do
  for s in spill linear; do
    d=$W/$n
    [ -f $d/$s.status ] && continue
    if as -o /tmp/$n.$s.o $d/$s.s 2>$d/$s.as.err && \
       gcc -static -o /tmp/$n.$s /tmp/$n.$s.o /cache/release/liblo_runtime.a -lm -lpthread -ldl 2>$d/$s.ld.err; then
      timeout 60 /tmp/$n.$s <$d/input.txt >$d/$s.out 2>$d/$s.err; echo $? > $d/$s.code
    else echo BUILD-FAIL > $d/$s.status; fi
  done
done < $W/names.txt'

while read -r n kind rel; do
    d="$WORK/$n"; cols=""
    for s in spill linear; do
        if [ -f "$d/$s.status" ]; then r=$(cat "$d/$s.status")
        elif [ "$(cat "$d/$s.code")" != "$(cat "$d/want.code")" ]; then r="FAIL(exit $(cat "$d/$s.code")!=$(cat "$d/want.code"))"
        elif [ -f "$d/want.out" ] && ! cmp -s "$d/$s.out" "$d/want.out"; then r="FAIL(stdout)"
        elif [ -s "$d/want.sub" ] && ! grep -qF -- "$(cat "$d/want.sub")" "$d/$s.err"; then r="FAIL(stderr)"
        else r=PASS; fi
        [ "$r" = PASS ] || fail=1
        cols="$cols$(printf '%-13s ' "$r")"
    done
    ROWS+=("$(printf '%-52s %-8s %s' "$rel" "$kind" "$cols")")
done < "$WORK/names.txt"

printf '%-52s %-8s %-13s %-13s\n' test kind spill-all linear-scan
printf '%s\n' "${ROWS[@]}" | sort
[ $fail = 0 ] && echo "suite passes" || echo "SUITE FAILURES (details under $WORK/<test>/)"
exit $fail
