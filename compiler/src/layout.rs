use crate::ast::Type;
use crate::type_checker::ClassTable;

pub const NATIVE: Target = Target { ptr: 8 };

pub const U32_SIZE: u32 = std::mem::size_of::<u32>() as u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub ptr: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DescriptorLayout {
    pub name: u32,
    pub name_len: u32,
    pub parent: u32,
    pub instance_size: u32,
    pub pointer_offsets: u32,
    pub pointer_count: u32,
    pub vtable_size: u32,
    pub vtable: u32,
    pub size: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameLayout {
    pub parent: u32,
    pub num_roots: u32,
    pub roots: u32,
    ptr: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldSlot {
    pub name: String,
    pub owner: String,
    pub ty: Type,
    pub offset: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassLayout {
    pub fields: Vec<FieldSlot>,
    pub output_destination: Option<u32>,
    pub instance_size: u32,
    pub pointer_offsets: Vec<u32>,
    pub string_offsets: Vec<u32>,
}

impl Target {
    /// `Object { class_descriptor: ptr, gc_bits: u32, flags: u32 }`.
    pub fn header_size(self) -> u32 {
        let (_, size) = c_struct([self.ptr, U32_SIZE, U32_SIZE]);
        size
    }

    pub fn field_offset(self, index: usize) -> u32 {
        self.header_size() + self.ptr * index as u32
    }

    pub fn descriptor(self) -> DescriptorLayout {
        let p = self.ptr;
        let (
            [name, name_len, parent, instance_size, pointer_offsets, pointer_count, vtable_size, vtable],
            size,
        ) = c_struct([p, U32_SIZE, p, U32_SIZE, p, U32_SIZE, U32_SIZE, p]);
        DescriptorLayout {
            name,
            name_len,
            parent,
            instance_size,
            pointer_offsets,
            pointer_count,
            vtable_size,
            vtable,
            size,
        }
    }

    pub fn vtable_entry(self, slot: usize) -> u32 {
        self.ptr * slot as u32
    }

    pub fn frame(self) -> FrameLayout {
        let ([parent, num_roots, roots], _) = c_struct([self.ptr, U32_SIZE, self.ptr]);
        FrameLayout {
            parent,
            num_roots,
            roots,
            ptr: self.ptr,
        }
    }
}

impl FrameLayout {
    pub fn root(&self, slot: u32) -> u32 {
        self.roots + slot * self.ptr
    }

    pub fn size(&self, slots: u32) -> u32 {
        self.root(slots)
    }
}

impl ClassLayout {
    pub fn field(&self, name: &str) -> Option<&FieldSlot> {
        self.fields.iter().find(|field| field.name == name)
    }
}

pub fn class_layout(target: Target, table: &ClassTable, class: &str) -> ClassLayout {
    let info = table
        .get(class)
        .unwrap_or_else(|| panic!("class_layout: unknown class {class}"));
    let fields: Vec<FieldSlot> = info
        .effective_fields
        .iter()
        .enumerate()
        .map(|(index, field)| FieldSlot {
            name: field.name.clone(),
            owner: field.owner.clone(),
            ty: field.ty.clone(),
            offset: target.field_offset(index),
        })
        .collect();
    let offsets_where = |keep: fn(&Type) -> bool| -> Vec<u32> {
        fields
            .iter()
            .filter(|field| keep(&field.ty))
            .map(|field| field.offset)
            .collect()
    };
    let pointer_offsets = offsets_where(|ty| matches!(ty, Type::String | Type::Class(_)));
    let string_offsets = offsets_where(|ty| *ty == Type::String);
    let output_destination = (class == "Output").then(|| target.field_offset(fields.len()));
    let slots = fields.len() + usize::from(output_destination.is_some());
    ClassLayout {
        instance_size: target.field_offset(slots),
        pointer_offsets,
        string_offsets,
        output_destination,
        fields,
    }
}

fn align_up(offset: u32, align: u32) -> u32 {
    offset.div_ceil(align) * align
}

/// C struct field offsets and total size.
fn c_struct<const N: usize>(sizes: [u32; N]) -> ([u32; N], u32) {
    let mut offsets = [0; N];
    let mut end = 0;
    for (offset, size) in offsets.iter_mut().zip(sizes) {
        *offset = align_up(end, size);
        end = *offset + size;
    }
    let max_align = sizes.into_iter().max().unwrap_or(1);
    (offsets, align_up(end, max_align))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn check(source: &str) -> ClassTable {
        let tokens = crate::lexer::tokenize(source).expect("lex");
        let program = crate::parser::parse_program(&tokens)
            .unwrap_or_else(|e| panic!("parse: {}", e.message));
        crate::type_checker::check_program(program)
            .unwrap_or_else(|e| panic!("type check: {}", e.message))
            .1
    }

    #[test]
    fn native_offsets_match_runtime_abi() {
        assert_eq!(NATIVE.header_size(), 16);
        assert_eq!(NATIVE.field_offset(2), 32);
        assert_eq!(NATIVE.vtable_entry(2), 16);
        let d = NATIVE.descriptor();
        assert_eq!(
            [
                d.name,
                d.name_len,
                d.parent,
                d.instance_size,
                d.pointer_offsets
            ],
            [0, 8, 16, 24, 32]
        );
        assert_eq!(
            [d.pointer_count, d.vtable_size, d.vtable, d.size],
            [40, 44, 48, 56]
        );
        let frame = NATIVE.frame();
        assert_eq!([frame.num_roots, frame.root(0), frame.root(1)], [8, 16, 24]);
    }

    #[test]
    fn class_layout_places_inherited_and_pointer_fields() {
        let table = check(
            "class Main () { int main() { return 0; } }
             class Shape ( int id; ) { int area() { return 0; } }
             class Circle extends Shape ( String name; Shape next; ) [
                 Circle(int i) { super(i); }
             ] { int area() { return 1; } }",
        );
        let circle = class_layout(NATIVE, &table, "Circle");
        let offsets: Vec<_> = circle
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.offset))
            .collect();
        assert_eq!(offsets, [("id", 16), ("name", 24), ("next", 32)]);
        assert_eq!(circle.instance_size, 40);
        assert_eq!(circle.pointer_offsets, [24, 32]);
        assert_eq!(circle.string_offsets, [24]);

        let output = class_layout(NATIVE, &table, "Output");
        assert_eq!(
            (output.output_destination, output.instance_size),
            (Some(16), 24)
        );
    }
}
