use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use crate::ast::{IoOp, Type};
use crate::type_checker::{
    TypedClassDecl, TypedConstructor, TypedDelegation, TypedMethodBody, TypedMethodDecl,
};

use super::module::Module;
use super::{ctor_symbol, is_reference, method_symbol, signature};

pub(super) type Value = usize;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Control {
    Exit,
    LoopExit,
    LoopHead,
    If,
}

struct Local {
    name: String,
    root: Option<usize>,
}

pub(super) struct Function<'m, 'a> {
    pub(super) module: &'m mut Module<'a>,
    pub(super) class: String,
    pub(super) bindings: BTreeMap<String, Value>,
    pub(super) return_value: Option<Value>,
    pub(super) controls: Vec<Control>,
    symbol: String,
    params: usize,
    locals: Vec<Local>,
    roots: usize,
    active: BTreeSet<Value>,
    initial_roots: Vec<Value>,
    string_defaults: Vec<Value>,
    frame: Value,
    old_sp: Value,
    code: String,
    entry: bool,
}

impl<'m, 'a> Function<'m, 'a> {
    fn new(
        module: &'m mut Module<'a>,
        class: &str,
        symbol: String,
        formals: &[(String, Type)],
        locals: &[(String, Type)],
        result: &Type,
        entry: bool,
    ) -> Self {
        let mut f = Self {
            module,
            class: class.into(),
            symbol,
            bindings: BTreeMap::new(),
            params: 0,
            locals: Vec::new(),
            roots: if entry { 3 } else { 0 },
            active: BTreeSet::new(),
            initial_roots: Vec::new(),
            string_defaults: Vec::new(),
            return_value: None,
            frame: 0,
            old_sp: 0,
            code: String::new(),
            controls: vec![Control::Exit],
            entry,
        };
        if !entry {
            f.named("this", &Type::Class(class.into()));
        }
        f.p9_formals(formals);
        f.params = f.locals.len();
        f.p11_locals(locals);
        if result != &Type::Void {
            f.return_value = Some(f.named("$return", result));
        }
        f.old_sp = f.named("$old_sp", &Type::Int);
        f.frame = f.named("$frame", &Type::Int);
        f.initial_roots = f.active.iter().copied().collect();
        f.ins("block");
        f
    }

    // P5/P6: constructors take an existing receiver and return no value.
    pub(super) fn p5_constructor(
        module: &'m mut Module<'a>,
        class: &TypedClassDecl,
        ctor: &TypedConstructor,
    ) -> String {
        let (formals, locals) = match ctor {
            TypedConstructor::Explicit {
                formals, locals, ..
            } => (formals.as_slice(), locals.as_slice()),
            TypedConstructor::Implicit { fields } => (fields.as_slice(), &[][..]),
        };
        let mut f = Self::new(
            module,
            &class.class_name,
            ctor_symbol(&class.class_name, formals.len()),
            formals,
            locals,
            &Type::Void,
            false,
        );
        f.ins("# P5/P6: constructor");
        match ctor {
            TypedConstructor::Explicit {
                delegation, stmts, ..
            } => {
                if let Some(delegation) = delegation {
                    f.p6_delegation(delegation);
                }
                f.p12_block(stmts);
            }
            TypedConstructor::Implicit { fields } => {
                for (name, ty) in fields {
                    let offset = f.module.field_offset(&class.class_name, name);
                    f.store_field(0, offset, f.bindings[name], ty);
                }
            }
        }
        f.finish()
    }

    // P5/P6: this(...) or super(...), selected by checked class and arity.
    fn p6_delegation(&mut self, delegation: &TypedDelegation) {
        let (class, actuals) = match delegation {
            TypedDelegation::ThisCall { actuals, .. } => (self.class.clone(), actuals),
            TypedDelegation::SuperCall { actuals, .. } => (
                self.module
                    .classes
                    .get(&self.class)
                    .unwrap()
                    .parent
                    .clone()
                    .expect("checked super"),
                actuals,
            ),
        };
        let actuals = self.p10_actuals(actuals);
        let args: Vec<_> = std::iter::once(0).chain(actuals.iter().copied()).collect();
        self.call(&ctor_symbol(&class, actuals.len()), &args, &Type::Void);
        self.release_all(&actuals);
    }

    // P7: emit one method; every return branches to its shared frame cleanup.
    pub(super) fn p7_method(
        module: &'m mut Module<'a>,
        class: &TypedClassDecl,
        method: &TypedMethodDecl,
    ) -> String {
        let locals = match &method.body {
            TypedMethodBody::UserDefined { locals, .. } => locals.as_slice(),
            _ => &[],
        };
        let mut f = Self::new(
            module,
            &class.class_name,
            method_symbol(&class.class_name, &method.method_name),
            &method.formals,
            locals,
            &method.return_type,
            false,
        );
        f.ins("# P7: method");
        match &method.body {
            TypedMethodBody::UserDefined { stmts, .. } => f.p12_block(stmts),
            TypedMethodBody::Io(op) => f.io_wrapper(op, &method.return_type),
        }
        f.finish()
    }

    // P9: receiver is parameter 0; source formals follow in declaration order.
    fn p9_formals(&mut self, formals: &[(String, Type)]) {
        for (name, ty) in formals {
            self.named(name, ty);
        }
    }

    // P11: hoisted locals are initialized once, including skipped-block locals.
    fn p11_locals(&mut self, locals: &[(String, Type)]) {
        for (name, ty) in locals {
            let local = self.named(name, ty);
            if ty == &Type::String {
                self.string_defaults.push(local);
            }
        }
    }

    fn io_wrapper(&mut self, op: &IoOp, result: &Type) {
        let (target, output) = match op {
            IoOp::ReadInt => ("lo_read_int", false),
            IoOp::ReadBool => ("lo_read_bool", false),
            IoOp::ReadString => ("lo_read_string", false),
            IoOp::Eof => ("lo_eof", false),
            IoOp::PrintInt => ("lo_print_int", true),
            IoOp::PrintBool => ("lo_print_bool", true),
            IoOp::PrintString => ("lo_print_string", true),
            IoOp::Println => ("lo_println", true),
        };
        let mut args: Vec<_> = (1..self.params).collect();
        if output {
            // Output carries the dynamic destination used by out, err, fields,
            // formals, and aliases of either pre-bound object.
            let destination = self.temporary(&Type::Int);
            self.get(0);
            self.ins("i32.load 12");
            self.set(destination);
            args.push(destination);
        }
        if let Some(value) = self.call(target, &args, result) {
            self.return_saved(value);
        }
    }

    pub(super) fn entry(module: &'m mut Module<'a>) -> String {
        let mut f = Self::new(module, "", "lo_entry".into(), &[], &[], &Type::Int, true);
        f.ins("# P1: persistent I/O roots exist before Main construction");
        for (slot, name) in ["in", "out", "err"].iter().enumerate() {
            f.ins(format!("i32.const lo_binding_{name}"));
            f.get(f.frame);
            f.ins(format!("i32.const {}", 8 + 4 * slot));
            f.ins("i32.add");
            f.ins("i32.store 0");
            let class = if *name == "in" { "Input" } else { "Output" };
            let object = f.allocate(class, &[]);
            if class == "Output" {
                f.get(object);
                f.ins(format!("i32.const {}", i32::from(*name == "err")));
                f.ins("i32.store 12");
            }
            f.get(f.frame);
            f.get(object);
            f.ins(format!("i32.store {}", 8 + 4 * slot));
            f.release(object);
        }
        let main = f.allocate("Main", &[]);
        let slot = f.module.classes.get("Main").unwrap().method_slot["main"];
        let result = f.virtual_call(main, &[], slot, &Type::Int).unwrap();
        f.release(main);
        f.return_saved(result);
        f.finish()
    }

    fn named(&mut self, name: &str, ty: &Type) -> Value {
        let index = self.temporary(ty);
        self.locals[index].name = name.into();
        self.bindings.insert(name.into(), index);
        index
    }

    pub(super) fn temporary(&mut self, ty: &Type) -> Value {
        let index = self.locals.len();
        let root = if is_reference(ty) {
            let slot = self.roots;
            self.roots += 1;
            self.active.insert(index);
            Some(slot)
        } else {
            None
        };
        self.locals.push(Local {
            name: format!("$temp{index}"),
            root,
        });
        index
    }

    pub(super) fn ins(&mut self, instruction: impl AsRef<str>) {
        writeln!(self.code, "    {}", instruction.as_ref()).unwrap();
    }

    pub(super) fn get(&mut self, value: Value) {
        self.ins(format!("local.get {value}"));
    }
    pub(super) fn set(&mut self, value: Value) {
        self.ins(format!("local.set {value}"));
    }

    pub(super) fn constant(&mut self, value: impl std::fmt::Display) -> Value {
        let local = self.temporary(&Type::Int);
        self.ins(format!("i32.const {value}"));
        self.set(local);
        local
    }

    pub(super) fn copy(&mut self, value: Value, ty: &Type) -> Value {
        let local = self.temporary(ty);
        self.get(value);
        self.set(local);
        local
    }

    pub(super) fn release(&mut self, value: Value) {
        if self.active.remove(&value) {
            // Clear both copies. A later loop iteration must not publish an old address.
            self.ins("i32.const 0");
            self.set(value);
            self.get(self.frame);
            self.ins("i32.const 0");
            self.ins(format!("i32.store {}", self.root_offset(value)));
        }
    }

    pub(super) fn release_all(&mut self, values: &[Value]) {
        for &value in values {
            self.release(value);
        }
    }

    fn root_offset(&self, value: Value) -> usize {
        8 + 4 * self.locals[value].root.unwrap()
    }

    fn publish(&mut self, roots: &[Value]) {
        for &value in roots {
            self.get(self.frame);
            self.get(value);
            self.ins(format!("i32.store {}", self.root_offset(value)));
        }
    }

    fn reload(&mut self, roots: &[Value]) {
        for &value in roots {
            self.get(self.frame);
            self.ins(format!("i32.load {}", self.root_offset(value)));
            self.set(value);
        }
    }

    pub(super) fn call(&mut self, target: &str, args: &[Value], result: &Type) -> Option<Value> {
        self.call_instruction(&format!("call {target}"), args, result)
    }

    fn call_instruction(
        &mut self,
        instruction: &str,
        args: &[Value],
        result: &Type,
    ) -> Option<Value> {
        let roots: Vec<_> = self.active.iter().copied().collect();
        self.publish(&roots);
        for &arg in args {
            self.get(arg);
        }
        self.ins(instruction);
        // Allocate the result after taking the pre-call snapshot: reload must not overwrite it.
        let value = if result != &Type::Void {
            let value = self.temporary(result);
            self.set(value);
            Some(value)
        } else {
            None
        };
        self.reload(&roots);
        value
    }

    pub(super) fn virtual_call(
        &mut self,
        receiver: Value,
        actuals: &[Value],
        slot: usize,
        result: &Type,
    ) -> Option<Value> {
        let index = self.temporary(&Type::Int);
        self.get(receiver);
        self.ins("i32.load 0 # Object.class_descriptor");
        self.ins("i32.load 28 # ClassDescriptor.vtable");
        self.ins(format!("i32.load {}", slot * 4));
        self.set(index);
        let args: Vec<_> = std::iter::once(receiver)
            .chain(actuals.iter().copied())
            .chain(std::iter::once(index))
            .collect();
        self.call_instruction(
            &format!(
                "call_indirect {}",
                signature(actuals.len() + 1, result != &Type::Void)
            ),
            &args,
            result,
        )
    }

    pub(super) fn open_if(&mut self, result: bool) {
        self.ins(if result { "if i32" } else { "if" });
        self.controls.push(Control::If);
    }

    pub(super) fn end_if(&mut self) {
        assert!(self.controls.pop() == Some(Control::If));
        self.ins("end_if");
    }

    pub(super) fn branch(&mut self, target: Control, conditional: bool) {
        let depth = self
            .controls
            .iter()
            .rev()
            .position(|c| *c == target)
            .expect("checked branch target");
        self.ins(format!(
            "{} {depth}",
            if conditional { "br_if" } else { "br" }
        ));
    }

    pub(super) fn return_saved(&mut self, value: Value) {
        self.get(value);
        self.set(self.return_value.expect("checked value return"));
        self.release(value);
        self.branch(Control::Exit, false);
    }

    fn finish(mut self) -> String {
        if self.return_value.is_some() {
            self.ins("unreachable # checked non-void fallthrough");
        }
        self.ins("end_block");
        assert!(self.controls.pop() == Some(Control::Exit));
        assert!(self.controls.is_empty());
        let roots: Vec<_> = self.active.iter().copied().collect();
        self.ins("# LEAVE: the frame remains reserved until after pop and reload");
        self.publish(&roots);
        self.ins("call lo_pop_frame");
        self.reload(&roots);
        self.get(self.old_sp);
        self.ins("global.set __stack_pointer");
        if let Some(value) = self.return_value {
            self.get(value);
        }
        self.ins("end_function");
        let body = std::mem::take(&mut self.code);
        if self.entry {
            self.ins("call lo_runtime_init");
        }
        for local in self.string_defaults.clone() {
            self.ins("i32.const LO_EMPTY_STRING");
            self.set(local);
        }
        self.ins("# ENTER: parent at 0, count at 4, inline root slots at 8");
        self.ins("global.get __stack_pointer");
        self.set(self.old_sp);
        self.get(self.old_sp);
        self.ins(format!("i32.const {}", (8 + 4 * self.roots + 15) & !15));
        self.ins("i32.sub");
        self.set(self.frame);
        self.get(self.frame);
        self.ins("global.set __stack_pointer");
        for (offset, value) in [(0, 0), (4, self.roots)] {
            self.get(self.frame);
            self.ins(format!("i32.const {value}"));
            self.ins(format!("i32.store {offset}"));
        }
        for slot in 0..self.roots {
            self.get(self.frame);
            self.ins("i32.const 0");
            self.ins(format!("i32.store {}", 8 + 4 * slot));
        }
        let initial = self.initial_roots.clone();
        self.publish(&initial);
        self.get(self.frame);
        self.ins("call lo_push_frame");
        self.reload(&initial);
        let symbol = &self.symbol;
        let mut output = format!(".section .text.{symbol},\"\",@\n.globl {symbol}\n.type {symbol},@function\n{symbol}:\n    .functype {symbol} {}\n", signature(self.params, self.return_value.is_some()));
        let count = self.locals.len() - self.params;
        if count > 0 {
            writeln!(output, "    .local {}", vec!["i32"; count].join(", ")).unwrap();
        }
        for (i, local) in self.locals.iter().enumerate() {
            if !local.name.starts_with("$temp") {
                writeln!(output, "    # local {i}: {}", local.name).unwrap();
            }
        }
        output.push_str(&self.code);
        output.push_str(&body);
        output
    }
}
