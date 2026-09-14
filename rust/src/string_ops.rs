//! String operations (`runtime-abi.md` §3.2). All **stubbed** — the team
//! implements these in P3. Each carries the exact C-ABI signature so the
//! skeleton links; calling one panics with a recognizable message.
//!
//! Implementation hints for the team live in the ABI: `lo_string_repeat` aborts
//! on negative count (exit 120); `lo_string_compare` is lexicographic UTF-8 byte
//! ordering; `lo_string_reverse` reverses codepoints, not bytes. The internal
//! variable-size allocator to build results with is `alloc::bump_alloc_string`.

use crate::object::Object;

/// Construct a string from `len` raw UTF-8 bytes (copied; caller owns the
/// source).
///
/// # Safety
/// `bytes` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn lo_string_new(bytes: *const u8, len: u32) -> *mut Object {
    let _ = (bytes, len);
    unimplemented!("lo_string_new: team implements per P3");
}

/// Return a new string `a + b`.
///
/// # Safety
/// `a` and `b` must point at valid `StringObject`s.
#[no_mangle]
pub unsafe extern "C" fn lo_string_concat(a: *mut Object, b: *mut Object) -> *mut Object {
    let _ = (a, b);
    unimplemented!("lo_string_concat: team implements per P3");
}

/// Return a new string: `s` repeated `n` times. Aborts (exit 120) on negative
/// `n`.
///
/// # Safety
/// `s` must point at a valid `StringObject`.
#[no_mangle]
pub unsafe extern "C" fn lo_string_repeat(s: *mut Object, n: i32) -> *mut Object {
    let _ = (s, n);
    unimplemented!("lo_string_repeat: team implements per P3");
}

/// Compare two strings, returning negative / zero / positive by lexicographic
/// UTF-8 byte ordering.
///
/// # Safety
/// `a` and `b` must point at valid `StringObject`s.
#[no_mangle]
pub unsafe extern "C" fn lo_string_compare(a: *mut Object, b: *mut Object) -> i32 {
    let _ = (a, b);
    unimplemented!("lo_string_compare: team implements per P3");
}

/// Return a new string with codepoints reversed.
///
/// # Safety
/// `s` must point at a valid `StringObject`.
#[no_mangle]
pub unsafe extern "C" fn lo_string_reverse(s: *mut Object) -> *mut Object {
    let _ = s;
    unimplemented!("lo_string_reverse: team implements per P3");
}
