//! Assembly-level names for everything codegen defines or references.
//!
//! Lowering emits these names (`call @lo_method_6_Circle_4_area`) and the static
//! data builder defines them (a vtable entry pointing at that same method), so
//! both must spell each name identically. Every name is built here.
//!
//! Class and member names are length-prefixed, so no two LO entities map to the
//! same symbol, and user code cannot collide with them because the type checker
//! rejects identifiers starting with `lo_`. The first three functions match
//! `wasm/mod.rs`, so both back ends use the same names.

/// A class's descriptor, e.g. `lo_class_6_Circle`.
pub fn class_descriptor(class: &str) -> String {
    format!("lo_class_{}_{}", class.len(), class)
}

/// A method's code, e.g. `lo_method_6_Circle_4_area`.
pub fn method(class: &str, method: &str) -> String {
    format!(
        "lo_method_{}_{}_{}_{}",
        class.len(),
        class,
        method.len(),
        method
    )
}

/// The constructor of `class` taking `arity` arguments, e.g. `lo_ctor_6_Circle_2`.
pub fn constructor(class: &str, arity: usize) -> String {
    format!("lo_ctor_{}_{}_{}", class.len(), class, arity)
}

/// A class's vtable: an array of method addresses.
pub fn vtable(class: &str) -> String {
    format!("{}_vtable", class_descriptor(class))
}

/// A class's array of pointer-field offsets, read by the collector.
pub fn pointer_offsets(class: &str) -> String {
    format!("{}_pointers", class_descriptor(class))
}

/// A class's NUL-terminated name, read by `lo_cast_check` error messages.
pub fn class_name(class: &str) -> String {
    format!("{}_name", class_descriptor(class))
}

/// The bytes of a method's name, passed to `lo_abort_null_receiver`.
pub fn method_name(method: &str) -> String {
    format!("lo_mname_{}_{}", method.len(), method)
}

/// The word holding the address of the root slot for `in`, `out` or `err`.
pub fn binding(name: &str) -> String {
    format!("lo_binding_{name}")
}

/// The pre-bound names, in the order their root slots are assigned.
pub const PREBOUND: [&str; 3] = ["in", "out", "err"];

/// Runtime exports codegen references by name (`runtime-abi.md` §2.3).
pub const EMPTY_STRING: &str = "LO_EMPTY_STRING";
pub const STRING_CLASS: &str = "LO_STRING_CLASS";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_the_wasm_back_end() {
        assert_eq!(class_descriptor("Circle"), "lo_class_6_Circle");
        assert_eq!(method("Circle", "area"), "lo_method_6_Circle_4_area");
        assert_eq!(constructor("Circle", 2), "lo_ctor_6_Circle_2");
        assert_eq!(vtable("Circle"), "lo_class_6_Circle_vtable");
        assert_eq!(binding("out"), "lo_binding_out");
    }

    #[test]
    fn length_prefixes_keep_names_apart() {
        // Without the lengths, class "a_b" + method "c" and class "a" + method "b_c"
        // would both be "lo_method_a_b_c".
        assert_ne!(method("a_b", "c"), method("a", "b_c"));
    }
}
