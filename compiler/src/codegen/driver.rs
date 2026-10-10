//! The native pipeline end to end, and its command line.
//!
//! source -> lex/parse/check -> lower -> insert_gc_roots -> verify -> allocate
//!        -> emit assembly -> `as` -> `gcc -static` (against liblo_runtime.a)
//!
//! Modes (handled ahead of the P1 contract in `main`):
//!   --dump-ir  FILE.lo                       readable IR, as the back end sees it
//!   --emit-asm FILE.lo [OUT.s]               Intel-syntax assembly
//!   --native   FILE.lo OUT [--runtime LIB.a] assemble and link an executable
//!                                            (LIB.a defaults to $LO_RUNTIME)
//! Allocation: linear scan by default; `--spill-all` selects the spill-everything
//! reference (every virtual register in a stack slot), useful for debugging.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::ir::lower::{lower_program, TargetLayout};
use crate::ir::register_allocator::spill_all::spill_everything_program;
use crate::ir::register_allocator::{allocate_registers, TargetConstraints};
use crate::ir::{gc_roots, CheckedIr};

use super::emit_program;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocStrategy {
    /// Every virtual register in its own stack slot. Slow, simple, always correct.
    SpillAll,
    /// Poletto-Sarkar linear scan over the System V register file.
    LinearScan,
}

const USAGE: &str = "usage: lo-compiler (--dump-ir FILE.lo | --emit-asm FILE.lo [OUT.s] | \
                     --native FILE.lo OUTPUT [--runtime LIB.a]) [--spill-all | --linear-scan]";

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
        AllocStrategy::LinearScan => allocate_registers(&ir, &TargetConstraints::system_v())
            .map_err(|e| format!("register allocation: {e:?}"))?,
    };
    emit_program(ir.program(), &allocation)
}

/// A scratch directory for the intermediate `.s` and `.o`, removed on drop so the
/// only file left behind is the executable.
struct ScratchDir(std::path::PathBuf);

impl ScratchDir {
    fn new() -> Result<ScratchDir, String> {
        let dir = std::env::temp_dir().join(format!("lo-native-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        Ok(ScratchDir(dir))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `as` then `gcc -static`, the same two steps as the grading harness. The Rust
/// runtime's standard library needs `-lm -lpthread -ldl` after the archive.
pub fn assemble_and_link(asm: &str, runtime_lib: &Path, output: &Path) -> Result<(), String> {
    let scratch = ScratchDir::new()?;
    let asm_path = scratch.0.join("program.s");
    let object_path = scratch.0.join("program.o");
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
        .arg(runtime_lib)
        .args(["-lm", "-lpthread", "-ldl"]))
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

struct Options<'a> {
    strategy: AllocStrategy,
    runtime: Option<OsString>,
    positional: Vec<&'a OsString>,
}

fn parse_options(args: &[OsString]) -> Result<Options<'_>, String> {
    let mut options = Options {
        strategy: AllocStrategy::LinearScan,
        runtime: std::env::var_os("LO_RUNTIME"),
        positional: vec![],
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--spill-all") => options.strategy = AllocStrategy::SpillAll,
            Some("--linear-scan") => options.strategy = AllocStrategy::LinearScan,
            Some("--runtime") => options.runtime = Some(iter.next().ok_or(USAGE)?.clone()),
            _ => options.positional.push(arg),
        }
    }
    Ok(options)
}

fn run_cli(mode: &str, args: &[OsString]) -> Result<String, String> {
    let options = parse_options(args)?;
    let strategy = options.strategy;
    let source =
        |path: &OsString| std::fs::read_to_string(path).map_err(|e| format!("read source: {e}"));
    match (mode, options.positional.as_slice()) {
        ("--dump-ir", [file]) => dump_ir(&source(file)?),
        ("--emit-asm", [file]) => compile_to_asm(&source(file)?, strategy),
        ("--emit-asm", [file, out]) => {
            let asm = compile_to_asm(&source(file)?, strategy)?;
            std::fs::write(out, asm).map_err(|e| format!("write output: {e}"))?;
            Ok(String::new())
        }
        ("--native", [file, out]) => {
            let runtime = options
                .runtime
                .ok_or("--native needs --runtime LIB.a (or $LO_RUNTIME)")?;
            let asm = compile_to_asm(&source(file)?, strategy)?;
            assemble_and_link(&asm, Path::new(&runtime), Path::new(out))?;
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
    fn linear_scan_programs_assemble_and_use_fewer_stack_accesses() {
        let count = |asm: &str| asm.matches("[rbp - ").count();
        for (index, source) in PROGRAMS.iter().enumerate() {
            let linear = compile_to_asm(source, AllocStrategy::LinearScan)
                .unwrap_or_else(|e| panic!("program {index}: {e}"));
            let spill = compile_to_asm(source, AllocStrategy::SpillAll).unwrap();
            if let Some(result) = assembles(&linear) {
                result.unwrap_or_else(|e| panic!("program {index} does not assemble:\n{e}"));
            }
            assert!(
                count(&linear) < count(&spill),
                "program {index}: linear scan should touch the stack less ({} vs {})",
                count(&linear),
                count(&spill)
            );
        }
    }

    #[test]
    fn cli_ignores_other_modes_and_reports_usage() {
        let args = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        assert!(cli(&args(&["lo-compiler", "--check", "x.lo"])).is_none());
        assert!(cli(&args(&["lo-compiler"])).is_none());
        let bad = cli(&args(&["lo-compiler", "--native", "x.lo"])).unwrap();
        assert!(bad.is_err());
    }

    #[test]
    fn linear_scan_is_the_default_and_spill_all_is_opt_in() {
        let args = |a: &[&str]| a.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            parse_options(&args(&["x.lo"])).unwrap().strategy,
            AllocStrategy::LinearScan
        );
        assert_eq!(
            parse_options(&args(&["x.lo", "--spill-all"]))
                .unwrap()
                .strategy,
            AllocStrategy::SpillAll
        );
        assert_eq!(
            parse_options(&args(&["--spill-all", "--linear-scan", "x.lo"]))
                .unwrap()
                .strategy,
            AllocStrategy::LinearScan
        );
        let runtime_args = args(&["x.lo", "--runtime", "lib.a"]);
        let with_runtime = parse_options(&runtime_args).unwrap();
        assert_eq!(with_runtime.runtime, Some(OsString::from("lib.a")));
        assert_eq!(with_runtime.positional.len(), 1);
        assert!(parse_options(&args(&["--runtime"])).is_err());
    }

    #[test]
    fn failed_link_leaves_no_files_behind() {
        let out_dir = std::env::temp_dir().join(format!("lo-out-{}", std::process::id()));
        std::fs::create_dir_all(&out_dir).unwrap();
        let result = assemble_and_link(
            "this is not assembly\n",
            Path::new("/nonexistent/liblo_runtime.a"),
            &out_dir.join("prog"),
        );
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(&out_dir).unwrap().count(), 0);
        assert!(!std::env::temp_dir()
            .join(format!("lo-native-{}", std::process::id()))
            .exists());
        std::fs::remove_dir_all(&out_dir).unwrap();
    }
}
