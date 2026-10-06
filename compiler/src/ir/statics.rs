//! Static data: each class's descriptor, vtable, pointer-offset array and
//! name, plus the `in`/`out`/`err` binding words. Descriptor fields are placed
//! at `layout` offsets, the same numbers lowering reads them from.

use crate::layout::{class_layout, Target};
use crate::symbols;
use crate::type_checker::ClassTable;

use super::*;

/// Builds a fixed-size struct by placing items at offsets; gaps become `Zero`.
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

pub fn define_static_data(
    program: &mut ProgramIr,
    table: &ClassTable,
    target: Target,
) -> Result<(), String> {
    for class in table.class_names() {
        define_class(program, table, target, class)?;
    }
    // Each holds the address of an entry-function root slot, written at startup.
    for name in symbols::PREBOUND {
        let symbol = program.declare_symbol(&symbols::binding(name), SymbolKind::Data)?;
        program.data.push(DataDef {
            symbol,
            section: Section::Writable,
            align: target.ptr,
            items: vec![null(target)],
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

    let mut name_bytes = class.as_bytes().to_vec();
    name_bytes.push(0);
    let name = read_only(
        program,
        &symbols::class_name(class),
        1,
        vec![DataItem::Bytes(name_bytes)],
    )?;
    fields.put(descriptor.name, DataItem::Addr(name));
    fields.put(descriptor.name_len, DataItem::U32(class.len() as u32));

    let parent = match &info.parent {
        Some(parent) => DataItem::Addr(
            program.declare_symbol(&symbols::class_descriptor(parent), SymbolKind::Data)?,
        ),
        None => null(target),
    };
    fields.put(descriptor.parent, parent);
    fields.put(
        descriptor.instance_size,
        DataItem::U32(layout.instance_size),
    );

    let offsets = &layout.pointer_offsets;
    let array = if offsets.is_empty() {
        null(target)
    } else {
        let items = offsets.iter().map(|&o| DataItem::U32(o)).collect();
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

    // An inherited method keeps the ancestor's code in its slot.
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

    read_only(
        program,
        &symbols::class_descriptor(class),
        target.ptr,
        fields.finish(),
    )?;
    Ok(())
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

fn null(target: Target) -> DataItem {
    DataItem::Zero(target.ptr)
}

fn width(target: Target, item: &DataItem) -> u32 {
    match item {
        DataItem::U32(_) => 4,
        DataItem::Addr(_) => target.ptr,
        DataItem::Bytes(bytes) => bytes.len() as u32,
        DataItem::Zero(size) => *size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tests::check;
    use crate::layout::NATIVE;

    const SHAPES: &str = "
        class Main () { int main() { return 0; } }
        class Shape ( int id; ) { int area() { return 0; } int sides() { return 0; } }
        class Circle extends Shape ( String name; ) [ Circle(int i) { super(i); } ] {
            int area() { return 1; }
            int radius() { return 2; }
        }
    ";

    fn built() -> ProgramIr {
        let mut program = ProgramIr {
            symbols: vec![],
            signatures: vec![],
            functions: vec![],
            data: vec![],
            startup: None,
        };
        define_static_data(&mut program, &check(SHAPES), NATIVE).unwrap();
        program.validate().unwrap();
        program
    }

    fn data<'p>(program: &'p ProgramIr, name: &str) -> &'p DataDef {
        let symbol = program.find_symbol(name).unwrap();
        program.data.iter().find(|d| d.symbol == symbol).unwrap()
    }

    fn name_of(program: &ProgramIr, item: &DataItem) -> String {
        let DataItem::Addr(id) = item else {
            panic!("expected an address, got {item:?}")
        };
        program.symbols[id.0].name.clone()
    }

    /// The item starting at byte `offset` of a struct.
    fn at(def: &DataDef, offset: u32) -> &DataItem {
        let mut position = 0;
        for item in &def.items {
            if position == offset {
                return item;
            }
            position += width(NATIVE, item);
        }
        panic!("no item starts at offset {offset}")
    }

    #[test]
    fn descriptor_fields_sit_at_layout_offsets() {
        let p = built();
        let d = NATIVE.descriptor();
        let circle = data(&p, "lo_class_6_Circle");
        let size: u32 = circle.items.iter().map(|i| width(NATIVE, i)).sum();
        assert_eq!(size, d.size);
        assert_eq!(name_of(&p, at(circle, d.name)), "lo_class_6_Circle_name");
        assert_eq!(at(circle, d.name_len), &DataItem::U32(6));
        assert_eq!(name_of(&p, at(circle, d.parent)), "lo_class_5_Shape");
        assert_eq!(at(circle, d.instance_size), &DataItem::U32(32));
        assert_eq!(
            name_of(&p, at(circle, d.pointer_offsets)),
            "lo_class_6_Circle_pointers"
        );
        assert_eq!(at(circle, d.pointer_count), &DataItem::U32(1));
        assert_eq!(at(circle, d.vtable_size), &DataItem::U32(3));
        assert_eq!(
            name_of(&p, at(circle, d.vtable)),
            "lo_class_6_Circle_vtable"
        );
        assert_eq!(
            data(&p, "lo_class_6_Circle_pointers").items,
            [DataItem::U32(24)]
        );
    }

    #[test]
    fn vtable_keeps_inherited_slots_and_overrides_in_place() {
        let p = built();
        let vtable = data(&p, "lo_class_6_Circle_vtable");
        let names: Vec<_> = vtable.items.iter().map(|i| name_of(&p, i)).collect();
        assert_eq!(
            names,
            [
                "lo_method_6_Circle_4_area",
                "lo_method_5_Shape_5_sides",
                "lo_method_6_Circle_6_radius",
            ]
        );
    }

    #[test]
    fn root_class_has_null_parent_and_no_pointer_array() {
        let p = built();
        let d = NATIVE.descriptor();
        let main = data(&p, "lo_class_4_Main");
        assert_eq!(at(main, d.parent), &DataItem::Zero(8));
        assert_eq!(at(main, d.pointer_offsets), &DataItem::Zero(8));
        assert!(p.find_symbol("lo_class_4_Main_pointers").is_none());
        assert_eq!(data(&p, "lo_binding_out").section, Section::Writable);
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
