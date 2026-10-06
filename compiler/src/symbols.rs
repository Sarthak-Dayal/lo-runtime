pub const PREBOUND: [&str; 3] = ["in", "out", "err"];
pub const EMPTY_STRING: &str = "LO_EMPTY_STRING";
pub const STRING_CLASS: &str = "LO_STRING_CLASS";

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

pub fn method_name(method: &str) -> String {
    format!("lo_mname_{}_{}", method.len(), method)
}

pub fn binding(name: &str) -> String {
    format!("lo_binding_{name}")
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
