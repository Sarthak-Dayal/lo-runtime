//! Class signatures and effective members.

use std::collections::{HashMap, HashSet};

use super::{ClassKind, ErrorCode, TypeError};
use crate::ast::{IoOp, Type};

pub struct ClassTable {
    pub(super) classes: HashMap<String, ClassInfo>,
    pub(super) order: Vec<String>,
}

pub struct ClassInfo {
    pub decl_line: u32,
    pub kind: ClassKind,
    pub parent: Option<String>,
    pub(super) own_fields: Vec<(String, Type, u32)>,
    pub(super) own_methods: Vec<MethodSig>,
    pub(super) own_constructors: Vec<ConstructorSig>,

    pub ancestors: Vec<String>,
    pub effective_fields: Vec<FieldInfo>,
    pub effective_methods: HashMap<String, MethodEntry>,

    /// Method names in vtable-slot order: an inherited method keeps its
    /// parent's slot, an override reuses that slot, and a genuinely new
    /// method is appended. `method_slot[name]` is that name's index here.
    pub vtable: Vec<String>,
    pub method_slot: HashMap<String, usize>,
}

/// An effective (possibly inherited) field: where it's declared and its type.
#[derive(Clone)]
pub struct FieldInfo {
    pub name: String,
    pub ty: Type,
    pub owner: String,
}

/// An effective (possibly inherited) method: which class's signature won out
/// and what that signature is. `owner` is the declaring class, not
/// necessarily the class this `ClassInfo` belongs to.
#[derive(Clone)]
pub struct MethodEntry {
    pub owner: String,
    pub sig: MethodSig,
}

#[derive(Clone, PartialEq)]
pub struct MethodSig {
    pub method_name: String,
    pub params: Vec<Type>,
    pub return_type: Type,
    pub line: u32,
    /// `Some(op)` iff this signature's body is a built-in IO operation rather
    /// than user-defined code.
    pub is_io: Option<IoOp>,
}

#[derive(Clone)]
pub struct ConstructorSig {
    pub arity: usize,
    pub params: Vec<Type>,
    /// `Some(n)` iff this constructor's own delegation is `this(...)` targeting
    /// the n-arity constructor of the same class.
    pub this_target_arity: Option<usize>,
    pub line: u32,
}

impl ClassTable {
    pub fn get(&self, name: &str) -> Option<&ClassInfo> {
        self.classes.get(name)
    }

    pub fn class_exists(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    pub fn is_subtype(&self, a: &str, b: &str) -> bool {
        a == b
            || self
                .classes
                .get(a)
                .is_some_and(|info| info.ancestors.iter().any(|anc| anc == b))
    }
}

impl ClassInfo {
    /// This class's own constructors (LO has no constructor inheritance --
    /// every instantiable class declares, or is given, its own). Exposed so
    /// downstream consumers (the interpreter, wasm codegen) can dispatch
    /// `new` without re-deriving arity/param information the type checker
    /// already computed.
    pub fn constructors(&self) -> &[ConstructorSig] {
        &self.own_constructors
    }

    pub fn find_constructor(&self, arity: usize) -> Option<&ConstructorSig> {
        self.own_constructors.iter().find(|c| c.arity == arity)
    }
}

pub(super) fn check_type_reference(
    ty: &Type,
    table: &ClassTable,
    line: u32,
    ctx: &str,
) -> Result<(), TypeError> {
    if let Type::Class(name) = ty {
        if !table.class_exists(name) {
            return Err(TypeError::new(
                ErrorCode::EUnknownClass,
                line,
                format!("unknown class '{}' referenced in '{}'", name, ctx),
            ));
        }
    }
    Ok(())
}

/// Shared cycle detection for inheritance and constructor delegation.
pub(super) fn find_cycle<'a, N, F>(nodes: impl Iterator<Item = &'a N>, edges: F) -> Option<(N, N)>
where
    N: Eq + std::hash::Hash + Clone + 'a,
    F: Fn(&N) -> Vec<N>,
{
    let mut visited: HashSet<N> = HashSet::new();
    let mut on_path: Vec<N> = Vec::new();

    fn visit<N, F>(
        node: &N,
        edges: &F,
        visited: &mut HashSet<N>,
        on_path: &mut Vec<N>,
    ) -> Option<(N, N)>
    where
        N: Eq + std::hash::Hash + Clone,
        F: Fn(&N) -> Vec<N>,
    {
        if on_path.contains(node) {
            return Some((on_path.last().unwrap().clone(), node.clone()));
        }
        if !visited.insert(node.clone()) {
            return None;
        }
        on_path.push(node.clone());
        for next in edges(node) {
            if let Some(cycle) = visit(&next, edges, visited, on_path) {
                return Some(cycle);
            }
        }
        on_path.pop();
        None
    }

    for node in nodes {
        if !visited.contains(node) {
            if let Some(cycle) = visit(node, &edges, &mut visited, &mut on_path) {
                return Some(cycle);
            }
        }
    }
    None
}
