#!/usr/bin/env python3
"""Run the LO-3/LO-4 course corpus and retain each pipeline stage separately."""

import argparse
import collections
import json
import os
from pathlib import Path
import re
import subprocess
import sys


def run(command, directory, stage, data=b"", timeout=30):
    try:
        result = subprocess.run(command, input=data, capture_output=True, timeout=timeout)
        record = {"command": [str(x) for x in command], "exit": result.returncode}
        stdout, stderr = result.stdout, result.stderr
    except subprocess.TimeoutExpired as error:
        record = {"command": [str(x) for x in command], "exit": None, "error": "timeout"}
        stdout, stderr = error.stdout or b"", error.stderr or b""
    except OSError as error:
        record = {"command": [str(x) for x in command], "exit": None, "error": str(error)}
        stdout, stderr = b"", str(error).encode()
    (directory / (stage + ".stdout")).write_bytes(stdout)
    (directory / (stage + ".stderr")).write_bytes(stderr)
    record["stdout"] = stdout.decode("utf-8", errors="replace")
    record["stderr"] = stderr.decode("utf-8", errors="replace")
    return record


def header(source, key):
    match = re.search(r"^\s*//\s*" + re.escape(key) + r":\s*(.*?)\s*$", source, re.MULTILINE)
    return match.group(1) if match else None


def frontend_layer(stderr):
    if stderr.startswith("lex line"): return "lexer"
    if stderr.startswith("parse line"): return "parser"
    if stderr.startswith("type check line"): return "type checker"
    return "frontend/driver"


def test_case(path, args):
    name = path.relative_to(args.suite).as_posix()
    directory = args.artifacts / name.removesuffix(".lo")
    directory.mkdir(parents=True, exist_ok=True)
    source = path.read_text()
    record = {"test": name, "passed": False, "stages": {}, "artifacts": str(directory)}

    def stage(label, command, **options):
        result = run(command, directory, label, **options)
        record["stages"][label] = result
        return result

    def fail(layer, reason):
        record.update(layer=layer, reason=reason)
        return record

    checked = stage("check", [args.compiler, "--check", path])
    invalid = "InvalidPrograms" in path.parts
    if invalid:
        expected = header(source, "expected compile error")
        if checked["exit"] != 1:
            return fail("frontend/driver", f"expected rejection status 1; got {checked['exit']}")
        if expected and expected not in checked["stderr"]:
            return fail(frontend_layer(checked["stderr"]), f"expected diagnostic {expected}; got {checked['stderr'].strip()}")
        record["passed"] = True
        return record
    if checked["exit"] != 0:
        return fail(frontend_layer(checked["stderr"]), checked["stderr"].strip() or "frontend failed")

    emitted = stage("emit", [args.compiler, "--emit-wasm", path])
    if emitted["exit"] != 0:
        return fail("emitter", emitted["stderr"].strip() or "emitter failed")
    assembly, obj, wasm = (directory / ("program." + suffix) for suffix in ("s", "o", "wasm"))
    assembly.write_bytes((directory / "emit.stdout").read_bytes())
    assembled = stage("assemble", [args.llvm_mc, "-triple=wasm32-unknown-unknown", "-filetype=obj", assembly, "-o", obj])
    if assembled["exit"] != 0:
        return fail("assembly encoding", assembled["stderr"].strip() or "assembler failed")
    linked = stage("link", [args.wasm_ld, "--no-entry", "--export=lo_entry", "--export-memory", "--allow-undefined", obj, args.runtime, "-o", wasm])
    if linked["exit"] != 0:
        return fail("link/runtime ABI", linked["stderr"].strip() or "linker failed")
    if not wasm.is_file() or wasm.stat().st_size == 0:
        return fail("link/runtime ABI", "no nonempty WASM module produced")
    input_path = path.with_suffix(".input.txt")
    executed = stage("execute", [args.wasmrun, wasm], data=input_path.read_bytes() if input_path.exists() else b"", timeout=args.timeout)
    abort = "RuntimeAbortPrograms" in path.parts
    expected = header(source, "expected exit code" if abort else "main method return value")
    if expected is None:
        return fail("test contract", "missing expected exit header")
    try:
        expected_exit = int(expected) & 255  # POSIX process status returned by wasmrun.
    except ValueError:
        return fail("test contract", f"unsupported exit header: {expected}")
    problems = []
    if executed["exit"] != expected_exit:
        problems.append(f"expected exit {expected_exit}, got {executed['exit']}")
    expected_stderr = header(source, "expected stderr substring")
    if expected_stderr and expected_stderr not in executed["stderr"]:
        problems.append(f"missing stderr substring: {expected_stderr}")
    expected_stdout = path.with_suffix(".expected.out")
    if expected_stdout.exists() and (directory / "execute.stdout").read_bytes() != expected_stdout.read_bytes():
        problems.append("stdout differs from .expected.out")
    if problems:
        # These are localization hints. Semantic mismatches need inspection of the saved artifacts.
        stderr = executed["stderr"]
        layer = "execution (emitter or runtime)"
        if "failed to compile" in stderr or "unknown import" in stderr:
            layer = "WASM validation/imports"
        elif any(symbol in stderr for symbol in ("lo_string_new", "lo_string_concat", "lo_string_repeat", "lo_string_compare", "lo_string_reverse", "lo_cast_check", "lo_instanceof")):
            layer = "runtime (inspect stub or abort path)"
        return fail(layer, "; ".join(problems))
    record["passed"] = True
    return record


def main():
    root = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, default=root / "compiler/target/conformance")
    parser.add_argument("--compiler", type=Path, default=root / "compiler/target/release/lo-compiler")
    parser.add_argument("--runtime", type=Path, default=root / "rust/target/wasm32-unknown-unknown/release/liblo_runtime.a")
    parser.add_argument("--llvm-mc", default=os.environ.get("LLVM_MC", "llvm-mc"))
    parser.add_argument("--wasm-ld", default=os.environ.get("WASM_LD", "wasm-ld"))
    parser.add_argument("--wasmrun", default="wasmrun")
    parser.add_argument("--timeout", type=int, default=30)
    args = parser.parse_args()
    args.suite, args.artifacts = args.suite.resolve(), args.artifacts.resolve()
    paths = sorted(path for base in (args.suite / "LO-3", args.suite / "LO-4", args.suite / "contributed-tests")
                   for path in base.rglob("*.lo") if "LO-3" in path.parts or "LO-4" in path.parts)
    if not paths: parser.error("no LO-3/LO-4 programs found")
    for path in (args.compiler, args.runtime):
        if not path.is_file(): parser.error(f"missing build artifact: {path}; run lo-build first")
    revision = subprocess.run(["git", "-C", args.suite, "rev-parse", "HEAD"], capture_output=True, text=True)
    records = []
    for path in paths:
        record = test_case(path, args)
        records.append(record)
        print(f"{'PASS' if record['passed'] else 'FAIL'} {record['test']}" +
              ("" if record["passed"] else f" [{record['layer']}] {record['reason']}"), flush=True)
    failures = collections.Counter(record["layer"] for record in records if not record["passed"])
    passed = sum(record["passed"] for record in records)
    report = {"suite_revision": revision.stdout.strip(), "passed": passed, "total": len(records),
              "failures_by_layer": dict(failures), "tests": records}
    args.artifacts.mkdir(parents=True, exist_ok=True)
    (args.artifacts / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"\n{passed}/{len(records)} passed. Report: {args.artifacts / 'report.json'}")
    for layer, count in failures.items(): print(f"  {count}: {layer}")
    return 0 if passed == len(records) else 1


if __name__ == "__main__":
    sys.exit(main())
