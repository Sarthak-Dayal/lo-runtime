//! Type operations (`runtime-abi.md` §3.5).

use crate::abort::runtime_abort;
use crate::object::{ClassDescriptor, Object};

/// Walks `class` up its single-inheritance chain, returning true the moment
/// `target` is reached (including immediately, i.e. `class == target`).
///
/// # Safety
/// `class`, if non-null, and `target` must point at valid `ClassDescriptor`s
/// forming a well-formed (acyclic, null-terminated) parent chain.
unsafe fn class_is_or_descends(
    mut class: *const ClassDescriptor,
    target: *const ClassDescriptor,
) -> bool {
    while !class.is_null() {
        if class == target {
            return true;
        }
        class = (*class).parent;
    }
    false
}

/// Reads a `ClassDescriptor`'s name. Descriptors are immutable `static` data
/// generated once, so `'static` is accurate here (unlike heap objects, which a
/// moving GC can relocate).
///
/// # Safety
/// `class` must point at a valid `ClassDescriptor`.
unsafe fn descriptor_name(class: *const ClassDescriptor) -> &'static str {
    let bytes = core::slice::from_raw_parts((*class).name, (*class).name_len as usize);
    core::str::from_utf8_unchecked(bytes)
}

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
    if obj.is_null() {
        return obj;
    }
    let actual = (*obj).class_descriptor;
    if class_is_or_descends(actual, target) {
        return obj;
    }
    runtime_abort(
        &format!(
            "lo_cast_check: cannot cast {} to {}",
            descriptor_name(actual),
            descriptor_name(target),
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
    class_is_or_descends((*obj).class_descriptor, target)
}
