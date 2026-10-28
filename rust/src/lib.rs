//! LO runtime skeleton (Rust) — CS 378H Compilers, Fall 2026.
//!
//! Implements the shared runtime ABI with Lisp-2 mark-compact collection.
//! The optional `reference-cheney` feature selects the reference collector for tests.
//! String and type operations share the native and WASM implementation.

mod abort;
#[cfg_attr(feature = "reference-cheney", path = "../reference/semispace_alloc.rs")]
mod alloc;
mod cast;
#[cfg(not(feature = "reference-cheney"))]
mod compaction;
mod descriptors;
mod exit_codes;
#[cfg_attr(feature = "reference-cheney", path = "../reference/cheney.rs")]
mod gc;
mod heap_layout;
mod init;
mod io;
mod object;
mod shadow_stack;
mod string_ops;

// --- Public C-ABI surface, re-exported at the crate root --------------------
// Each item below is also `#[no_mangle]` in its module, so the C symbol exists
// regardless of these re-exports; the re-exports are for Rust consumers (the
// language-native tests and the io probe bin).

pub use object::{ClassDescriptor, Object, ShadowFrame, StringObject, VTableEntry};
// Layout helpers for teams implementing the GC / string ops (not part of the
// C-ABI symbol set, but useful Rust-side accessors for the flexible tails).
pub use object::{shadow_frame_roots_offset, string_data_offset};

pub use descriptors::{LO_BOOL_BOX_CLASS, LO_EMPTY_STRING, LO_INT_BOX_CLASS, LO_STRING_CLASS};

pub use alloc::lo_alloc;
pub use init::{lo_runtime_init, lo_runtime_shutdown};
pub use shadow_stack::{lo_pop_frame, lo_push_frame};

pub use io::{
    lo_eof, lo_print_bool, lo_print_int, lo_print_string, lo_println, lo_read_bool, lo_read_int,
    lo_read_string,
};

pub use abort::lo_abort_null_receiver;
pub use cast::{lo_cast_check, lo_instanceof};
pub use gc::{lo_gc_collect, lo_gc_write_barrier};
pub use string_ops::{
    lo_string_compare, lo_string_concat, lo_string_new, lo_string_repeat, lo_string_reverse,
};

// Debug/test hooks (not part of the ABI).
#[doc(hidden)]
pub use alloc::heap_used;
#[doc(hidden)]
pub use shadow_stack::current_frame;
