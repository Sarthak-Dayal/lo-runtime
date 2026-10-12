//! Static data: `DataDef` to GNU assembler directives.
//!
//! Read-only data (descriptors, vtables, name bytes) goes to `.rodata`, writable
//! data (the `lo_bindings` shadow frame) to `.data`. Addresses are 8-byte
//! relocations (`.quad`); native pointers are 64-bit.

use crate::ir::{DataDef, DataItem, ProgramIr, Section};

use super::x86::Item;

const BYTES_PER_LINE: usize = 16;

pub fn emit_data(program: &ProgramIr) -> Result<Vec<Item>, String> {
    let mut out = vec![];
    for def in &program.data {
        out.extend(emit_def(program, def)?);
    }
    Ok(out)
}

fn emit_def(program: &ProgramIr, def: &DataDef) -> Result<Vec<Item>, String> {
    let name = &program.symbols[def.symbol.0].name;
    let section = match def.section {
        Section::ReadOnly => ".section .rodata",
        Section::Writable => ".data",
    };
    let mut out = vec![
        Item::Blank,
        Item::Directive(section.into()),
        Item::Directive(format!(".balign {}", def.align)),
        Item::Label(name.clone()),
    ];
    for item in &def.items {
        match item {
            DataItem::U32(n) => out.push(Item::Directive(format!("    .long {n}"))),
            DataItem::Addr(target) => {
                let target = program
                    .symbols
                    .get(target.0)
                    .ok_or_else(|| format!("{name}: relocation to unknown symbol"))?;
                out.push(Item::Directive(format!("    .quad {}", target.name)));
            }
            DataItem::Bytes(bytes) => {
                for chunk in bytes.chunks(BYTES_PER_LINE) {
                    let list: Vec<String> = chunk.iter().map(|b| format!("0x{b:02x}")).collect();
                    out.push(Item::Directive(format!("    .byte {}", list.join(", "))));
                }
            }
            DataItem::Zero(n) => out.push(Item::Directive(format!("    .zero {n}"))),
        }
    }
    Ok(out)
}
