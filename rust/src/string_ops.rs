//! String operations (`runtime-abi.md` §3.2).
//!
//! `concat`/`repeat`/`reverse` take existing heap `StringObject`s as input, and
//! `bump_alloc_string` can trigger a moving collection while building the
//! result. So each copies its input bytes into an owned buffer first (via
//! `copy_bytes`) and never holds a raw pointer into the heap across an
//! allocation; `compare` never allocates, so it borrows directly instead.

use crate::abort::runtime_abort;
use crate::alloc::bump_alloc_string;
use crate::object::{string_data_offset, Object, StringObject};

/// Borrows `s`'s inline UTF-8 bytes. Only valid until the next allocation.
///
/// # Safety
/// `s` must point at a valid `StringObject`.
unsafe fn borrow_bytes<'a>(s: *const Object) -> &'a [u8] {
    let so = s as *const StringObject;
    let len = (*so).length as usize;
    let data = (s as *const u8).add(string_data_offset());
    core::slice::from_raw_parts(data, len)
}

/// Copies `s`'s inline UTF-8 bytes into an owned buffer, safe to hold across a
/// later allocation.
///
/// # Safety
/// `s` must point at a valid `StringObject`.
unsafe fn copy_bytes(s: *const Object) -> Vec<u8> {
    borrow_bytes(s).to_vec()
}

/// Allocates a `StringObject` and fills it with `bytes`.
///
/// # Safety
/// Must be called after `heap_init`.
unsafe fn alloc_from_bytes(bytes: &[u8]) -> *mut Object {
    let obj = bump_alloc_string(bytes.len() as u32);
    if !bytes.is_empty() {
        let data = (obj as *mut u8).add(string_data_offset());
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
    }
    obj
}

/// Construct a string from `len` raw UTF-8 bytes (copied; caller owns the
/// source). `bytes` is not a heap pointer (typically module data), so no
/// GC hazard applies here.
///
/// # Safety
/// `bytes` must point at `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn lo_string_new(bytes: *const u8, len: u32) -> *mut Object {
    alloc_from_bytes(core::slice::from_raw_parts(bytes, len as usize))
}

/// Return a new string `a + b`.
///
/// # Safety
/// `a` and `b` must point at valid `StringObject`s.
#[no_mangle]
pub unsafe extern "C" fn lo_string_concat(a: *mut Object, b: *mut Object) -> *mut Object {
    let mut combined = copy_bytes(a);
    combined.extend(copy_bytes(b));
    alloc_from_bytes(&combined)
}

/// Return a new string: `s` repeated `n` times. Aborts (exit 120) on negative
/// `n`.
///
/// # Safety
/// `s` must point at a valid `StringObject`.
#[no_mangle]
pub unsafe extern "C" fn lo_string_repeat(s: *mut Object, n: i32) -> *mut Object {
    if n < 0 {
        runtime_abort(&format!("lo_string_repeat: negative count {n}"), 120);
    }
    let source = copy_bytes(s);
    let mut combined = Vec::with_capacity(source.len() * n as usize);
    for _ in 0..n {
        combined.extend_from_slice(&source);
    }
    alloc_from_bytes(&combined)
}

/// Compare two strings, returning negative / zero / positive by lexicographic
/// UTF-8 byte ordering. Never allocates, so it borrows both inputs directly.
///
/// # Safety
/// `a` and `b` must point at valid `StringObject`s.
#[no_mangle]
pub unsafe extern "C" fn lo_string_compare(a: *mut Object, b: *mut Object) -> i32 {
    match borrow_bytes(a).cmp(borrow_bytes(b)) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

/// Return a new string with codepoints reversed. LO strings are valid UTF-8 by
/// construction, so reversing by `char` reverses codepoints, not bytes.
///
/// # Safety
/// `s` must point at a valid `StringObject`.
#[no_mangle]
pub unsafe extern "C" fn lo_string_reverse(s: *mut Object) -> *mut Object {
    let text = String::from_utf8_unchecked(copy_bytes(s));
    let reversed: String = text.chars().rev().collect();
    alloc_from_bytes(reversed.as_bytes())
}
