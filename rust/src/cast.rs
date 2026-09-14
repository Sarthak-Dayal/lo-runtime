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
    let _ = (obj, target);
    unimplemented!("lo_cast_check: team implements per P3");
}

/// Return true iff `obj`'s class is `target` or a descendant. Null `obj` yields
/// false; never aborts.
///
/// # Safety
/// `obj`, if non-null, must point at a valid object; `target` must point at a
/// valid `ClassDescriptor`.
#[no_mangle]
pub unsafe extern "C" fn lo_instanceof(obj: *mut Object, target: *const ClassDescriptor) -> bool {
    let _ = (obj, target);
    unimplemented!("lo_instanceof: team implements per P3");
}
