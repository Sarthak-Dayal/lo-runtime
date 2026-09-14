//! Assemble and link separately so diagnostics name the failing pipeline stage.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn compile(assembly: &str, output: &Path) -> Result<(), String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("toolchain clock: {e}"))?
        .as_nanos();
    let work = std::env::temp_dir().join(format!("lo-compile-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&work).map_err(|e| format!("create artifacts: {e}"))?;
    let result = assemble_and_link(assembly, output, &work);
    match result {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&work);
            Ok(())
        }
        Err(error) => Err(format!(
            "{error}\nAssembly/object artifacts: {}",
            work.display()
        )),
    }
}

fn assemble_and_link(assembly: &str, output: &Path, work: &Path) -> Result<(), String> {
    let source = work.join("program.s");
    let object = work.join("program.o");
    std::fs::write(&source, assembly).map_err(|e| format!("write assembly: {e}"))?;
    let mc = std::env::var_os("LLVM_MC").unwrap_or_else(|| "llvm-mc".into());
    run(
        "assemble",
        Command::new(mc)
            .args(["-triple=wasm32-unknown-unknown", "-filetype=obj"])
            .arg(&source)
            .arg("-o")
            .arg(&object),
    )?;
    let runtime = std::env::var_os("LO_RUNTIME_ARCHIVE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../rust/target/wasm32-unknown-unknown/release/liblo_runtime.a")
        });
    if !runtime.is_file() {
        return Err(format!(
            "link: runtime archive not found: {} (run lo-build or set LO_RUNTIME_ARCHIVE)",
            runtime.display()
        ));
    }
    let ld = std::env::var_os("WASM_LD").unwrap_or_else(|| "wasm-ld".into());
    run(
        "link",
        Command::new(ld)
            .args([
                "--no-entry",
                "--export=lo_entry",
                "--export-memory",
                "--allow-undefined",
            ])
            .arg(&object)
            .arg(runtime)
            .arg("-o")
            .arg(output),
    )
}

fn run(stage: &str, command: &mut Command) -> Result<(), String> {
    let result = command.output().map_err(|e| format!("{stage}: {e}"))?;
    if result.status.success() {
        return Ok(());
    }
    Err(format!(
        "{stage}: {}\n{}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    ))
}
