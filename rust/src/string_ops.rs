//! String operations (`runtime-abi.md` §3.2). All **stubbed** — the team
//! implements these in P3. Each carries the exact C-ABI signature so the
//! skeleton links; calling one panics with a recognizable message.
//!
//! Implementation hints for the team live in the ABI: `lo_string_repeat` aborts
//! on negative count (exit 120); `lo_string_compare` is lexicographic UTF-8 byte
//! ordering; `lo_string_reverse` reverses codepoints, not bytes. The internal
//! variable-size allocator to build results with is `alloc::bump_alloc_string`.

use crate::object::{string_data_offset, Object, StringObject};

unsafe fn bytes(object: *mut Object) -> &'static [u8] {
    let string = object as *const StringObject;
    let length = (*string).length as usize;
    core::slice::from_raw_parts((object as *const u8).add(string_data_offset()), length)
}

unsafe fn allocate_bytes(contents: &[u8]) -> *mut Object {
    let length = u32::try_from(contents.len())
        .unwrap_or_else(|_| crate::abort::runtime_abort("lo_alloc: out of memory", 137));
    let object = crate::alloc::bump_alloc_string(length);
    if !contents.is_empty() {
        core::ptr::copy_nonoverlapping(
            contents.as_ptr(),
            (object as *mut u8).add(string_data_offset()),
            contents.len(),
        );
    }
    object
}

/// Construct a string from `len` raw UTF-8 bytes (copied; caller owns the
/// source).
///
/// # Safety
/// `bytes` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn lo_string_new(bytes: *const u8, len: u32) -> *mut Object {
    // Copy before allocating because the managed allocation may trigger GC.
    let contents = core::slice::from_raw_parts(bytes, len as usize).to_vec();
    allocate_bytes(&contents)
}

/// Return a new string `a + b`.
///
/// # Safety
/// `a` and `b` must point at valid `StringObject`s.
#[no_mangle]
pub unsafe extern "C" fn lo_string_concat(a: *mut Object, b: *mut Object) -> *mut Object {
    // Read both inputs before allocating; GC may relocate either object.
    let a = bytes(a);
    let b = bytes(b);
    let length = a
        .len()
        .checked_add(b.len())
        .unwrap_or_else(|| crate::abort::runtime_abort("lo_alloc: out of memory", 137));
    let mut contents = Vec::with_capacity(length);
    contents.extend_from_slice(a);
    contents.extend_from_slice(b);
    allocate_bytes(&contents)
}

/// Return a new string: `s` repeated `n` times. Aborts (exit 120) on negative
/// `n`.
///
/// # Safety
/// `s` must point at a valid `StringObject`.
#[no_mangle]
pub unsafe extern "C" fn lo_string_repeat(s: *mut Object, n: i32) -> *mut Object {
    if n < 0 {
        crate::abort::runtime_abort(&format!("lo_string_repeat: negative count {n}"), 120);
    }
    let input = bytes(s);
    let length = input
        .len()
        .checked_mul(n as usize)
        .unwrap_or_else(|| crate::abort::runtime_abort("lo_alloc: out of memory", 137));
    let mut contents = Vec::with_capacity(length);
    for _ in 0..n {
        contents.extend_from_slice(input);
    }
    allocate_bytes(&contents)
}

/// Compare two strings, returning negative / zero / positive by lexicographic
/// UTF-8 byte ordering.
///
/// # Safety
/// `a` and `b` must point at valid `StringObject`s.
#[no_mangle]
pub unsafe extern "C" fn lo_string_compare(a: *mut Object, b: *mut Object) -> i32 {
    use core::cmp::Ordering;
    match bytes(a).cmp(bytes(b)) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// Return a new string with codepoints reversed.
///
/// # Safety
/// `s` must point at a valid `StringObject`.
#[no_mangle]
pub unsafe extern "C" fn lo_string_reverse(s: *mut Object) -> *mut Object {
    let input = core::str::from_utf8_unchecked(bytes(s));
    let contents: String = input.chars().rev().collect();
    allocate_bytes(contents.as_bytes())
}
