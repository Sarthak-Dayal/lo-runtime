//! The native pipeline end to end, and its command line.
//!
//! source -> lex/parse/check -> lower -> insert_gc_roots -> verify -> allocate
//!        -> emit assembly -> `as` -> `gcc -static` (against liblo_runtime.a)
//!
//! Modes (handled ahead of the P1 contract in `main`):
//!   --dump-ir  FILE.lo                       readable IR, as the back end sees it
//!   --emit-asm FILE.lo [OUT.s]               Intel-syntax assembly
//!   --native   FILE.lo OUT --runtime LIB.a   assemble and link an executable
//! Allocation: `--spill-all` (default, the debugging reference) or `--linear-scan`.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::ir::lower::{lower_program, TargetLayout};
use crate::ir::register_allocator::spill_all::spill_everything_program;
use crate::ir::{gc_roots, CheckedIr};

use super::emit_program;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocStrategy {
    /// Every virtual register in its own stack slot. Slow, simple, always correct.
    SpillAll,
    /// Poletto-Sarkar linear scan. Not wired in yet.
    LinearScan,
}

const USAGE: &str = "usage: lo-compiler (--dump-ir FILE.lo | --emit-asm FILE.lo [OUT.s] | \
                     --native FILE.lo OUTPUT --runtime LIB.a) [--spill-all | --linear-scan]";

/// Lowered, rooted and verified IR: exactly what instruction selection consumes.
pub fn build_ir(source: &str) -> Result<CheckedIr, String> {
    let tokens = crate::lexer::tokenize(source)
        .map_err(|e| format!("lex line {} [{}]: {:?}", e.line, e.kind.as_str(), e.kind))?;
    let ast = crate::parser::parse_program(&tokens)
        .map_err(|e| format!("parse line {} [{}]: {}", e.line, e.code.as_str(), e.message))?;
    let (typed, classes) = crate::type_checker::check_program(ast).map_err(|e| {
        format!(
            "type check line {} [{}]: {}",
            e.line,
            e.code.as_str(),
            e.message
        )
    })?;
    let ir = lower_program(&typed, &classes, TargetLayout::X86_64)?;
    gc_roots::insert_gc_roots(ir).verify()
}

pub fn dump_ir(source: &str) -> Result<String, String> {
    Ok(build_ir(source)?.program().dump())
}

pub fn compile_to_asm(source: &str, strategy: AllocStrategy) -> Result<String, String> {
    let ir = build_ir(source)?;
    let allocation = match strategy {
        AllocStrategy::SpillAll => spill_everything_program(&ir),
        AllocStrategy::LinearScan => {
            return Err(
                "--linear-scan is not wired in yet: the allocator still targets \
                         Microsoft x64 and calls need parallel argument moves"
                    .into(),
            )
        }
    };
    emit_program(ir.program(), &allocation)
}

/// `as` then `gcc -static`, the same two steps as the grading harness.
pub fn assemble_and_link(asm: &str, runtime_lib: &Path, output: &Path) -> Result<(), String> {
    let asm_path = output.with_extension("s");
    let object_path = output.with_extension("o");
    std::fs::write(&asm_path, asm).map_err(|e| format!("write {}: {e}", asm_path.display()))?;
    run(Command::new("as")
        .arg("-o")
        .arg(&object_path)
        .arg(&asm_path))?;
    run(Command::new("gcc")
        .arg("-static")
        .arg("-o")
        .arg(output)
        .arg(&object_path)
        .arg(runtime_lib))
}

fn run(command: &mut Command) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|e| format!("{:?}: {e}", command.get_program()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{:?} failed: {}",
            command.get_program(),
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// Handles the native modes; `None` means the arguments are not ours.
pub fn cli(args: &[OsString]) -> Option<Result<String, String>> {
    let mode = args.get(1)?.to_str()?;
    if !matches!(mode, "--dump-ir" | "--emit-asm" | "--native") {
        return None;
    }
    Some(run_cli(mode, &args[2..]))
}

fn run_cli(mode: &str, args: &[OsString]) -> Result<String, String> {
    let mut strategy = AllocStrategy::SpillAll;
    let mut runtime: Option<&OsString> = None;
    let mut positional = vec![];
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--spill-all") => strategy = AllocStrategy::SpillAll,
            Some("--linear-scan") => strategy = AllocStrategy::LinearScan,
            Some("--runtime") => runtime = Some(iter.next().ok_or(USAGE)?),
            _ => positional.push(arg),
        }
    }
    let source =
        |path: &OsString| std::fs::read_to_string(path).map_err(|e| format!("read source: {e}"));
    match (mode, positional.as_slice()) {
        ("--dump-ir", [file]) => dump_ir(&source(file)?),
        ("--emit-asm", [file]) => compile_to_asm(&source(file)?, strategy),
        ("--emit-asm", [file, out]) => {
            let asm = compile_to_asm(&source(file)?, strategy)?;
            std::fs::write(out, asm).map_err(|e| format!("write output: {e}"))?;
            Ok(String::new())
        }
        ("--native", [file, out]) => {
            let runtime = runtime.ok_or("--native needs --runtime LIB.a")?;
            let asm = compile_to_asm(&source(file)?, strategy)?;
            assemble_and_link(&asm, Path::new(runtime), Path::new(out))?;
            Ok(String::new())
        }
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::Stdio;

    const PROGRAMS: &[&str] = &[
        include_str!("../../../tests/lo_programs/alloc_basic.lo"),
        include_str!("../../../tests/lo_programs/class_basic.lo"),
        include_str!("../../../tests/lo_programs/string_basic.lo"),
        "class Parent (int n;) { int value() { return n; } } class Child extends Parent (int m; String text;) [ Child() { super(1); m = 2; text = \"hi\"; } ] { int value() { return (super.value() + m); } int sum() { return (n + m); } } class Main () { int main() { Parent p; p = new Child(); out.print_int(p.value()); out.print_string((\"x\" + \"y\")); return ((p instanceof Child) ? ((Child) p).sum() : 0); } }",
        "class Main () { int f(int a, int b, int c, int d, int e, int g, int h, int i) { return ((((a + b) + (c * d)) + ((e - g) / h)) % i); } int fact(int n) { return ((n < 2) ? 1 : (n * this.fact((n - 1)))); } int main() { return (this.f(1,2,3,4,5,6,7,8) + this.fact(5)); } }",
        "class Main () { int main() { Output saved; saved = out; out = err; out.print_int(1); saved.print_int(2); err = saved; err.print_bool(true); out.println(); return 0; } }",
        "class Main () { int main() { int i; String s; i = 0; s = \"\"; while ((i < 5)) { s = (s + \"a\"); i = (i + 1); if ((i > 3)) { break; } else { i = i; } } out.print_string(s); return ((10 / (i - 4)) + (7 % (0 - 1))); } }",
        "class Node (int v; Node next;) [ Node(int x) { v = x; next = null; } ] { int get() { return v; } void link(Node n) { next = n; } } class Main () { int main() { Node a; Node b; a = new Node(1); b = new Node(2); a.link(b); return (a.get() + b.get()); } }",
    ];

    /// Assemble with clang as an x86-64 Linux target; `None` if clang is absent.
    fn assembles(asm: &str) -> Option<Result<(), String>> {
        let mut child = Command::new("clang")
            .args([
                "--target=x86_64-unknown-linux-gnu",
                "-c",
                "-x",
                "assembler",
                "-o",
            ])
            .arg(std::env::temp_dir().join(format!("lo-test-{}.o", std::process::id())))
            .arg("-")
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(asm.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        Some(if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).into_owned())
        })
    }

    #[test]
    fn every_program_compiles_and_assembles() {
        for (index, source) in PROGRAMS.iter().enumerate() {
            let asm = compile_to_asm(source, AllocStrategy::SpillAll)
                .unwrap_or_else(|e| panic!("program {index}: {e}"));
            assert!(asm.starts_with(".intel_syntax noprefix\n.text\n"));
            assert!(asm.contains(".globl main\nmain:"));
            assert!(asm.contains("call lo_entry"));
            if let Some(result) = assembles(&asm) {
                result.unwrap_or_else(|e| panic!("program {index} does not assemble:\n{e}"));
            }
        }
    }

    #[test]
    fn startup_frame_is_registered_after_runtime_init() {
        let asm = compile_to_asm(PROGRAMS[0], AllocStrategy::SpillAll).unwrap();
        let entry = asm.split("\nlo_entry:\n").nth(1).unwrap();
        let entry = entry.split("\n.globl").next().unwrap();
        let init = entry.find("call lo_runtime_init").unwrap();
        let push = entry.find("call lo_push_frame").unwrap();
        assert!(init < push, "frame pushed before lo_runtime_init");
        // lo_entry's own frame is linked first, then the static lo_bindings frame.
        let first_lea = entry[init..].find("lea rdi, [rbp").unwrap();
        assert!(
            init + first_lea < init + entry[init..].find("lea rdi, [rip + lo_bindings]").unwrap()
        );
        assert_eq!(entry.matches("call lo_push_frame").count(), 2);
        assert_eq!(entry.matches("call lo_pop_frame").count(), 2);
    }

    #[test]
    fn data_sections_and_stack_note_are_emitted() {
        let asm = compile_to_asm(PROGRAMS[1], AllocStrategy::SpillAll).unwrap();
        assert!(asm.contains("lo_class_3_Box:"));
        assert!(asm.contains("lo_class_3_Box_vtable:"));
        assert!(asm.contains("lo_bindings:"));
        assert!(asm.contains(".quad lo_method_3_Box_3_get"));
        assert!(asm.ends_with(".section .note.GNU-stack,\"\",@progbits\n"));
    }

    #[test]
    fn linear_scan_is_rejected_until_wired_in() {
        let err = compile_to_asm(PROGRAMS[0], AllocStrategy::LinearScan).unwrap_err();
        assert!(err.contains("not wired in"));
    }

    #[test]
    fn cli_ignores_other_modes_and_reports_usage() {
        let args = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(cli(&args(&["lo-compiler", "--check", "x.lo"])).is_none());
        assert!(cli(&args(&["lo-compiler"])).is_none());
        let bad = cli(&args(&["lo-compiler", "--native", "x.lo"])).unwrap();
        assert!(bad.is_err());
    }
}
