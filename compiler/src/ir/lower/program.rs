use std::collections::HashMap;

use crate::ast::Type;
use crate::type_checker::{ClassTable, TypedProgram};

use super::layout::{align, offset};
use super::{
    ctor_symbol, lower_constructor, lower_method, lower_startup, method_symbol, value_type,
    TargetLayout,
};
use crate::ir::*;

#[derive(Clone)]
pub(super) struct ClassLayout {
    pub fields: HashMap<String, i32>,
    pub size: u32,
}

pub(super) struct Context<'a> {
    pub ir: ProgramIr,
    pub classes: &'a ClassTable,
    pub target: TargetLayout,
    pub layouts: HashMap<String, ClassLayout>,
    symbols: HashMap<String, SymbolId>,
    bytes: HashMap<Vec<u8>, SymbolId>,
}

/// Produces checked semantic IR. Safepoint rooting and frame construction are
/// performed by a later pass; CheckedIr alone does not imply moving-GC safety.
pub fn lower_program(
    program: &TypedProgram,
    classes: &ClassTable,
    target: TargetLayout,
) -> Result<CheckedIr, String> {
    let mut ctx = Context::new(classes, target);
    ctx.declarations(program)?;
    for class in &program.classes {
        ctx.class_data(&class.class_name)?;
    }
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
    fn new(classes: &'a ClassTable, target: TargetLayout) -> Self {
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
            symbols: HashMap::new(),
            bytes: HashMap::new(),
        }
    }

    fn declare(&mut self, name: String, kind: SymbolKind) -> Result<SymbolId, String> {
        if self.symbols.contains_key(&name) {
            return Err(format!("IR: duplicate symbol {name}"));
        }
        let id = SymbolId(self.ir.symbols.len());
        self.ir.symbols.push(Symbol {
            name: name.clone(),
            kind,
        });
        self.symbols.insert(name, id);
        Ok(id)
    }

    fn function(
        &mut self,
        name: String,
        params: Vec<IrType>,
        result: Option<IrType>,
    ) -> Result<SymbolId, String> {
        let sig = self.ir.intern_signature(Signature { params, result });
        self.declare(name, SymbolKind::Function(sig))
    }

    pub fn symbol(&self, name: &str) -> Result<SymbolId, String> {
        self.symbols
            .get(name)
            .copied()
            .ok_or_else(|| format!("IR: missing symbol {name}"))
    }

    pub fn signature(&self, symbol: SymbolId) -> SignatureId {
        match self.ir.symbols[symbol.0].kind {
            SymbolKind::Function(sig) => sig,
            _ => unreachable!("registered callable"),
        }
    }

    fn declarations(&mut self, program: &TypedProgram) -> Result<(), String> {
        use IrType::*;
        for (name, params, result) in [
            ("lo_runtime_init", vec![], None),
            ("lo_push_frame", vec![Ptr], None),
            ("lo_pop_frame", vec![], None),
            ("lo_alloc", vec![Ptr], Some(Ref)),
            ("lo_gc_write_barrier", vec![Ref, Int32, Ref], None),
            ("lo_abort_null_receiver", vec![Ptr, Int32], None),
            ("lo_string_new", vec![Ptr, Int32], Some(Ref)),
            ("lo_string_concat", vec![Ref, Ref], Some(Ref)),
            ("lo_string_repeat", vec![Ref, Int32], Some(Ref)),
            ("lo_string_compare", vec![Ref, Ref], Some(Int32)),
            ("lo_string_reverse", vec![Ref], Some(Ref)),
            ("lo_cast_check", vec![Ref, Ptr], Some(Ref)),
            ("lo_instanceof", vec![Ref, Ptr], Some(Bool)),
            ("lo_print_int", vec![Int32, Int32], None),
            ("lo_print_bool", vec![Bool, Int32], None),
            ("lo_print_string", vec![Ref, Int32], None),
            ("lo_println", vec![Int32], None),
            ("lo_read_int", vec![], Some(Int32)),
            ("lo_read_bool", vec![], Some(Bool)),
            ("lo_read_string", vec![], Some(Ref)),
            ("lo_eof", vec![], Some(Bool)),
        ] {
            self.function(name.into(), params, result)?;
        }
        self.declare("LO_EMPTY_STRING".into(), SymbolKind::StaticRef)?;
        let symbol = self.declare("lo_bindings".into(), SymbolKind::Data)?;
        let ptr = self.target.pointer_bytes();
        let frame = self.target.bindings_frame();
        let mut items = vec![DataItem::Zero(ptr), DataItem::U32(3)];
        let padding = frame.roots - (ptr + 4);
        if padding != 0 {
            items.push(DataItem::Zero(padding));
        }
        items.push(DataItem::Zero(3 * ptr));
        self.data(symbol, Section::Writable, ptr, items);
        let startup = self.function("lo_entry".into(), vec![], Some(Int32))?;
        self.ir.startup = Some(startup);
        for class in &program.classes {
            self.layout(&class.class_name)?;
            self.declare(super::class_symbol(&class.class_name), SymbolKind::Data)?;
            self.declare(
                format!("{}_pointers", super::class_symbol(&class.class_name)),
                SymbolKind::Data,
            )?;
            self.declare(
                format!("{}_vtable", super::class_symbol(&class.class_name)),
                SymbolKind::Data,
            )?;
            for ctor in self
                .classes
                .get(&class.class_name)
                .ok_or("IR: missing class")?
                .constructors()
            {
                let mut params = vec![Ref];
                params.extend(
                    ctor.params
                        .iter()
                        .map(value_type)
                        .collect::<Result<Vec<_>, _>>()?,
                );
                self.function(ctor_symbol(&class.class_name, ctor.arity), params, None)?;
            }
            for method in &class.methods {
                let mut params = vec![Ref];
                params.extend(
                    method
                        .formals
                        .iter()
                        .map(|(_, ty)| value_type(ty))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                let result = if method.return_type == Type::Void {
                    None
                } else {
                    Some(value_type(&method.return_type)?)
                };
                self.function(
                    method_symbol(&class.class_name, &method.method_name),
                    params,
                    result,
                )?;
            }
        }
        Ok(())
    }

    fn layout(&mut self, class: &str) -> Result<ClassLayout, String> {
        if let Some(layout) = self.layouts.get(class) {
            return Ok(layout.clone());
        }
        let info = self
            .classes
            .get(class)
            .ok_or_else(|| format!("IR: missing class {class}"))?;
        let header = self.target.object_header_bytes();
        let ptr = self.target.pointer_bytes();
        let field_count =
            u32::try_from(info.effective_fields.len()).map_err(|_| "IR: field count overflow")?;
        let slot_offset = |index: u32| -> Result<i32, String> {
            let bytes = ptr.checked_mul(index).ok_or("IR: layout overflow")?;
            offset(header.checked_add(bytes).ok_or("IR: layout overflow")?)
        };
        let fields = info
            .effective_fields
            .iter()
            .enumerate()
            .map(|(index, field)| Ok((field.name.clone(), slot_offset(index as u32)?)))
            .collect::<Result<HashMap<_, _>, String>>()?;
        let slots = field_count
            .checked_add(u32::from(class == "Output"))
            .ok_or("IR: field count overflow")?;
        let layout = ClassLayout {
            fields,
            size: slot_offset(slots)? as u32,
        };
        self.layouts.insert(class.into(), layout.clone());
        Ok(layout)
    }

    pub fn field_offset(&self, owner: &str, name: &str) -> Result<i32, String> {
        self.layouts
            .get(owner)
            .and_then(|l| l.fields.get(name))
            .copied()
            .ok_or_else(|| format!("IR: missing field {owner}.{name}"))
    }

    pub fn bytes(&mut self, bytes: &[u8]) -> Result<SymbolId, String> {
        if let Some(id) = self.bytes.get(bytes) {
            return Ok(*id);
        }
        super::length(bytes.len())?;
        let symbol = self.declare(format!("lo_bytes_{}", self.bytes.len()), SymbolKind::Data)?;
        self.data(
            symbol,
            Section::ReadOnly,
            1,
            vec![if bytes.is_empty() {
                DataItem::Zero(1)
            } else {
                DataItem::Bytes(bytes.to_vec())
            }],
        );
        self.bytes.insert(bytes.to_vec(), symbol);
        Ok(symbol)
    }

    fn data(&mut self, symbol: SymbolId, section: Section, alignment: u32, items: Vec<DataItem>) {
        self.ir.data.push(DataDef {
            symbol,
            section,
            align: alignment,
            items,
        });
    }

    fn class_data(&mut self, class: &str) -> Result<(), String> {
        let info = self.classes.get(class).ok_or("IR: missing class")?;
        let name = super::class_symbol(class);
        let descriptor = self.symbol(&name)?;
        let pointers = self.symbol(&format!("{name}_pointers"))?;
        let vtable = self.symbol(&format!("{name}_vtable"))?;
        let refs = info
            .effective_fields
            .iter()
            .filter(|f| matches!(f.ty, Type::String | Type::Class(_)))
            .map(|f| {
                self.field_offset(class, &f.name)
                    .map(|n| DataItem::U32(n as u32))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let ref_count = super::length(refs.len())? as u32;
        let entries = info
            .vtable
            .iter()
            .map(|m| {
                self.symbol(&method_symbol(&info.effective_methods[m].owner, m))
                    .map(DataItem::Addr)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let entry_count = super::length(entries.len())? as u32;
        let parent = info
            .parent
            .as_ref()
            .map(|p| self.symbol(&super::class_symbol(p)))
            .transpose()?;
        let size = self.layouts[class].size;
        let name_bytes = self.bytes(&[class.as_bytes(), &[0]].concat())?;
        self.data(
            pointers,
            Section::ReadOnly,
            4,
            if refs.is_empty() {
                vec![DataItem::Zero(4)]
            } else {
                refs
            },
        );
        self.data(
            vtable,
            Section::ReadOnly,
            self.target.pointer_bytes(),
            if entries.is_empty() {
                vec![DataItem::Zero(self.target.pointer_bytes())]
            } else {
                entries
            },
        );
        // C-struct alignment is applied independently to each ABI field.
        let ptr = self.target.pointer_bytes();
        let mut items = vec![];
        let mut cursor = 0;
        for (width, item) in [
            (ptr, DataItem::Addr(name_bytes)),
            (4, DataItem::U32(super::length(class.len())? as u32)),
            (
                ptr,
                parent.map(DataItem::Addr).unwrap_or(DataItem::Zero(ptr)),
            ),
            (4, DataItem::U32(size)),
            (ptr, DataItem::Addr(pointers)),
            (4, DataItem::U32(ref_count)),
            (4, DataItem::U32(entry_count)),
            (ptr, DataItem::Addr(vtable)),
        ] {
            let aligned = align(cursor, width)?;
            if aligned != cursor {
                items.push(DataItem::Zero(aligned - cursor));
            }
            items.push(item);
            cursor = aligned
                .checked_add(width)
                .ok_or("IR: descriptor overflow")?;
        }
        self.data(descriptor, Section::ReadOnly, ptr, items);
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn class_layout_for_test(
    classes: &ClassTable,
    target: TargetLayout,
    class: &str,
) -> Result<(HashMap<String, i32>, u32), String> {
    let layout = Context::new(classes, target).layout(class)?;
    Ok((layout.fields, layout.size))
}
