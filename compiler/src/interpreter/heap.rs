//! Every LO object and string lives in one flat `Vec<Cell>`, addressed by an
//! integer handle (`ObjId`/`StrId`) rather than a pointer. Handles keep cyclic
//! data from leaking the accounting and let a future mark-sweep work without a
//! redesign. A byte budget over the arena reproduces the ABI's out-of-memory
//! abort (exit 137) deterministically; the interpreter does not otherwise collect.

use std::rc::Rc;

use super::abort::AbortKind;
use super::value::Value;

/// A handle to an object cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjId(pub u32);

/// A handle to a string cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StrId(pub u32);

/// Object header bytes on the wasm32 ABI (`class_descriptor` + `gc_bits` + `flags`).
const HEADER_BYTES: usize = 12;
/// One pointer-sized field slot on wasm32.
const FIELD_BYTES: usize = 4;
/// A string's `length: u32`, ahead of its inline UTF-8 tail.
const STR_LEN_BYTES: usize = 4;

/// Default arena budget: 64 MiB. A starting guess (design §11) — set it after
/// measuring the largest conformance program's live allocation.
pub const DEFAULT_HEAP_LIMIT: usize = 64 * 1024 * 1024;

enum Cell {
    Obj { class: Rc<str>, fields: Vec<Value> },
    Str(Rc<str>),
}

pub struct Heap {
    cells: Vec<Cell>,
    charged: usize,
    limit: usize,
    empty: StrId,
}

impl Heap {
    /// A fresh heap with the given byte budget, holding only the canonical empty
    /// string.
    pub fn new(limit: usize) -> Self {
        let mut heap = Heap {
            cells: Vec::new(),
            charged: 0,
            limit,
            empty: StrId(0),
        };
        // The empty string mirrors the ABI's LO_EMPTY_STRING: a fixed static that
        // lives outside the collectable heap, so it is interned once and never
        // charged against the budget.
        let id = StrId(heap.cells.len() as u32);
        heap.cells.push(Cell::Str(Rc::from("")));
        heap.empty = id;
        heap
    }

    /// The canonical empty-string handle (the `String` type-default).
    pub fn empty(&self) -> StrId {
        self.empty
    }

    /// Bytes charged against the budget so far.
    pub fn charged(&self) -> usize {
        self.charged
    }

    /// Allocate an object of `class` whose `fields` are already laid out
    /// (parent-first, at their type-defaults). Charges before pushing, so a
    /// failed allocation leaves the heap unchanged.
    pub fn alloc_obj(&mut self, class: Rc<str>, fields: Vec<Value>) -> Result<ObjId, AbortKind> {
        let cost = HEADER_BYTES + FIELD_BYTES * fields.len();
        self.charge(cost)?;
        let id = ObjId(self.cells.len() as u32);
        self.cells.push(Cell::Obj { class, fields });
        Ok(id)
    }

    /// Intern `s` as a string cell. The empty string reuses the uncharged
    /// canonical singleton; any other string is a fresh, charged allocation.
    pub fn alloc_str(&mut self, s: &str) -> Result<StrId, AbortKind> {
        if s.is_empty() {
            return Ok(self.empty);
        }
        let cost = HEADER_BYTES + STR_LEN_BYTES + s.len();
        self.charge(cost)?;
        let id = StrId(self.cells.len() as u32);
        self.cells.push(Cell::Str(Rc::from(s)));
        Ok(id)
    }

    /// Intern a string as a fixed *static*, the way a compiled string literal
    /// lives in `.rodata` rather than the collectable heap: pushed once, and
    /// **never charged** against the budget. The empty string reuses the
    /// canonical singleton.
    pub fn intern_static(&mut self, s: &str) -> StrId {
        if s.is_empty() {
            return self.empty;
        }
        let id = StrId(self.cells.len() as u32);
        self.cells.push(Cell::Str(Rc::from(s)));
        id
    }

    /// Allocate a fixed infrastructure object — the `in`/`out`/`err` singletons —
    /// with no fields and **without charging** the budget. Like the empty-string
    /// static, these are persistent runtime state, not program allocations.
    pub fn alloc_singleton(&mut self, class: Rc<str>) -> ObjId {
        let id = ObjId(self.cells.len() as u32);
        self.cells.push(Cell::Obj {
            class,
            fields: Vec::new(),
        });
        id
    }

    /// The runtime class name of an object.
    pub fn class_of(&self, id: ObjId) -> &str {
        match &self.cells[id.0 as usize] {
            Cell::Obj { class, .. } => class,
            Cell::Str(_) => unreachable!("interpreter invariant: class_of on a string cell"),
        }
    }

    /// The value in an object's field slot (index into its parent-first layout).
    pub fn field(&self, id: ObjId, index: usize) -> Value {
        match &self.cells[id.0 as usize] {
            Cell::Obj { fields, .. } => fields[index],
            Cell::Str(_) => unreachable!("interpreter invariant: field on a string cell"),
        }
    }

    /// Store into an object's field slot.
    pub fn set_field(&mut self, id: ObjId, index: usize, value: Value) {
        match &mut self.cells[id.0 as usize] {
            Cell::Obj { fields, .. } => fields[index] = value,
            Cell::Str(_) => unreachable!("interpreter invariant: set_field on a string cell"),
        }
    }

    /// The bytes of a string cell (cheap clone of the shared buffer).
    pub fn str_value(&self, id: StrId) -> Rc<str> {
        match &self.cells[id.0 as usize] {
            Cell::Str(s) => Rc::clone(s),
            Cell::Obj { .. } => unreachable!("interpreter invariant: str_value on an object cell"),
        }
    }

    fn charge(&mut self, cost: usize) -> Result<(), AbortKind> {
        // Saturating so a pathological cost cannot wrap the counter below `limit`.
        let next = self.charged.saturating_add(cost);
        if next > self.limit {
            return Err(AbortKind::OutOfMemory);
        }
        self.charged = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_interned_and_uncharged() {
        let heap = Heap::new(DEFAULT_HEAP_LIMIT);
        assert_eq!(heap.charged(), 0);
        assert_eq!(&*heap.str_value(heap.empty()), "");
    }

    #[test]
    fn object_charge_is_header_plus_one_word_per_field() {
        let mut heap = Heap::new(DEFAULT_HEAP_LIMIT);
        let class: Rc<str> = Rc::from("C");
        heap.alloc_obj(Rc::clone(&class), vec![Value::Int(0), Value::Int(0)])
            .unwrap();
        assert_eq!(heap.charged(), HEADER_BYTES + 2 * FIELD_BYTES); // 20
    }

    #[test]
    fn string_charge_is_header_len_field_plus_bytes() {
        let mut heap = Heap::new(DEFAULT_HEAP_LIMIT);
        heap.alloc_str("hello").unwrap();
        assert_eq!(heap.charged(), HEADER_BYTES + STR_LEN_BYTES + 5); // 21
    }

    #[test]
    fn empty_result_reuses_the_singleton_and_charges_nothing() {
        let mut heap = Heap::new(DEFAULT_HEAP_LIMIT);
        let id = heap.alloc_str("").unwrap();
        assert_eq!(id, heap.empty());
        assert_eq!(heap.charged(), 0);
    }

    #[test]
    fn exceeding_the_budget_aborts_and_leaves_the_heap_unchanged() {
        // Budget fits exactly one 1-field object (16 bytes), not two.
        let mut heap = Heap::new(20);
        let class: Rc<str> = Rc::from("C");
        heap.alloc_obj(Rc::clone(&class), vec![Value::Int(0)])
            .unwrap();
        assert_eq!(heap.charged(), 16);

        let err = heap
            .alloc_obj(Rc::clone(&class), vec![Value::Int(0)])
            .unwrap_err();
        assert_eq!(err, AbortKind::OutOfMemory);
        // The failed allocation charged nothing and did not push a cell.
        assert_eq!(heap.charged(), 16);

        // A subsequent allocation that fits still succeeds (index space intact).
        heap.alloc_str("").unwrap();
    }

    #[test]
    fn fields_read_back_what_was_stored() {
        let mut heap = Heap::new(DEFAULT_HEAP_LIMIT);
        let class: Rc<str> = Rc::from("Pair");
        let id = heap
            .alloc_obj(class, vec![Value::Int(1), Value::Bool(false)])
            .unwrap();
        assert_eq!(heap.class_of(id), "Pair");
        heap.set_field(id, 1, Value::Bool(true));
        assert_eq!(heap.field(id, 0), Value::Int(1));
        assert_eq!(heap.field(id, 1), Value::Bool(true));
    }
}
