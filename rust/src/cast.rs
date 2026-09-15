//! Type operations (`runtime-abi.md` §3.5). Both **stubbed** — the team
//! implements these in P3.
//!
//! Implementation hints from the ABI: `lo_cast_check` returns `obj` if its class
//! is `target` or a descendant, else aborts (exit 101) after writing
//! `lo_cast_check: cannot cast <from> to <to>`; a null `obj` short-circuits to
//! null. `lo_instanceof` returns a bool and never aborts; a null receiver yields
//! `false`. Both walk `ClassDescriptor.parent` up the single-inheritance chain.

use crate::object::{ClassDescriptor, Object};

/// Checked downcast: return `obj` if its class is `target` or a descendant; abort
/// (exit 101) otherwise. Null `obj` returns null.
///
/// # Safety
/// `obj`, if non-null, must point at a valid object; `target` must point at a
/// valid `ClassDescriptor`.
#[no_mangle]
pub unsafe extern "C" fn lo_cast_check(
    obj: *mut Object,
    target: *const ClassDescriptor,
) -> *mut Object {
    if obj.is_null() || lo_instanceof(obj, target) {
        return obj;
    }
    let source = (*obj).class_descriptor;
    crate::abort::runtime_abort(
        &format!(
            "lo_cast_check: cannot cast {} to {}",
            class_name(source),
            class_name(target)
        ),
        101,
    )
}

/// Return true iff `obj`'s class is `target` or a descendant. Null `obj` yields
/// false; never aborts.
///
/// # Safety
/// `obj`, if non-null, must point at a valid object; `target` must point at a
/// valid `ClassDescriptor`.
#[no_mangle]
pub unsafe extern "C" fn lo_instanceof(obj: *mut Object, target: *const ClassDescriptor) -> bool {
    if obj.is_null() {
        return false;
    }
    let mut class = (*obj).class_descriptor;
    while !class.is_null() {
        if class == target {
            return true;
        }
        class = (*class).parent;
    }
    false
}

unsafe fn class_name(class: *const ClassDescriptor) -> &'static str {
    let bytes = core::slice::from_raw_parts((*class).name, (*class).name_len as usize);
    core::str::from_utf8(bytes).unwrap_or("<invalid-utf8>")
}
