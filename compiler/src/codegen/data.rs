//! Static data: `DataDef` to GNU assembler directives.
//!
//! Read-only data (descriptors, vtables, name bytes) goes to `.rodata`, writable
//! data (the `lo_bindings` shadow frame) to `.data`. Addresses are 8-byte
//! relocations (`.quad`); native pointers are 64-bit.

use crate::ir::{DataDef, DataItem, ProgramIr, Section};

use super::x86::Item;

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
        Item::Directive(String::new()),
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
                for chunk in bytes.chunks(16) {
                    let list: Vec<String> = chunk.iter().map(|b| format!("0x{b:02x}")).collect();
                    out.push(Item::Directive(format!("    .byte {}", list.join(", "))));
                }
            }
            DataItem::Zero(n) => out.push(Item::Directive(format!("    .zero {n}"))),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::print::item_text;
    use crate::ir::{SymbolId, SymbolKind};

    fn text(items: &[Item]) -> Vec<String> {
        items
            .iter()
            .map(item_text)
            .filter(|l| !l.is_empty())
            .collect()
    }

    #[test]
    fn every_item_kind_has_a_directive() {
        let mut p = ProgramIr {
            symbols: vec![],
            signatures: vec![],
            functions: vec![],
            data: vec![],
            startup: None,
        };
        let target = p
            .declare_symbol("lo_class_4_Main", SymbolKind::Data)
            .unwrap();
        let blob = p.declare_symbol("lo_bytes_0", SymbolKind::Data).unwrap();
        let frame = p.declare_symbol("lo_bindings", SymbolKind::Data).unwrap();
        p.data.push(DataDef {
            symbol: blob,
            section: Section::ReadOnly,
            align: 1,
            items: vec![DataItem::Bytes(b"Main\0".to_vec())],
        });
        p.data.push(DataDef {
            symbol: frame,
            section: Section::Writable,
            align: 8,
            items: vec![DataItem::Addr(target), DataItem::U32(3), DataItem::Zero(4)],
        });
        assert_eq!(
            text(&emit_data(&p).unwrap()),
            [
                ".section .rodata",
                ".balign 1",
                "lo_bytes_0:",
                "    .byte 0x4d, 0x61, 0x69, 0x6e, 0x00",
                ".data",
                ".balign 8",
                "lo_bindings:",
                "    .quad lo_class_4_Main",
                "    .long 3",
                "    .zero 4",
            ]
        );
    }

    #[test]
    fn long_byte_runs_wrap_at_sixteen() {
        let mut p = ProgramIr {
            symbols: vec![],
            signatures: vec![],
            functions: vec![],
            data: vec![],
            startup: None,
        };
        let s = p.declare_symbol("b", SymbolKind::Data).unwrap();
        p.data.push(DataDef {
            symbol: s,
            section: Section::ReadOnly,
            align: 1,
            items: vec![DataItem::Bytes(vec![7; 20])],
        });
        let lines = text(&emit_data(&p).unwrap());
        assert_eq!(lines.iter().filter(|l| l.contains(".byte")).count(), 2);
    }

    #[test]
    fn dangling_relocation_is_an_error() {
        let mut p = ProgramIr {
            symbols: vec![],
            signatures: vec![],
            functions: vec![],
            data: vec![],
            startup: None,
        };
        let s = p.declare_symbol("d", SymbolKind::Data).unwrap();
        p.data.push(DataDef {
            symbol: s,
            section: Section::ReadOnly,
            align: 8,
            items: vec![DataItem::Addr(SymbolId(99))],
        });
        assert!(emit_data(&p).is_err());
    }
}
