//! Assembly symbol names for generated code and data. Every LO name is
//! length-prefixed (`lo_class_6_Circle`), so two different LO names, or an LO
//! name and a runtime entry point such as `lo_alloc`, never share a symbol.

pub const PREBOUND: [&str; 3] = ["in", "out", "err"];
pub const EMPTY_STRING: &str = "LO_EMPTY_STRING";
/// Static shadow frame whose roots hold the prebound `in`, `out` and `err`.
pub const BINDINGS: &str = "lo_bindings";

pub fn class_descriptor(class: &str) -> String {
    format!("lo_class_{}_{}", class.len(), class)
}

pub fn method(class: &str, method: &str) -> String {
    format!(
        "lo_method_{}_{}_{}_{}",
        class.len(),
        class,
        method.len(),
        method
    )
}

pub fn constructor(class: &str, arity: usize) -> String {
    format!("lo_ctor_{}_{}_{}", class.len(), class, arity)
}

pub fn vtable(class: &str) -> String {
    format!("{}_vtable", class_descriptor(class))
}

pub fn pointer_offsets(class: &str) -> String {
    format!("{}_pointers", class_descriptor(class))
}

pub fn class_name(class: &str) -> String {
    format!("{}_name", class_descriptor(class))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_names_do_not_collide() {
        assert_ne!(vtable("A"), class_descriptor("A_vtable"));
        assert_ne!(method("a_b", "c"), method("a", "b_c"));
    }
}
