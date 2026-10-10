//! Native x86-64 back end: Intel-syntax assembly for the System V AMD64 ABI.
//! See planning/p2_docs/p2-codegen-overview.md.

pub mod data;
pub mod driver;
pub mod frame;
pub mod moves;
pub mod print;
pub mod select;
pub mod x86;

use crate::ir::register_allocator::ProgramAllocation;
use crate::ir::ProgramIr;

use x86::{Inst, Item, Operand, Reg};

/// The whole assembly file for a verified, rooted program and its allocation.
pub fn emit_program(program: &ProgramIr, allocation: &ProgramAllocation) -> Result<String, String> {
    let mut items = vec![];
    for function in &program.functions {
        let allocation = allocation
            .function_allocations
            .get(&function.symbol)
            .ok_or_else(|| {
                format!(
                    "no allocation for {}",
                    program.symbols[function.symbol.0].name
                )
            })?;
        let is_startup = program.startup == Some(function.symbol);
        items.extend(select::select_function(
            program, function, allocation, is_startup,
        )?);
    }
    items.extend(main_wrapper(program)?);
    items.extend(data::emit_data(program)?);
    // Without this note the linker assumes an executable stack.
    items.push(Item::Directive(String::new()));
    items.push(Item::Directive(
        ".section .note.GNU-stack,\"\",@progbits".into(),
    ));
    Ok(print::file(&items))
}

/// The C entry point. `gcc -static` supplies `_start`, which calls `main` and
/// passes its `eax` to `exit`, so `Main.main`'s result becomes the exit status.
fn main_wrapper(program: &ProgramIr) -> Result<Vec<Item>, String> {
    let startup = program
        .startup
        .ok_or("program has no startup function (lo_entry)")?;
    let entry = program.symbols[startup.0].name.clone();
    // `main` is entered with rsp = 8 (mod 16); push rbp makes it 0 for the call.
    Ok(vec![
        Item::Directive(String::new()),
        Item::Directive(".globl main".into()),
        Item::Label("main".into()),
        Item::Inst(Inst::Push(Operand::r64(Reg::Rbp))),
        Item::Inst(Inst::Mov(Operand::r64(Reg::Rbp), Operand::r64(Reg::Rsp))),
        Item::Inst(Inst::Call(entry)),
        Item::Inst(Inst::Pop(Operand::r64(Reg::Rbp))),
        Item::Inst(Inst::Ret),
    ])
}
