//! Static data: the bytes the runtime reads that codegen must define.
//!
//! For every class: its descriptor, vtable, pointer-offset array and name.
//! For the program: the three binding words behind `in`, `out` and `err`.
//! Every position inside a descriptor comes from `layout`, the same numbers
//! lowering uses to read it, so the data and the code that reads it agree.

use crate::layout::{class_layout, Target};
use crate::symbols;
use crate::type_checker::ClassTable;

use super::*;

/// Defines the static data for every class in `table`, plus the binding words.
pub fn define_static_data(
    program: &mut ProgramIr,
    table: &ClassTable,
    target: Target,
) -> Result<(), String> {
    for class in table.class_names() {
        define_class(program, table, target, class)?;
    }
    for name in symbols::PREBOUND {
        // Holds the address of the entry function's root slot for `name`
        // (filled in at startup), so it is writable and starts as zero.
        let symbol = program.declare_symbol(&symbols::binding(name), SymbolKind::Data)?;
        program.data.push(DataDef {
            symbol,
            section: Section::Writable,
            align: target.ptr,
            items: vec![DataItem::Zero(target.ptr)],
        });
    }
    Ok(())
}

fn define_class(
    program: &mut ProgramIr,
    table: &ClassTable,
    target: Target,
    class: &str,
) -> Result<(), String> {
    let info = table.get(class).expect("class from class_names");
    let layout = class_layout(target, table, class);
    let descriptor = target.descriptor();
    let mut fields = Placer::new(target, descriptor.size);

    let name = read_only(program, &symbols::class_name(class), 1, {
        let mut bytes = class.as_bytes().to_vec();
        bytes.push(0); // the ABI's name is NUL-terminated; name_len excludes it
        vec![DataItem::Bytes(bytes)]
    })?;
    fields.put(descriptor.name, DataItem::Addr(name));
    fields.put(descriptor.name_len, DataItem::U32(class.len() as u32));

    let parent = match &info.parent {
        Some(parent) => DataItem::Addr(
            program.declare_symbol(&symbols::class_descriptor(parent), SymbolKind::Data)?,
        ),
        None => null(target), // the root of the hierarchy
    };
    fields.put(descriptor.parent, parent);

    fields.put(
        descriptor.instance_size,
        DataItem::U32(layout.instance_size),
    );

    // An empty array is a null pointer: the runtime reads no entries when the
    // count is zero, so no symbol is needed.
    let offsets = &layout.pointer_offsets;
    let array = if offsets.is_empty() {
        null(target)
    } else {
        let items = offsets
            .iter()
            .map(|&offset| DataItem::U32(offset))
            .collect();
        DataItem::Addr(read_only(
            program,
            &symbols::pointer_offsets(class),
            4,
            items,
        )?)
    };
    fields.put(descriptor.pointer_offsets, array);
    fields.put(
        descriptor.pointer_count,
        DataItem::U32(offsets.len() as u32),
    );

    // Slot k holds the implementation `class` actually runs for method k:
    // an inherited method keeps the ancestor's code, an override its own.
    let mut entries = Vec::with_capacity(info.vtable.len());
    for method in &info.vtable {
        let entry = &info.effective_methods[method];
        entries.push(DataItem::Addr(
            program.declare_method(&entry.owner, &entry.sig)?,
        ));
    }
    fields.put(descriptor.vtable_size, DataItem::U32(entries.len() as u32));
    let vtable = if entries.is_empty() {
        null(target)
    } else {
        DataItem::Addr(read_only(
            program,
            &symbols::vtable(class),
            target.ptr,
            entries,
        )?)
    };
    fields.put(descriptor.vtable, vtable);

    let items = fields.finish();
    read_only(
        program,
        &symbols::class_descriptor(class),
        target.ptr,
        items,
    )?;
    Ok(())
}

/// A null pointer, written out explicitly so the dump shows it as its own item
/// rather than merged into neighbouring padding.
fn null(target: Target) -> DataItem {
    DataItem::Zero(target.ptr)
}

fn read_only(
    program: &mut ProgramIr,
    name: &str,
    align: u32,
    items: Vec<DataItem>,
) -> Result<SymbolId, String> {
    let symbol = program.declare_symbol(name, SymbolKind::Data)?;
    program.data.push(DataDef {
        symbol,
        section: Section::ReadOnly,
        align,
        items,
    });
    Ok(symbol)
}

fn width(target: Target, item: &DataItem) -> u32 {
    match item {
        DataItem::U32(_) => 4,
        DataItem::Addr(_) => target.ptr,
        DataItem::Bytes(bytes) => bytes.len() as u32,
        DataItem::Zero(size) => *size,
    }
}

/// Builds a fixed-size struct by placing each item at a named offset, rather
/// than relying on the order items are pushed. Gaps (C padding, null
/// pointers) become `Zero`.
struct Placer {
    target: Target,
    size: u32,
    items: Vec<(u32, DataItem)>,
}

impl Placer {
    fn new(target: Target, size: u32) -> Self {
        Placer {
            target,
            size,
            items: Vec::new(),
        }
    }

    fn put(&mut self, offset: u32, item: DataItem) {
        self.items.push((offset, item));
    }

    fn finish(mut self) -> Vec<DataItem> {
        self.items.sort_by_key(|(offset, _)| *offset);
        let mut out = Vec::new();
        let mut cursor = 0;
        for (offset, item) in self.items {
            assert!(offset >= cursor, "overlapping fields at offset {offset}");
            if offset > cursor {
                out.push(DataItem::Zero(offset - cursor));
            }
            cursor = offset + width(self.target, &item);
            out.push(item);
        }
        assert!(cursor <= self.size, "fields overrun the struct");
        if self.size > cursor {
            out.push(DataItem::Zero(self.size - cursor));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{NATIVE, WASM32};
    use crate::test_support::check;

    fn empty_program() -> ProgramIr {
        ProgramIr {
            symbols: vec![],
            signatures: vec![],
            functions: vec![],
            data: vec![],
            startup: None,
        }
    }

    fn built(source: &str, target: Target) -> ProgramIr {
        let mut program = empty_program();
        define_static_data(&mut program, &check(source), target).unwrap();
        program
    }

    fn data<'p>(program: &'p ProgramIr, name: &str) -> &'p DataDef {
        let symbol = program
            .find_symbol(name)
            .unwrap_or_else(|| panic!("no {name}"));
        program
            .data
            .iter()
            .find(|data| data.symbol == symbol)
            .unwrap_or_else(|| panic!("{name} is declared but not defined"))
    }

    fn name_of(program: &ProgramIr, item: &DataItem) -> String {
        match item {
            DataItem::Addr(id) => program.symbols[id.0].name.clone(),
            other => panic!("expected an address, got {other:?}"),
        }
    }

    /// The item starting at byte `offset` of a struct.
    fn at(target: Target, def: &DataDef, offset: u32) -> DataItem {
        let mut position = 0;
        for item in &def.items {
            if position == offset {
                return item.clone();
            }
            position += width(target, item);
        }
        panic!("no item starts at offset {offset}")
    }

    const SHAPES: &str = "
        class Main () { int main() { return 0; } }
        class Shape ( int id; ) { int area() { return 0; } int sides() { return 0; } }
        class Circle extends Shape ( String name; ) [
            Circle(int i) { super(i); }
        ] {
            int area() { return 1; }
            int radius() { return 2; }
        }
    ";

    #[test]
    fn descriptor_fields_sit_at_layout_offsets() {
        let p = built(SHAPES, NATIVE);
        let d = NATIVE.descriptor();
        let circle = data(&p, "lo_class_6_Circle");
        assert_eq!(circle.section, Section::ReadOnly);
        assert_eq!(circle.align, 8);
        let total: u32 = circle.items.iter().map(|i| width(NATIVE, i)).sum();
        assert_eq!(total, 56);
        assert_eq!(
            name_of(&p, &at(NATIVE, circle, d.name)),
            "lo_class_6_Circle_name"
        );
        assert_eq!(at(NATIVE, circle, d.name_len), DataItem::U32(6));
        assert_eq!(
            name_of(&p, &at(NATIVE, circle, d.parent)),
            "lo_class_5_Shape"
        );
        assert_eq!(at(NATIVE, circle, d.instance_size), DataItem::U32(32));
        assert_eq!(
            name_of(&p, &at(NATIVE, circle, d.pointer_offsets)),
            "lo_class_6_Circle_pointers"
        );
        assert_eq!(at(NATIVE, circle, d.pointer_count), DataItem::U32(1));
        assert_eq!(at(NATIVE, circle, d.vtable_size), DataItem::U32(3));
        assert_eq!(
            name_of(&p, &at(NATIVE, circle, d.vtable)),
            "lo_class_6_Circle_vtable"
        );
    }

    #[test]
    fn vtable_keeps_inherited_slots_and_overrides_in_place() {
        let p = built(SHAPES, NATIVE);
        let vtable = data(&p, "lo_class_6_Circle_vtable");
        let names: Vec<_> = vtable.items.iter().map(|item| name_of(&p, item)).collect();
        assert_eq!(
            names,
            [
                "lo_method_6_Circle_4_area",   // slot 0: overridden
                "lo_method_5_Shape_5_sides",   // slot 1: inherited, Shape's code
                "lo_method_6_Circle_6_radius", // slot 2: new in Circle
            ]
        );
    }

    #[test]
    fn method_symbols_carry_receiver_first_signatures() {
        let p = built(SHAPES, NATIVE);
        let area = p.find_symbol("lo_method_6_Circle_4_area").unwrap();
        let SymbolKind::Function(sig) = p.symbols[area.0].kind else {
            panic!("area is not a function")
        };
        let sig = &p.signatures[sig.0];
        assert_eq!(sig.params, vec![IrType::Ref]);
        assert_eq!(sig.result, Some(IrType::Int32));
    }

    #[test]
    fn pointer_offsets_and_names_are_defined() {
        let p = built(SHAPES, NATIVE);
        assert_eq!(
            data(&p, "lo_class_6_Circle_pointers").items,
            vec![DataItem::U32(24)]
        );
        assert_eq!(
            data(&p, "lo_class_6_Circle_name").items,
            vec![DataItem::Bytes(b"Circle\0".to_vec())]
        );
    }

    #[test]
    fn root_class_and_empty_arrays_are_null() {
        let p = built(SHAPES, NATIVE);
        let d = NATIVE.descriptor();
        // Main: no parent, no pointer fields.
        let main = data(&p, "lo_class_4_Main");
        assert_eq!(at(NATIVE, main, d.parent), DataItem::Zero(8));
        assert_eq!(at(NATIVE, main, d.pointer_count), DataItem::U32(0));
        assert!(p.find_symbol("lo_class_4_Main_pointers").is_none());
        // Shape: no pointer fields; its descriptor has no array to point at.
        assert!(p.find_symbol("lo_class_5_Shape_pointers").is_none());
    }

    #[test]
    fn bindings_are_writable_pointer_words() {
        let p = built(SHAPES, NATIVE);
        for name in ["lo_binding_in", "lo_binding_out", "lo_binding_err"] {
            let binding = data(&p, name);
            assert_eq!(binding.section, Section::Writable);
            assert_eq!(binding.items, vec![DataItem::Zero(8)]);
        }
    }

    #[test]
    fn output_descriptor_counts_its_destination_slot() {
        let p = built(SHAPES, NATIVE);
        let output = data(&p, "lo_class_6_Output");
        assert_eq!(
            at(NATIVE, output, NATIVE.descriptor().instance_size),
            DataItem::U32(24)
        );
    }

    #[test]
    fn wasm_layout_reproduces_p1_descriptor() {
        // P1 (wasm/module.rs) writes eight 4-byte words: name, len, parent, size,
        // pointers, count, vtable size, vtable.
        let p = built(SHAPES, WASM32);
        let circle = data(&p, "lo_class_6_Circle");
        let total: u32 = circle.items.iter().map(|i| width(WASM32, i)).sum();
        assert_eq!(total, 32);
        assert_eq!(at(WASM32, circle, 12), DataItem::U32(12 + 4 * 2));
        assert_eq!(
            name_of(&p, &at(WASM32, circle, 28)),
            "lo_class_6_Circle_vtable"
        );
    }

    #[test]
    fn result_verifies() {
        built(SHAPES, NATIVE).verify().unwrap();
    }

    #[test]
    #[should_panic(expected = "overlapping")]
    fn placer_rejects_overlap() {
        let mut placer = Placer::new(NATIVE, 16);
        placer.put(0, DataItem::Addr(SymbolId(0)));
        placer.put(4, DataItem::U32(1));
        placer.finish();
    }
}
