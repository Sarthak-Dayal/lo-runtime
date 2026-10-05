//! Byte layout of every runtime-visible structure (`runtime-abi.md` §2–3.3).
//!
//! This is the only place offsets are computed. Lowering asks it where to load
//! a field or the vtable from; the static-data builder asks it where to place
//! each descriptor field. Both read the same numbers, so they cannot disagree.
//!
//! Offsets follow C struct rules (each field starts at a multiple of its own
//! size, the struct's size rounds up to its largest field), which is what the
//! runtime's `#[repr(C)]` definitions in `rust/src/object.rs` produce. Nothing
//! is written down per target: every number derives from the pointer width.

use crate::ast::Type;
use crate::type_checker::ClassTable;

/// The machine the layout is for. Only the pointer width differs between targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub ptr: u32,
}

pub const NATIVE: Target = Target { ptr: 8 };
pub const WASM32: Target = Target { ptr: 4 };

const U32: u32 = 4;

fn align_up(offset: u32, align: u32) -> u32 {
    offset.div_ceil(align) * align
}

/// Lays out a C struct whose fields have the given sizes, each aligned to its
/// own size. Returns each field's offset and the struct's total size.
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

/// Offsets within a `ClassDescriptor` (`runtime-abi.md` §2.1).
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

/// Offsets within a `ShadowFrame` (`runtime-abi.md` §3.3). The roots array is
/// inline, starting at `roots`; root `k` lives at [`FrameLayout::root`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameLayout {
    pub parent: u32,
    pub num_roots: u32,
    pub roots: u32,
    ptr: u32,
}

impl FrameLayout {
    pub fn root(&self, slot: u32) -> u32 {
        self.roots + slot * self.ptr
    }

    /// Bytes a frame with `slots` roots occupies.
    pub fn size(&self, slots: u32) -> u32 {
        self.root(slots)
    }
}

impl Target {
    /// `Object { class_descriptor: ptr, gc_bits: u32, flags: u32 }`.
    pub fn header_size(self) -> u32 {
        c_struct([self.ptr, U32, U32]).1
    }

    /// Every field gets a pointer-sized slot, whatever its type, so a field's
    /// offset depends only on its index and pointer fields are always aligned.
    pub fn slot_size(self) -> u32 {
        self.ptr
    }

    pub fn field_offset(self, index: usize) -> u32 {
        self.header_size() + self.slot_size() * index as u32
    }

    /// `StringObject { header, length: u32, data }` — the length follows the header.
    pub fn string_length_offset(self) -> u32 {
        self.header_size()
    }

    pub fn descriptor(self) -> DescriptorLayout {
        let p = self.ptr;
        let (
            [name, name_len, parent, instance_size, pointer_offsets, pointer_count, vtable_size, vtable],
            size,
        ) = c_struct([p, U32, p, U32, p, U32, U32, p]);
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

    /// Offset of method slot `slot` within a vtable (an array of code pointers).
    pub fn vtable_entry(self, slot: usize) -> u32 {
        self.ptr * slot as u32
    }

    pub fn frame(self) -> FrameLayout {
        let ([parent, num_roots, roots], _) = c_struct([self.ptr, U32, self.ptr]);
        FrameLayout {
            parent,
            num_roots,
            roots,
            ptr: self.ptr,
        }
    }
}

/// One field of a class, inherited fields first, as the runtime sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldSlot {
    pub name: String,
    pub owner: String,
    pub ty: Type,
    pub offset: u32,
}

/// Where everything in one class's objects lives.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassLayout {
    pub fields: Vec<FieldSlot>,
    /// Output's hidden stdout/stderr selector (0 or 1), read by its `print_*`
    /// bodies and passed to the runtime as `to_stderr`. A trailing slot after
    /// the declared fields; `None` for every other class.
    pub output_destination: Option<u32>,
    /// Total object size including the header, as `lo_alloc` allocates it.
    pub instance_size: u32,
    /// Offsets of String- and class-typed fields, for the collector to follow.
    pub pointer_offsets: Vec<u32>,
    /// Offsets of String fields, which must be set to `LO_EMPTY_STRING` right
    /// after `lo_alloc` (it zero-fills, and a String's default is "" not null).
    pub string_offsets: Vec<u32>,
}

impl ClassLayout {
    pub fn field(&self, name: &str) -> Option<&FieldSlot> {
        self.fields.iter().find(|field| field.name == name)
    }
}

/// The preamble class whose objects carry a destination word (`add_io_classes`).
const OUTPUT_CLASS: &str = "Output";

/// Lays out `class`. Field order is the type checker's `effective_fields`, so an
/// inherited field keeps its parent's offset and a subclass object can be used
/// wherever a parent object can.
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
    let output_destination = (class == OUTPUT_CLASS).then(|| target.field_offset(fields.len()));
    let slots = fields.len() + usize::from(output_destination.is_some());
    ClassLayout {
        instance_size: target.field_offset(slots),
        pointer_offsets,
        string_offsets,
        output_destination,
        fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::check;

    #[test]
    fn native_offsets_match_runtime_abi() {
        // The x86-64 numbers from runtime-abi.md §2–3.3 / rust/src/object.rs.
        assert_eq!(NATIVE.header_size(), 16);
        assert_eq!(
            NATIVE.descriptor(),
            DescriptorLayout {
                name: 0,
                name_len: 8,
                parent: 16,
                instance_size: 24,
                pointer_offsets: 32,
                pointer_count: 40,
                vtable_size: 44,
                vtable: 48,
                size: 56,
            }
        );
        let frame = NATIVE.frame();
        assert_eq!((frame.parent, frame.num_roots, frame.roots), (0, 8, 16));
        assert_eq!((frame.root(0), frame.root(3), frame.size(3)), (16, 40, 40));
        assert_eq!(NATIVE.vtable_entry(2), 16);
        assert_eq!(NATIVE.string_length_offset(), 16);
    }

    #[test]
    fn wasm_offsets_reproduce_p1() {
        // The numbers P1's WASM back end hardcodes (wasm/module.rs, wasm/function.rs),
        // proving this module encodes the same layout P1 already ships.
        assert_eq!(WASM32.header_size(), 12);
        let d = WASM32.descriptor();
        assert_eq!(
            [d.name, d.name_len, d.parent, d.instance_size],
            [0, 4, 8, 12]
        );
        assert_eq!(
            [
                d.pointer_offsets,
                d.pointer_count,
                d.vtable_size,
                d.vtable,
                d.size
            ],
            [16, 20, 24, 28, 32]
        );
        let frame = WASM32.frame();
        assert_eq!((frame.num_roots, frame.root(0), frame.root(2)), (4, 8, 16));
        assert_eq!(WASM32.field_offset(3), 12 + 4 * 3);
    }

    const SHAPES: &str = "
        class Main () { int main() { return 0; } }
        class Shape ( int id; ) { int area() { return 0; } }
        class Circle extends Shape ( int r; String name; Shape next; ) [
            Circle(int i, int radius) { super(i); r = radius; }
        ] {
            int area() { return (r * r); }
        }
    ";

    #[test]
    fn inherited_fields_keep_parent_offsets() {
        let table = check(SHAPES);
        let shape = class_layout(NATIVE, &table, "Shape");
        let circle = class_layout(NATIVE, &table, "Circle");
        assert_eq!(shape.field("id").unwrap().offset, 16);
        assert_eq!(circle.field("id").unwrap().offset, 16);
        assert_eq!(circle.field("id").unwrap().owner, "Shape");
        assert_eq!(circle.field("r").unwrap().offset, 24);
        assert_eq!(circle.field("name").unwrap().offset, 32);
        assert_eq!(circle.field("next").unwrap().offset, 40);
        assert_eq!(circle.instance_size, 48);
        assert_eq!(shape.instance_size, 24);
    }

    #[test]
    fn pointer_and_string_fields_are_listed() {
        let circle = class_layout(NATIVE, &check(SHAPES), "Circle");
        assert_eq!(circle.pointer_offsets, vec![32, 40]);
        assert_eq!(circle.string_offsets, vec![32]);
        assert_eq!(circle.output_destination, None);
    }

    #[test]
    fn output_has_a_trailing_destination_slot() {
        let table = check(SHAPES);
        let output = class_layout(NATIVE, &table, "Output");
        assert!(output.fields.is_empty());
        assert_eq!(output.output_destination, Some(16));
        assert_eq!(output.instance_size, 24);
        // P1: size 12 + 4·0 + 4, destination read with `i32.load 12`.
        let wasm = class_layout(WASM32, &table, "Output");
        assert_eq!(
            (wasm.output_destination, wasm.instance_size),
            (Some(12), 16)
        );
    }
}
