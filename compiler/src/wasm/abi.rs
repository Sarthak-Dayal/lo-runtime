//! WASM32 object/descriptor layout constants. See `design.md`'s "Object Layout"
//! and "Class Descriptors and Vtables" sections for the authoritative spec.

/// Every heap object's fields are four bytes wide (pointers and LO values are i32).
pub(super) const WORD_SIZE: usize = 4;

// Object header (offsets 0, 4, 8 are fixed by the ABI; fields start at 12).
pub(super) const CLASS_DESCRIPTOR_OFFSET: usize = 0;
pub(super) const GC_BITS_OFFSET: usize = 4;
pub(super) const RESERVED_FLAGS_OFFSET: usize = 8;
pub(super) const FIRST_FIELD_OFFSET: usize = 12;

/// `Output`'s compiler-private destination field lives at the first field offset.
pub(super) const OUTPUT_DESTINATION_OFFSET: usize = FIRST_FIELD_OFFSET;

// Class descriptor layout (read-only linear memory).
pub(super) const CLASS_NAME_PTR_OFFSET: usize = 0;
pub(super) const CLASS_NAME_LEN_OFFSET: usize = 4;
pub(super) const PARENT_DESCRIPTOR_OFFSET: usize = 8;
pub(super) const OBJECT_SIZE_OFFSET: usize = 12;
pub(super) const REF_FIELD_ARRAY_OFFSET: usize = 16;
pub(super) const NUM_REF_FIELDS_OFFSET: usize = 20;
pub(super) const NUM_VTABLE_ENTRIES_OFFSET: usize = 24;
pub(super) const VTABLE_ADDRESS_OFFSET: usize = 28;

/// Vtable entries are WebAssembly function-table indices, four bytes each.
pub(super) const VTABLE_ENTRY_SIZE: usize = 4;

// Call frame layout: parent pointer at 0, root count at 4, root slots from 8.
pub(super) const FRAME_HEADER_SIZE: usize = 8;
pub(super) const FRAME_PARENT_OFFSET: usize = 0;
pub(super) const FRAME_ROOT_COUNT_OFFSET: usize = 4;
pub(super) const FRAME_ROOTS_OFFSET: usize = FRAME_HEADER_SIZE;
pub(super) const ROOT_SLOT_SIZE: usize = WORD_SIZE;

/// The stack pointer must stay 16-byte aligned.
pub(super) const STACK_ALIGN: usize = 16;
