//! Native x86-64 back end: Intel-syntax assembly for the System V AMD64 ABI.

pub mod data;
pub mod driver;
pub mod frame;
pub mod moves;
pub mod print;
pub mod select;
pub mod x86;

use crate::ir::register_allocator::ProgramAllocation;
use std::path::Path;

use crate::ir::{DebugMode, FunctionIr, ProgramIr, GENERATED_LINE};

use frame::{frame_end, frame_setup};

use x86::{Inst, Item, Operand, Reg};

/// The whole assembly file for a verified, rooted program and its allocation.
/// `source` is the `.lo` file, named in the `-g` line table.
///
/// Under `-g`, the functions the compiler writes (the C `main` wrapper,
/// `lo_entry`, implicit constructors, the preamble's IO methods) come first and
/// carry no line marks. The assembler records one sequence of line rows per
/// section, from the first `.loc` to the end, so code placed before the first
/// LO method maps to no source line, rather than to whatever LO line precedes
/// it. (A `.loc` with line 0 would not help: the assembler drops it.)
pub fn emit_program(
    program: &ProgramIr,
    allocation: &ProgramAllocation,
    debug: DebugMode,
    source: &Path,
) -> Result<String, String> {
    let mut items = vec![];
    if debug == DebugMode::On {
        items.extend(line_table_header(source));
    }
    let is_generated = |function: &&FunctionIr| function.decl_line == GENERATED_LINE;
    let (generated, written): (Vec<&FunctionIr>, Vec<&FunctionIr>) = match debug {
        DebugMode::On => program.functions.iter().partition(is_generated),
        DebugMode::Off => (vec![], program.functions.iter().collect()),
    };
    if debug == DebugMode::On {
        items.extend(main_function(program, debug)?);
    }
    for function in generated.into_iter().chain(written) {
        let function_allocation = allocation
            .function_allocations
            .get(&function.symbol)
            .ok_or_else(|| {
                format!(
                    "no allocation for {}",
                    program.symbols[function.symbol.0].name
                )
            })?;
        items.extend(select::select_function(
            program,
            function,
            function_allocation,
            debug,
        )?);
        items.extend(frame_end(debug));
    }
    if debug == DebugMode::Off {
        items.extend(main_function(program, debug)?);
    }
    items.extend(data::emit_data(program)?);
    // Without this note the linker assumes an executable stack.
    items.push(Item::Blank);
    items.push(Item::Directive(
        ".section .note.GNU-stack,\"\",@progbits".into(),
    ));
    Ok(print::file(&items))
}

/// The C `main` wrapper, checked and closed.
fn main_function(program: &ProgramIr, debug: DebugMode) -> Result<Vec<Item>, String> {
    let mut items = main_wrapper(program, debug)?;
    x86::check_instructions("main", &items)?;
    items.extend(frame_end(debug));
    Ok(items)
}

/// Under `-g`, the top of the file: the source file for the line table (DWARF 5
/// needs file 0, the compilation directory and file; `.loc` lines use file 1),
/// then a label at the start of `.debug_line` for the debug info to point at.
fn line_table_header(source: &Path) -> Vec<Item> {
    let absolute = std::path::absolute(source).unwrap_or_else(|_| source.to_path_buf());
    let directory = absolute.parent().unwrap_or(Path::new("."));
    let name = absolute.file_name().map(Path::new).unwrap_or(source);
    let quoted = |path: &Path| {
        let text = path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        format!("\"{text}\"")
    };
    vec![
        Item::Directive(format!(".file 0 {} {}", quoted(directory), quoted(name))),
        Item::Directive(format!(".file 1 {}", quoted(name))),
        Item::Directive(".section .debug_line,\"\",@progbits".into()),
        Item::Label(".Ldebug_line0".into()),
        Item::Directive(".text".into()),
    ]
}

/// The C entry point. `gcc -static` supplies `_start`, which calls `main` and
/// passes its `eax` to `exit`, so `Main.main`'s result becomes the exit status.
/// The runtime is initialized here, before `lo_entry` registers its shadow frame
/// (`runtime-abi.md` §3.6), so `lo_entry` needs no special prologue.
fn main_wrapper(program: &ProgramIr, debug: DebugMode) -> Result<Vec<Item>, String> {
    let startup = program
        .startup
        .ok_or("program has no startup function (lo_entry)")?;
    let entry = program.symbols[startup.0].name.clone();
    // `main` is entered with rsp = 8 (mod 16); push rbp makes it 0 for the calls.
    let mut items = vec![
        Item::Blank,
        Item::Directive(".globl main".into()),
        Item::Label("main".into()),
    ];
    items.extend(frame_setup(debug));
    items.extend([
        Item::Inst(Inst::Call("lo_runtime_init".into())),
        Item::Inst(Inst::Call(entry)),
        Item::Inst(Inst::Pop(Operand::r64(Reg::Rbp))),
        Item::Inst(Inst::Ret),
    ]);
    Ok(items)
}
