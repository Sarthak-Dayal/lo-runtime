//! The native pipeline end to end, and its command line.
//!
//! source -> lex/parse/check -> lower -> insert_gc_roots -> verify -> allocate
//!        -> emit assembly -> `as` -> `gcc -static` (against liblo_runtime.a)
//!
//! Modes (handled ahead of the P1 contract in `main`):
//!   --dump-ir  FILE.lo                       readable IR, as the back end sees it
//!                                            (after GC-root insertion)
//!   --emit-asm FILE.lo [OUT.s]               Intel-syntax assembly
//!   --native   FILE.lo OUT [--runtime LIB.a] assemble and link an executable
//!                                            (LIB.a defaults to $LO_RUNTIME)
//! Allocation: linear scan by default; `--spill-all` selects the spill-everything
//! reference (every virtual register in a stack slot), useful for debugging.
//! `-g` selects the debug build (P4), in any of the three modes.

use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

use crate::ir::lower::lower_program;
use crate::ir::register_allocator::spill_all::spill_everything_program;
use crate::ir::register_allocator::{allocate_registers, TargetConstraints};
use crate::ir::{gc_roots, CheckedIr, DebugMode};
use crate::layout::NATIVE;

use super::emit_program;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocStrategy {
    /// Every virtual register in its own stack slot. Slow, simple, always correct.
    SpillAll,
    /// Poletto-Sarkar linear scan (one pass over live intervals sorted by start;
    /// Poletto and Sarkar, 1999) over the System V register file.
    LinearScan,
}

const USAGE: &str = "usage: lo-compiler (--dump-ir FILE.lo | --emit-asm FILE.lo [OUT.s] | \
                     --native FILE.lo OUTPUT [--runtime LIB.a]) [--spill-all | --linear-scan] [-g]";

/// Lowered, rooted and verified IR: exactly what instruction selection consumes.
pub fn build_ir(path: &OsStr, debug: DebugMode) -> Result<CheckedIr, String> {
    let (typed, classes) = crate::front_end(path)?;
    let ir = lower_program(&typed, &classes, NATIVE, debug)?;
    let mut ir = gc_roots::insert_gc_roots(ir, debug);
    for function in &mut ir.functions {
        crate::ir::remove_dead_copies(function, debug);
    }
    ir.verify()
}

pub fn dump_ir(path: &OsStr, debug: DebugMode) -> Result<String, String> {
    Ok(build_ir(path, debug)?.program().dump())
}

pub fn compile_to_asm(
    path: &OsStr,
    strategy: AllocStrategy,
    debug: DebugMode,
) -> Result<String, String> {
    let ir = build_ir(path, debug)?;
    let allocation = match strategy {
        AllocStrategy::SpillAll => spill_everything_program(&ir),
        AllocStrategy::LinearScan => allocate_registers(&ir, &TargetConstraints::system_v(), debug),
    };
    emit_program(ir.program(), &allocation, debug, Path::new(path))
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
/// Under `-g`, `as --gdwarf-5` writes the line table as DWARF 5 (DWARF Subset
/// Reference §9.1), and `--eh-frame-hdr` keeps the index a debugger uses to find
/// each function's call-frame information; `gcc -static` leaves it out otherwise.
pub fn assemble_and_link(
    asm: &str,
    runtime_lib: &Path,
    output: &Path,
    debug: DebugMode,
) -> Result<(), String> {
    let scratch = ScratchDir::new()?;
    let asm_path = scratch.0.join("program.s");
    let object_path = scratch.0.join("program.o");
    std::fs::write(&asm_path, asm).map_err(|e| format!("write {}: {e}", asm_path.display()))?;
    let mut assemble = Command::new("as");
    if debug == DebugMode::On {
        assemble.arg("--gdwarf-5");
    }
    run(assemble.arg("-o").arg(&object_path).arg(&asm_path))?;
    let mut link = Command::new("gcc");
    if debug == DebugMode::On {
        link.arg("-Wl,--eh-frame-hdr");
    }
    run(link
        .arg("-static")
        .arg("-o")
        .arg(output)
        .arg(&object_path)
        .arg(runtime_lib)
        .args(["-lm", "-lpthread", "-ldl"]))
}

fn run(command: &mut Command) -> Result<(), String> {
    let program = command.get_program().to_owned();
    let output = command.output().map_err(|e| format!("{program:?}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{program:?} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim_end()
    ))
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
    debug: DebugMode,
    runtime: Option<OsString>,
    positional: Vec<&'a OsString>,
}

fn parse_options(args: &[OsString]) -> Result<Options<'_>, String> {
    let mut options = Options {
        strategy: AllocStrategy::LinearScan,
        debug: DebugMode::Off,
        runtime: std::env::var_os("LO_RUNTIME"),
        positional: vec![],
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.to_str() {
            Some("--spill-all") => options.strategy = AllocStrategy::SpillAll,
            Some("--linear-scan") => options.strategy = AllocStrategy::LinearScan,
            Some("-g") => options.debug = DebugMode::On,
            Some("--runtime") => options.runtime = Some(iter.next().ok_or(USAGE)?.clone()),
            _ => options.positional.push(arg),
        }
    }
    Ok(options)
}

fn run_cli(mode: &str, args: &[OsString]) -> Result<String, String> {
    let options = parse_options(args)?;
    let (strategy, debug) = (options.strategy, options.debug);
    match (mode, options.positional.as_slice()) {
        ("--dump-ir", [file]) => dump_ir(file, debug),
        ("--emit-asm", [file]) => compile_to_asm(file, strategy, debug),
        ("--emit-asm", [file, out]) => {
            let asm = compile_to_asm(file, strategy, debug)?;
            std::fs::write(out, asm).map_err(|e| format!("write output: {e}"))?;
            Ok(String::new())
        }
        ("--native", [file, out]) => {
            let runtime = options
                .runtime
                .ok_or("--native needs --runtime LIB.a (or $LO_RUNTIME)")?;
            let asm = compile_to_asm(file, strategy, debug)?;
            assemble_and_link(&asm, Path::new(&runtime), Path::new(out), debug)?;
            Ok(String::new())
        }
        _ => Err(USAGE.into()),
    }
}
