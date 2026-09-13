use std::collections::HashMap;

use crate::ast::Type;

use super::heap::{ObjId, StrId};
use super::value::{type_default, Value};

pub struct Frame {
    /// The receiver, if this is a method/constructor frame. `None` for a `static`
    /// context (there is none in LO-4, but the entry `Main` is built the same way).
    pub this: Option<ObjId>,
    slots: Vec<Value>,
    index: HashMap<String, usize>,
}

impl Frame {
    /// Build a frame: `params` are bound positionally to `args`, then `locals` are
    /// appended at their type-defaults. Names are unique across the two (the
    /// checker forbids a local shadowing a formal).
    pub fn new(
        this: Option<ObjId>,
        params: &[(String, Type)],
        args: Vec<Value>,
        locals: &[(String, Type)],
        empty: StrId,
    ) -> Frame {
        debug_assert_eq!(
            params.len(),
            args.len(),
            "argument count must match the parameter list"
        );
        let mut slots = args;
        let mut index = HashMap::with_capacity(params.len() + locals.len());
        for (i, (name, _)) in params.iter().enumerate() {
            index.insert(name.clone(), i);
        }
        for (name, ty) in locals {
            index.insert(name.clone(), slots.len());
            slots.push(type_default(ty, empty));
        }
        Frame { this, slots, index }
    }

    /// Read a local or formal by name.
    pub fn get(&self, name: &str) -> Value {
        self.slots[self.index[name]]
    }

    /// Write a local or formal by name.
    pub fn set(&mut self, name: &str, value: Value) {
        let i = self.index[name];
        self.slots[i] = value;
    }
}
