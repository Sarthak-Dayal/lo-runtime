#!/usr/bin/env bash
# =============================================================================
# CS 378H — P2 grading contract
# =============================================================================
# This file is the interface between your submission and the autograder.
# Every submission builds and runs differently, so instead of forcing a common
# starter layout, we ask you to fill in a few shell functions that tell the
# grader how to drive *your* compiler.
#
# Submit this file, filled in, at the TOP LEVEL of your repository, keeping the
# name `p2-grading-contract.sh`.
#
# P2 keeps the four P1 functions UNCHANGED — lo-build, lo-check, lo-compile,
# lo-wasmrun — so you may paste your P1 fill-in verbatim. It adds one variable
# (LO_ASSEMBLER) and one function (lo-emit-asm) for the native back end, and
# one REQUIRED function (lo-dump-ir) that prints your IR.
#
# -----------------------------------------------------------------------------
# What the grader does — three stages
# -----------------------------------------------------------------------------
# Once per submission:
#
#     $ source p2-grading-contract.sh
#     $ lo-build                                       # build everything, once
#     $ lo-dump-ir prog.lo                             # IR-dump gate, on ONE program
#
# IR-dump gate (0 points, ALL-OR-NOTHING): lo-dump-ir must be defined and, on one
# LO-4 program, exit 0 and print some text (your IR, any format). If it is
# missing, fails or prints nothing, nothing else is graded and the submission
# scores 0.
#
# Stage 1, WASM regression gate (0 points, ALL-OR-NOTHING): every Project-1
# test, via lo-check / lo-compile / lo-wasmrun, exactly as in P1:
#
#     $ lo-check   prog.lo                             # front-end only
#     $ lo-compile prog.lo /tmp/artifacts/prog.wasm    # front-end, and if good,
#                                                      # must emit .wasm at $2
#     $ lo-wasmrun /tmp/artifacts/prog.wasm            # run it
#
#   One failure stops grading and the submission scores 0. A P2 that regresses
#   P1 is not graded — fix the regression and resubmit.
#
# Stage 2, native (0.25 points per test): every Project-1 test again. lo-check as
# before, and for run tests:
#
#     $ lo-emit-asm prog.lo /tmp/artifacts/prog.s      # YOUR compiler writes
#                                                      # Intel-syntax x86-64 asm to $2
#   then the GRADER, as you:
#     $ as --64 -o prog.o prog.s         (or: nasm -f elf64 -o prog.o prog.s
#                                              when LO_ASSEMBLER=nasm)
#     $ cc -no-pie -o prog prog.o /opt/lo-runtime/liblo_runtime.a -lpthread -ldl -lm
#     $ ./prog < input                                 # exit status and stdout graded
#
# Stage 3, spill tests (1 point per program): lo-emit-asm on register-pressure
# programs (the public ones are in lo-testing's p2-spill-tests/); the executable is run under
# valgrind and its stack loads + stores per loop iteration are compared with
# the target in the program's header ("expected spills: less than N (plus
# configured slack)" — N is what the reference compiler achieves + 1; the
# grader adds a published allowance on top). Every program's verdict is one
# line — your spills per loop iteration, the bound, the reference's count and
# the configured slack — followed by every instruction of yours that contributes
# to that count, with its share per iteration.
#
# Not every program runs every step: programs that are supposed to be REJECTED
# are only ever passed to `lo-check`.
#
# -----------------------------------------------------------------------------
# Rules that apply to this file as a whole
# -----------------------------------------------------------------------------
#  * It must be safe to `source`. Do NOT do any work at the top level: no
#    building, no `cd`, no output. Only variable assignments and function
#    definitions. The grader sources this file once and then calls the
#    functions many times.
#  * Do NOT print anything while being sourced. Anything your functions print
#    on stdout is compared byte-for-byte against expected program output, and a
#    stray banner will fail tests that would otherwise pass.
#  * Do NOT use `set -e` at the top level. A non-zero exit from `lo-check` is a
#    NORMAL, EXPECTED result (it means "this program has an error"), and `set -e`
#    would tear down the grader's shell. Use `set -e` inside a function body if
#    you want fail-fast there.
#  * Do not rename the functions or change their argument order.
#  * Paths given to you may be absolute and may contain no spaces, but quote
#    "$1" / "$2" anyway.
# =============================================================================

set -uo pipefail   # deliberately NOT -e; see above

# Absolute path to your repository root, so the functions work no matter what
# directory the grader happens to be in. Leave this as-is.
# To be safe, consider using $LO_ROOT in your commands to refer to your submission's
# root (review filled-in example at the end).
LO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"


# -----------------------------------------------------------------------------
# LO_ASSEMBLER
# -----------------------------------------------------------------------------
# Which assembler dialect lo-emit-asm writes:
#   gas   GNU as, Intel syntax (`.intel_syntax noprefix`) — the default
#   nasm  NASM syntax (`nasm -f elf64`)
# Leave it empty or set it to `gas` unless you emit NASM.
# -----------------------------------------------------------------------------
LO_ASSEMBLER=gas


# -----------------------------------------------------------------------------
# lo-build
# -----------------------------------------------------------------------------
# Build your compiler and anything it needs (your runtime, libraries, ...).
# Called ONCE, before any other function.
#
#   arguments : none
#   stdout    : ignored (build logs are fine here)
#   stderr    : ignored (shown to you if the build fails)
#   exit      : 0 on success, non-zero on failure.
#
# If this fails, the grader reports "your submission did not build", shows you
# the tail of this output, and skips every other test. Get this right first.
#
# The grading container has a network connection and a warm Cargo registry, but
# builds are on a slow, single-core machine — keep it under ~10 minutes.
# -----------------------------------------------------------------------------
lo-build() {
    cargo build --release --manifest-path "$LO_ROOT/compiler/Cargo.toml" || return 1
    # The WASM runtime for lo-compile. Emit WASM objects, not LLVM bitcode tied
    # to rustc's LLVM version. The native runtime is the grader's own copy.
    cargo build --release --target wasm32-unknown-unknown --lib \
        --config profile.release.lto=false --manifest-path "$LO_ROOT/rust/Cargo.toml"
}


# -----------------------------------------------------------------------------
# lo-check  <source.lo>
# -----------------------------------------------------------------------------
# Run ONLY the front end: parse, resolve names, type check. Do not generate or
# write any output file.
#
#   $1        : path to a .lo source file
#   stdout    : ignored
#   stderr    : on rejection, must contain the error code for the problem you
#               found (see error-codes.md), e.g. E_UNKNOWN_VARIABLE. The grader
#               searches stderr for the code as a substring, so you are free to
#               word the surrounding message however you like.
#   exit      : 0  the program is well-formed and type-correct
#               1  the program has an error (and stderr names the code)
#
# Exit exactly 1 to reject. Any other non-zero status is treated as your
# compiler having crashed rather than having diagnosed the problem, and will
# not earn credit — this is deliberate, since a segfault on a malformed program
# is not the same as catching the error.
# -----------------------------------------------------------------------------
lo-check() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --check "$1"
}


# -----------------------------------------------------------------------------
# lo-compile  <source.lo>  <output.wasm>
# -----------------------------------------------------------------------------
# Compile a program all the way to a linked WebAssembly module: run the front
# end, generate code, and link against your runtime. UNCHANGED from P1: this is
# what the regression gate runs.
#
#   $1        : path to a .lo source file
#   $2        : path where the finished .wasm MUST be written
#   stdout    : ignored
#   stderr    : shown to you on failure
#   exit      : 0 on success, non-zero on failure
#
# $2 is always of the form /tmp/artifacts/<name>.wasm. The grader creates and
# empties /tmp/artifacts before each program, so you may assume the directory
# exists and is writable, and you do not need to clean up after yourself.
#
# Write the module to EXACTLY $2 — not to a fixed path of your own choosing,
# and not next to the source file. Immediately after this function returns, the
# grader checks that $2 exists and is a non-empty regular file, and reports that
# check as its own test. If you exit 0 but write nothing, you will see
# "no .wasm produced" rather than a confusing failure later on.
#
# The finished module must export `lo_entry` (returning i32) and a `memory`,
# because that is what the `wasmrun` host harness looks for.
# -----------------------------------------------------------------------------
lo-compile() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --compile "$1" "$2"
}


# -----------------------------------------------------------------------------
# lo-wasmrun  <module.wasm>
# -----------------------------------------------------------------------------
# Run a compiled module under the course's host harness.
#
# This one is already written, and you should NOT change it. `wasmrun` is
# installed in the course container and in the grading image; it supplies the
# `host` I/O imports your runtime is written against, calls `lo_entry`, and
# exits with whatever your program returned (or with a documented abort code
# such as 101 or 110 if your runtime trapped).
#
#   $1        : path to a linked .wasm module
#   stdin     : the grader may pipe a test's input in; wasmrun forwards it
#   stdout    : your program's output, compared byte-for-byte where a test
#               declares expected output
#   stderr    : searched for the expected message on abort tests
#   exit      : your program's exit status — must be passed through unchanged
#
# Do not wrap this in anything that swallows, renumbers, or remaps the exit
# status; the grader compares it against the value the test declares.
# -----------------------------------------------------------------------------
lo-wasmrun() {
    wasmrun "$1"
}


# -----------------------------------------------------------------------------
# lo-emit-asm  <source.lo>  <output.s>                                     (NEW)
# -----------------------------------------------------------------------------
# Compile a program to x86-64 assembly text: run the front end, lower to your
# IR, allocate registers, and write the assembly file. Do NOT assemble, link or
# run it — the grader does that, with the commands shown at the top of this
# file, against ITS copy of the complete runtime (/opt/lo-runtime/liblo_runtime.a).
#
#   $1        : path to a .lo source file
#   $2        : path where the assembly MUST be written (/tmp/artifacts/<name>.s)
#   stdout    : ignored
#   stderr    : shown to you on failure
#   exit      : 0 on success, non-zero on failure
#
# What the grader's link step needs from your .s:
#   * a global `main` (`.globl main`) that calls `lo_runtime_init` first and
#     returns `Main.main`'s result as the process exit status;
#   * the dialect LO_ASSEMBLER names (GNU as Intel syntax by default);
#   * absolute `.quad` tables are fine — the link is `-no-pie`;
#   * end with `.section .note.GNU-stack,"",@progbits` (avoids a linker warning);
#   * runtime calls by their ABI names (lo_alloc, lo_print_int, lo_push_frame, …).
#
# Write to EXACTLY $2. The grader checks that $2 exists and is non-empty and
# reports that as its own test, then assembles it: an `as` error shows up as
# `assemble_failed` with the assembler's diagnostic, a link error as
# `link_failed` with the undefined symbols.
# -----------------------------------------------------------------------------
lo-emit-asm() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --emit-asm "$1" "$2"
}


# -----------------------------------------------------------------------------
# lo-dump-ir  <source.lo>                                              (REQUIRED)
# -----------------------------------------------------------------------------
# Print your intermediate representation for the program to stdout. Worth 0
# points, but a GATE: the grader runs it once, on one LO-4 program, and needs
# exit status 0 and some non-blank text on stdout. The format is yours (the
# handout asks for a readable dump); the grader does not parse it, and keeps the
# first few KB for the course staff (it is handy during the code review). If it
# is missing, fails or prints nothing, nothing else is graded and the
# submission scores 0.
#
#   $1        : path to a .lo source file
#   stdout    : your IR, any format, not empty
#   stderr    : shown to you on failure
#   exit      : 0 on success
# -----------------------------------------------------------------------------
lo-dump-ir() {
    "$LO_ROOT/compiler/target/release/lo-compiler" --dump-ir "$1"
}


# =============================================================================
# Worked example
# =============================================================================
# For reference, here is a complete fill-in for a Rust compiler named `loc`
# that emits a relocatable wasm object and links it against a runtime
# staticlib, and that writes GNU-as Intel-syntax assembly for the native back
# end. Yours will differ — this is only to show the shape. Pay special
# attention to the return codes of each function.
#
#   LO_ASSEMBLER=gas
#
#   lo-build() {
#       ( cd "$LO_ROOT" && cargo build --release ) || return 1
#       ( cd "$LO_ROOT/runtime" &&
#         cargo build --release --target wasm32-unknown-unknown --lib ) || return 1
#   }
#
#   lo-check() {
#       "$LO_ROOT/target/release/loc" --check "$1"
#   }
#
#   lo-compile() {
#       local obj; obj="$(mktemp)" || return 1
#       "$LO_ROOT/target/release/loc" --emit-obj "$obj" "$1" || { rm -f "$obj"; return 1; }
#       "$LO_ROOT/tools/wasm-ld" "$obj" "$LO_ROOT/runtime/liblo_runtime.a" -o "$2" \
#           --export=lo_entry --no-entry --allow-undefined
#       local rc=$?; rm -f "$obj"; return $rc
#   }
#
#   lo-emit-asm() {
#       "$LO_ROOT/target/release/loc" --emit-asm "$2" "$1"
#   }
#
#   lo-dump-ir() {
#       "$LO_ROOT/target/release/loc" --dump-ir "$1"
#   }
# =============================================================================
