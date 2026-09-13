//! Runtime values (design §2).
//!
//! A `Value` is self-describing: it carries its own kind, so operator dispatch
//! reads the value rather than any static type annotation. Strings and objects
//! are handles into the heap arena; `Obj(None)` is the one and only LO null.

use crate::ast::Type;

use super::heap::{ObjId, StrId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    Int(i32),
    Bool(bool),
    /// A string handle — never null (`String` is primitive-equivalent; its
    /// type-default is the empty string, and `null` is only class-compatible).
    Str(StrId),
    /// A class-typed value; `None` is LO null.
    Obj(Option<ObjId>),
}

/// The LO type-default for `ty`, applied to fields at allocation and to locals at
/// frame setup.
///
/// The subtle case is `String`: it defaults to the *empty string* (`empty`), not
/// zero/null — mirroring the ABI's `LO_EMPTY_STRING`. Only class types default to
/// null. `Void` is not a value type and has no default.
pub fn type_default(ty: &Type, empty: StrId) -> Value {
    match ty {
        Type::Int => Value::Int(0),
        Type::Bool => Value::Bool(false),
        Type::String => Value::Str(empty),
        Type::Class(_) => Value::Obj(None),
        Type::Void => unreachable!("interpreter invariant: void has no runtime value"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::heap::Heap;

    #[test]
    fn scalar_defaults() {
        let heap = Heap::new(super::super::heap::DEFAULT_HEAP_LIMIT);
        assert_eq!(type_default(&Type::Int, heap.empty()), Value::Int(0));
        assert_eq!(type_default(&Type::Bool, heap.empty()), Value::Bool(false));
    }

    #[test]
    fn string_defaults_to_empty_not_null() {
        let heap = Heap::new(super::super::heap::DEFAULT_HEAP_LIMIT);
        assert_eq!(
            type_default(&Type::String, heap.empty()),
            Value::Str(heap.empty())
        );
    }

    #[test]
    fn class_defaults_to_null() {
        let heap = Heap::new(super::super::heap::DEFAULT_HEAP_LIMIT);
        assert_eq!(
            type_default(&Type::Class("Foo".into()), heap.empty()),
            Value::Obj(None)
        );
    }
}
