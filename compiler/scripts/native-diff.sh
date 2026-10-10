#!/usr/bin/env bash
# Differential test of the native back end.
#
# For each LO program, compares stdout, stderr and the exit status of
#   * the expected result (the interpreter, or the FILE.expected.* sidecars),
#   * native code with --spill-all,
#   * native code with the default linear-scan allocation,
# where the native programs are assembled, linked and run in an amd64 Linux
# container (Docker Desktop must be running).
#
#   usage: compiler/scripts/native-diff.sh [FILE.lo ...]
#   default: tests/lo_programs/*.lo compiler/examples/*.lo
#
# FILE.input.txt next to a program is fed to stdin (empty otherwise).
# FILE.expected.out / .expected.err / .expected.code override the interpreter's
# stdout / stderr / exit status, for programs the interpreter gets wrong.
set -u
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT" || exit 2
IMAGE=lo-native-dev
VOLUME=lo-rt-target
BIN=compiler/target/debug/lo-compiler
WORK=compiler/target/native-diff

command -v docker >/dev/null && docker info >/dev/null 2>&1 \
    || { echo "Docker is not running. Start Docker Desktop and retry." >&2; exit 2; }
docker image inspect "$IMAGE" >/dev/null 2>&1 \
    || docker build --platform=linux/amd64 -t "$IMAGE" -f compiler/scripts/native-dev.Dockerfile compiler/scripts \
    || exit 2
cargo build --quiet --manifest-path compiler/Cargo.toml || exit 2
docker run --rm --platform linux/amd64 -v "$ROOT":/work -v "$VOLUME":/cache \
    -e CARGO_TARGET_DIR=/cache -w /work/rust "$IMAGE" cargo build --release --quiet || exit 2

if [ $# -gt 0 ]; then FILES=("$@"); else FILES=(tests/lo_programs/*.lo compiler/examples/*.lo); fi
rm -rf "$WORK"; mkdir -p "$WORK"
: > "$WORK/names.txt"

for f in "${FILES[@]}"; do
    n=$(basename "$f" .lo); d="$WORK/$n"; mkdir -p "$d"; echo "$n" >> "$WORK/names.txt"
    base="${f%.lo}"
    if [ -f "$base.input.txt" ]; then cp "$base.input.txt" "$d/input.txt"; else : > "$d/input.txt"; fi
    "$BIN" --run "$f" > "$d/expected.out" 2> "$d/expected.err" < "$d/input.txt"; echo $? > "$d/expected.code"
    for ext in out err code; do [ -f "$base.expected.$ext" ] && cp "$base.expected.$ext" "$d/expected.$ext"; done
    "$BIN" --emit-asm "$f" "$d/spill.s" --spill-all 2> "$d/spill.cerr" || echo COMPILE-FAIL > "$d/spill.status"
    "$BIN" --emit-asm "$f" "$d/linear.s" 2> "$d/linear.cerr" || echo COMPILE-FAIL > "$d/linear.status"
done

docker run --rm --platform linux/amd64 -v "$ROOT":/work -v "$VOLUME":/cache -w /work "$IMAGE" sh -c '
W=compiler/target/native-diff
for n in $(cat $W/names.txt); do
  for s in spill linear; do
    d=$W/$n
    [ -f $d/$s.status ] && continue
    if as -o /tmp/$n.$s.o $d/$s.s 2>$d/$s.as.err && \
       gcc -static -o /tmp/$n.$s /tmp/$n.$s.o /cache/release/liblo_runtime.a -lm -lpthread -ldl 2>$d/$s.ld.err; then
      timeout 60 /tmp/$n.$s <$d/input.txt >$d/$s.out 2>$d/$s.err; echo $? > $d/$s.code
    else echo BUILD-FAIL > $d/$s.status; fi
  done
done'

fail=0
printf '%-22s %-14s %-14s\n' program spill-all linear-scan
while read -r n; do
    d="$WORK/$n"; row=""
    for s in spill linear; do
        if [ -f "$d/$s.status" ]; then r=$(cat "$d/$s.status")
        elif ! cmp -s "$d/$s.out" "$d/expected.out"; then r="FAIL(stdout)"
        elif ! cmp -s "$d/$s.err" "$d/expected.err"; then r="FAIL(stderr)"
        elif [ "$(cat "$d/$s.code")" != "$(cat "$d/expected.code")" ]; then
            r="FAIL(exit $(cat "$d/$s.code")!=$(cat "$d/expected.code"))"
        else r=PASS; fi
        [ "$r" = PASS ] || fail=1
        row="$row$(printf '%-14s ' "$r")"
    done
    printf '%-22s %s\n' "$n" "$row"
done < "$WORK/names.txt"
[ $fail = 0 ] && echo "all programs match" || echo "SOME PROGRAMS DIFFER (see $WORK/<program>/)"
exit $fail
