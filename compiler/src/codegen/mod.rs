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
        let function_items = select::select_function(program, function, function_allocation)?;
        items.extend(with_frame_info(function_items, debug)?);
    }
    let wrapper = main_wrapper(program)?;
    x86::check_instructions("main", &wrapper)?;
    items.extend(with_frame_info(wrapper, debug)?);
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
fn main_wrapper(program: &ProgramIr) -> Result<Vec<Item>, String> {
    let startup = program
        .startup
        .ok_or("program has no startup function (lo_entry)")?;
    let entry = program.symbols[startup.0].name.clone();
    // `main` is entered with rsp = 8 (mod 16); push rbp makes it 0 for the calls.
    Ok(vec![
        Item::Blank,
        Item::Directive(".globl main".into()),
        Item::Label("main".into()),
        Item::Inst(Inst::Push(Operand::r64(Reg::Rbp))),
        Item::Inst(Inst::Mov(Operand::r64(Reg::Rbp), Operand::r64(Reg::Rsp))),
        Item::Inst(Inst::Call("lo_runtime_init".into())),
        Item::Inst(Inst::Call(entry)),
        Item::Inst(Inst::Pop(Operand::r64(Reg::Rbp))),
        Item::Inst(Inst::Ret),
    ])
}

/// Under `-g`, adds the call-frame information a debugger needs to walk the
/// stack (DWARF Subset Reference §9.6): where the return address and the
/// caller's `rbp` are, at each instruction. Every function starts with
/// `push rbp ; mov rbp, rsp`, which gives three states:
///
/// ```text
/// .cfi_startproc               entry: return address just above rsp
/// push rbp
/// .cfi_def_cfa_offset 16       return address 16 above rsp, caller's rbp 16 below
/// .cfi_offset 6, -16           (6 is rbp's DWARF register number)
/// mov rbp, rsp
/// .cfi_def_cfa_register 6      from here on, both are found relative to rbp
/// ...
/// .cfi_endproc
/// ```
///
/// §9.6 asks for nothing at the epilogue: the debugger only stops at statement
/// boundaries, never between `leave` and `ret`.
fn with_frame_info(items: Vec<Item>, debug: DebugMode) -> Result<Vec<Item>, String> {
    if debug == DebugMode::Off {
        return Ok(items);
    }
    let is = |item: &Item, inst: &Inst| matches!(item, Item::Inst(i) if i == inst);
    let push_rbp = Inst::Push(Operand::r64(Reg::Rbp));
    let set_rbp = Inst::Mov(Operand::r64(Reg::Rbp), Operand::r64(Reg::Rsp));
    let push = items.iter().position(|item| is(item, &push_rbp));
    let set = items.iter().position(|item| is(item, &set_rbp));
    let (Some(push), Some(set)) = (push, set) else {
        return Err("function does not start with push rbp ; mov rbp, rsp".into());
    };
    let directive = |text: &str| Item::Directive(format!("    {text}"));
    let mut out = Vec::with_capacity(items.len() + 5);
    for (index, item) in items.into_iter().enumerate() {
        if index == push {
            out.push(directive(".cfi_startproc"));
        }
        out.push(item);
        if index == push {
            out.push(directive(".cfi_def_cfa_offset 16"));
            out.push(directive(".cfi_offset 6, -16"));
        }
        if index == set {
            out.push(directive(".cfi_def_cfa_register 6"));
        }
    }
    out.push(directive(".cfi_endproc"));
    Ok(out)
}
