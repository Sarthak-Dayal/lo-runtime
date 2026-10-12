//! Lowering driver: declares every symbol and static datum, then lowers each
//! constructor, method and lo_entry.

use std::collections::HashMap;

use crate::layout::{class_layout, ClassLayout, Target};
use crate::symbols;
use crate::type_checker::{ClassTable, TypedProgram};

use super::{lower_constructor, lower_method, lower_startup};
use crate::ir::*;

pub(super) struct Context<'a> {
    pub ir: ProgramIr,
    pub classes: &'a ClassTable,
    pub target: Target,
    layouts: HashMap<String, ClassLayout>,
    bytes: HashMap<Vec<u8>, SymbolId>,
}

/// Lowers the whole program to verified IR. The IR is not yet GC-safe:
/// `gc_roots::insert_gc_roots` adds the root saves and reloads around calls.
pub fn lower_program(
    program: &TypedProgram,
    classes: &ClassTable,
    target: Target,
) -> Result<CheckedIr, String> {
    let mut ctx = Context::new(classes, target);
    ctx.declarations(program)?;
    for class in &program.classes {
        for ctor in &class.constructors {
            let function = lower_constructor(&mut ctx, &class.class_name, ctor)?;
            ctx.ir.functions.push(function);
        }
        for method in &class.methods {
            let function = lower_method(&mut ctx, &class.class_name, method)?;
            ctx.ir.functions.push(function);
        }
    }
    let startup = lower_startup(&mut ctx)?;
    ctx.ir.functions.push(startup);
    ctx.ir.verify()
}

impl<'a> Context<'a> {
    fn new(classes: &'a ClassTable, target: Target) -> Self {
        Self {
            ir: ProgramIr {
                symbols: vec![],
                signatures: vec![],
                functions: vec![],
                data: vec![],
                startup: None,
            },
            classes,
            target,
            layouts: HashMap::new(),
            bytes: HashMap::new(),
        }
    }

    pub fn symbol(&self, name: &str) -> Result<SymbolId, String> {
        self.ir
            .find_symbol(name)
            .ok_or_else(|| format!("IR: missing symbol {name}"))
    }

    pub fn signature(&self, symbol: SymbolId) -> SignatureId {
        match self.ir.symbols[symbol.0].kind {
            SymbolKind::Function(sig) => sig,
            _ => unreachable!(
                "{} is not a function symbol",
                self.ir.symbols[symbol.0].name
            ),
        }
    }

    /// Runtime entry points, every method and constructor, class descriptors,
    /// vtables, the prebound frame, and `lo_entry`.
    fn declarations(&mut self, program: &TypedProgram) -> Result<(), String> {
        for function in crate::ir::runtime::FUNCTIONS {
            self.ir.runtime_function(function.name)?;
        }
        self.ir.empty_string()?;
        for class in &program.classes {
            let name = &class.class_name;
            let info = self
                .classes
                .get(name)
                .ok_or_else(|| format!("IR: missing class {name}"))?;
            for ctor in info.constructors() {
                self.ir.declare_constructor(name, ctor)?;
            }
            for method in &class.methods {
                self.ir
                    .declare_method(name, &info.effective_methods[&method.method_name].sig)?;
            }
            self.layouts
                .insert(name.clone(), class_layout(self.target, self.classes, name));
        }
        crate::ir::statics::define_static_data(&mut self.ir, self.classes, self.target)?;
        let startup = self
            .ir
            .declare_callable("lo_entry", vec![], Some(IrType::Int32))?;
        self.ir.startup = Some(startup);
        Ok(())
    }

    fn layout(&self, class: &str) -> Result<&ClassLayout, String> {
        self.layouts
            .get(class)
            .ok_or_else(|| format!("IR: missing class {class}"))
    }

    pub fn field_offset(&self, owner: &str, name: &str) -> Result<i32, String> {
        let field = self
            .layout(owner)?
            .field(name)
            .ok_or_else(|| format!("IR: missing field {owner}.{name}"))?;
        Ok(field.offset as i32)
    }

    /// Offsets of `class`'s String fields, which start as `LO_EMPTY_STRING`.
    pub fn string_offsets(&self, class: &str) -> Result<Vec<i32>, String> {
        let layout = self.layout(class)?;
        Ok(layout.string_offsets.iter().map(|&o| o as i32).collect())
    }

    /// Where an `Output` object keeps its destination (0 = stdout, 1 = stderr).
    pub fn output_destination(&self) -> Result<i32, String> {
        self.layout("Output")?
            .output_destination
            .map(|offset| offset as i32)
            .ok_or_else(|| "IR: Output has no destination slot".into())
    }

    /// Offset of a prebound name's root in the `lo_bindings` frame.
    pub fn binding_offset(&self, name: &str) -> Result<i32, String> {
        let slot = symbols::PREBOUND
            .iter()
            .position(|n| *n == name)
            .ok_or_else(|| format!("IR: unknown prebound name {name}"))?;
        Ok(self.target.frame().root(slot as u32) as i32)
    }

    /// A read-only byte string, interned so equal literals share one symbol.
    pub fn bytes(&mut self, bytes: &[u8]) -> Result<SymbolId, String> {
        if let Some(id) = self.bytes.get(bytes) {
            return Ok(*id);
        }
        let name = format!("lo_bytes_{}", self.bytes.len());
        let symbol = self.ir.declare_symbol(&name, SymbolKind::Data)?;
        self.ir.data.push(DataDef {
            symbol,
            section: Section::ReadOnly,
            align: 1,
            items: vec![DataItem::Bytes(bytes.to_vec())],
        });
        self.bytes.insert(bytes.to_vec(), symbol);
        Ok(symbol)
    }
}
