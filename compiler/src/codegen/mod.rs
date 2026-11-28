//! Native x86-64 back end: Intel-syntax assembly for the System V AMD64 ABI.

pub mod data;
pub mod driver;
pub mod frame;
pub mod moves;
pub mod print;
pub mod select;
pub mod x86;

use crate::ir::register_allocator::ProgramAllocation;
use crate::ir::{DebugMode, ProgramIr};

use frame::{frame_end, frame_setup};

use x86::{Inst, Item, Operand, Reg};

/// The whole assembly file for a verified, rooted program and its allocation.
pub fn emit_program(
    program: &ProgramIr,
    allocation: &ProgramAllocation,
    debug: DebugMode,
) -> Result<String, String> {
    let mut items = vec![];
    for function in &program.functions {
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
    let wrapper = main_wrapper(program, debug)?;
    x86::check_instructions("main", &wrapper)?;
    items.extend(wrapper);
    items.extend(frame_end(debug));
    items.extend(data::emit_data(program)?);
    // Without this note the linker assumes an executable stack.
    items.push(Item::Blank);
    items.push(Item::Directive(
        ".section .note.GNU-stack,\"\",@progbits".into(),
    ));
    Ok(print::file(&items))
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
