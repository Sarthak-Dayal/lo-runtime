use std::collections::BTreeMap;
use std::fmt::Write;

use crate::ast::Type;
use crate::type_checker::{ClassTable, TypedClassDecl, TypedConstructor, TypedProgram};

use super::function::Function;
use super::{class_symbol, ctor_symbol, is_reference, method_symbol, signature};

pub(super) struct Module<'a> {
    pub(super) classes: &'a ClassTable,
    declarations: BTreeMap<String, (usize, bool)>,
    data: String,
    literals: BTreeMap<Vec<u8>, String>,
}

// P1: declaration scan, then one emission visit per constructor/method body.
pub fn p1_program(program: &TypedProgram, classes: &ClassTable) -> String {
    let mut module = Module {
        classes,
        declarations: BTreeMap::new(),
        data: String::new(),
        literals: BTreeMap::new(),
    };
    module.runtime_declarations();
    for class in &program.classes {
        for ctor in &class.constructors {
            let arity = match ctor {
                TypedConstructor::Explicit { formals, .. } => formals.len(),
                TypedConstructor::Implicit { fields } => fields.len(),
            };
            module.declare(ctor_symbol(&class.class_name, arity), arity + 1, false);
        }
        for method in &class.methods {
            module.declare(
                method_symbol(&class.class_name, &method.method_name),
                method.formals.len() + 1,
                method.return_type != Type::Void,
            );
        }
        module.p4_class(class);
    }
    module.declare("lo_entry".into(), 0, true);
    for name in ["in", "out", "err"] {
        module.words(&format!("lo_binding_{name}"), &["0".into()], true);
    }
    let mut bodies = String::new();
    for class in &program.classes {
        for ctor in &class.constructors {
            bodies.push_str(&Function::p5_constructor(&mut module, class, ctor));
        }
        for method in &class.methods {
            bodies.push_str(&Function::p7_method(&mut module, class, method));
        }
    }
    bodies.push_str(&Function::entry(&mut module));
    let mut output = String::from("# Generated LO code. P-numbers refer to LO Appendix A.5.\n.globaltype __stack_pointer, i32\n");
    for (symbol, (params, returns)) in &module.declarations {
        writeln!(
            output,
            ".functype {symbol} {}",
            signature(*params, *returns)
        )
        .unwrap();
    }
    output.push_str(&bodies);
    output.push_str(&module.data);
    output
}

impl Module<'_> {
    fn declare(&mut self, name: String, params: usize, returns: bool) {
        self.declarations.insert(name, (params, returns));
    }

    fn runtime_declarations(&mut self) {
        for (name, params, returns) in [
            ("lo_runtime_init", 0, false),
            ("lo_alloc", 1, true),
            ("lo_push_frame", 1, false),
            ("lo_pop_frame", 0, false),
            ("lo_gc_write_barrier", 3, false),
            ("lo_abort_null_receiver", 2, false),
            ("lo_string_new", 2, true),
            ("lo_string_concat", 2, true),
            ("lo_string_repeat", 2, true),
            ("lo_string_compare", 2, true),
            ("lo_string_reverse", 1, true),
            ("lo_cast_check", 2, true),
            ("lo_instanceof", 2, true),
            ("lo_print_int", 1, false),
            ("lo_print_bool", 1, false),
            ("lo_print_string", 1, false),
            ("lo_println", 0, false),
            ("lo_read_int", 0, true),
            ("lo_read_bool", 0, true),
            ("lo_read_string", 0, true),
            ("lo_eof", 0, true),
        ] {
            self.declare(name.into(), params, returns);
        }
    }

    // P4/P11: inherit field offsets and vtable slots from the checked class table.
    fn p4_class(&mut self, class: &TypedClassDecl) {
        let name = &class.class_name;
        let info = self.classes.get(name).expect("checked class");
        let symbol = class_symbol(name);
        let name_bytes = self.bytes(&[name.as_bytes(), &[0]].concat());
        let pointers: Vec<_> = info
            .effective_fields
            .iter()
            .enumerate()
            .filter(|(_, field)| is_reference(&field.ty))
            .map(|(i, _)| (12 + i * 4).to_string())
            .collect();
        let vtable: Vec<_> = info
            .vtable
            .iter()
            .map(|method| method_symbol(&info.effective_methods[method].owner, method))
            .collect();
        self.words(&format!("{symbol}_pointers"), &pointers, false);
        self.words(&format!("{symbol}_vtable"), &vtable, false);
        let size = 12 + 4 * info.effective_fields.len() + if name == "Output" { 4 } else { 0 };
        self.words(
            &symbol,
            &[
                name_bytes,
                name.len().to_string(),
                info.parent
                    .as_deref()
                    .map(class_symbol)
                    .unwrap_or_else(|| "0".into()),
                size.to_string(),
                format!("{symbol}_pointers"),
                pointers.len().to_string(),
                vtable.len().to_string(),
                format!("{symbol}_vtable"),
            ],
            false,
        );
    }

    fn words(&mut self, symbol: &str, words: &[String], writable: bool) {
        let section = if writable { "data" } else { "rodata" };
        writeln!(
            self.data,
            ".section .{section}.{symbol},\"\",@\n.p2align 2\n.type {symbol},@object\n{symbol}:"
        )
        .unwrap();
        for word in words {
            writeln!(self.data, "    .int32 {word}").unwrap();
        }
        // Empty tables still have a distinct, addressable symbol.
        if words.is_empty() {
            self.data.push_str("    .int32 0\n");
        }
        writeln!(self.data, ".size {symbol}, {}", words.len().max(1) * 4).unwrap();
    }

    pub(super) fn bytes(&mut self, bytes: &[u8]) -> String {
        if let Some(symbol) = self.literals.get(bytes) {
            return symbol.clone();
        }
        let symbol = format!("lo_bytes_{}", self.literals.len());
        writeln!(
            self.data,
            ".section .rodata.{symbol},\"\",@\n.type {symbol},@object\n{symbol}:"
        )
        .unwrap();
        for byte in bytes {
            writeln!(self.data, "    .int8 {byte}").unwrap();
        }
        if bytes.is_empty() {
            self.data.push_str("    .int8 0\n");
        }
        writeln!(self.data, ".size {symbol}, {}", bytes.len().max(1)).unwrap();
        self.literals.insert(bytes.to_vec(), symbol.clone());
        symbol
    }

    pub(super) fn field_offset(&self, owner: &str, name: &str) -> usize {
        12 + 4 * self
            .classes
            .get(owner)
            .expect("checked field owner")
            .effective_fields
            .iter()
            .position(|f| f.name == name)
            .expect("checked field")
    }
}
