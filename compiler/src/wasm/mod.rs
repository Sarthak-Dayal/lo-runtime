//! LO-3/LO-4 typed AST -> LLVM WebAssembly assembly.
//! Production numbers follow LO Appendix A.5. Encoding belongs to llvm-mc.

mod expressions;
mod function;
mod module;
mod statements;

pub use module::p1_program;

use crate::ast::Type;

// P35-P39: every value is i32; only String and class values are GC roots.
fn is_reference(ty: &Type) -> bool {
    matches!(ty, Type::String | Type::Class(_))
}

// P44/P45/P50/P53: length prefixes keep user identifiers from colliding.
fn class_symbol(class: &str) -> String {
    format!("lo_class_{}_{}", class.len(), class)
}

fn method_symbol(class: &str, method: &str) -> String {
    format!(
        "lo_method_{}_{}_{}_{}",
        class.len(),
        class,
        method.len(),
        method
    )
}

fn ctor_symbol(class: &str, arity: usize) -> String {
    format!("lo_ctor_{}_{}_{}", class.len(), class, arity)
}

fn signature(params: usize, returns: bool) -> String {
    format!(
        "({}) -> ({})",
        vec!["i32"; params].join(", "),
        if returns { "i32" } else { "" }
    )
}
