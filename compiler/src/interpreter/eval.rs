use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::{Binop, IoOp, Type, Unop};
use crate::type_checker::{
    BindingInfo, CastDirection, ClassTable, MethodResolution, TypedClassDecl, TypedConstructor,
    TypedDelegation, TypedExpr, TypedMethodBody, TypedMethodCall, TypedMethodDecl, TypedObjName,
    TypedProgram, TypedStmt,
};

use super::abort::AbortKind;
use super::env::Frame;
use super::heap::{Heap, ObjId, StrId};
use super::io::Io;
use super::strings;
use super::value::{type_default, Value};
use super::Outcome;

/// A non-local transfer of control out of an expression or statement.
#[derive(Debug)]
pub enum Signal {
    /// `break` — unwinds to the nearest enclosing `while`.
    Break,
    /// `return e` — unwinds to the call boundary, carrying the value.
    Return(Value),
    /// A runtime abort — unwinds all the way to `interpret`.
    Abort(AbortKind),
}

pub type Exec<T> = Result<T, Signal>;

/// The interpreter's run state. Borrows the checker's outputs immutably and owns
/// the heap; the borrowed `'p` references detach from `&self`, so class-table and
/// program lookups can coexist with `&mut self.heap`.
pub struct Interp<'p> {
    table: &'p ClassTable,
    /// Class declaration by name (for constructor lookup and field layout).
    classes: HashMap<String, &'p TypedClassDecl>,
    /// User-defined method bodies, keyed declaring-class to method name to delc. The
    /// override winner is found via `effective_methods[m].owner`, then indexed
    /// here. I/O methods have no body and never appear.
    method_bodies: HashMap<String, HashMap<String, &'p TypedMethodDecl>>,
    heap: Heap,
    literals: HashMap<String, StrId>,
    io: Io,
    /// The three preamble singletons. `out`/`err` share the class `Output`, so the
    /// stream is told apart by object identity, not by class (design §8).
    in_id: ObjId,
    out_id: ObjId,
    err_id: ObjId,
}

impl<'p> Interp<'p> {
    /// Build an interpreter whose I/O is discarded (no input, output to a sink).
    pub fn new(program: &'p TypedProgram, table: &'p ClassTable, heap_limit: usize) -> Self {
        Self::with_io(program, table, heap_limit, Io::sink())
    }

    /// Build an interpreter with explicit, injectable I/O streams.
    pub fn with_io(
        program: &'p TypedProgram,
        table: &'p ClassTable,
        heap_limit: usize,
        io: Io,
    ) -> Self {
        let mut classes = HashMap::new();
        let mut method_bodies: HashMap<String, HashMap<String, &'p TypedMethodDecl>> =
            HashMap::new();
        for class in &program.classes {
            classes.insert(class.class_name.clone(), class);
            for method in &class.methods {
                if let TypedMethodBody::UserDefined { .. } = method.body {
                    method_bodies
                        .entry(class.class_name.clone())
                        .or_default()
                        .insert(method.method_name.clone(), method);
                }
            }
        }
        let mut heap = Heap::new(heap_limit);
        let in_id = heap.alloc_singleton(Rc::from("Input"));
        let out_id = heap.alloc_singleton(Rc::from("Output"));
        let err_id = heap.alloc_singleton(Rc::from("Output"));
        Interp {
            table,
            classes,
            method_bodies,
            heap,
            literals: HashMap::new(),
            io,
            in_id,
            out_id,
            err_id,
        }
    }

    /// Run the program: construct `Main` via its zero-arg constructor and call
    /// `int Main.main()`, mirroring the ABI's `lo_entry` (design §10). `Main`'s
    /// return value is the exit status; any abort en route becomes `Outcome::Abort`.
    pub fn run(&mut self) -> Outcome {
        let main = match self.construct("Main", vec![]) {
            Ok(id) => id,
            Err(Signal::Abort(a)) => return Outcome::Abort(a),
            Err(_) => unreachable!("interpreter invariant: non-abort signal constructing Main"),
        };
        let body = self.body("Main", "main");
        match self.invoke(Some(main), body, vec![]) {
            Ok(Value::Int(n)) => Outcome::Exit(n),
            Ok(_) => unreachable!("interpreter invariant: Main.main() returns int"),
            Err(Signal::Abort(a)) => Outcome::Abort(a),
            Err(_) => unreachable!("interpreter invariant: non-abort signal escaped main"),
        }
    }

    // --- expressions ------------------------------------------------------

    pub fn eval_expr(&mut self, e: &TypedExpr, frame: &mut Frame) -> Exec<Value> {
        match e {
            TypedExpr::Num(n, _) => Ok(Value::Int(*n)),
            TypedExpr::Bool(b, _) => Ok(Value::Bool(*b)),
            TypedExpr::Str(s, _) => Ok(Value::Str(self.intern_literal(s))),
            TypedExpr::Null(_) => Ok(Value::Obj(None)),
            TypedExpr::This(_, _) => Ok(Value::Obj(frame.this)),
            TypedExpr::Var { name, binding, .. } => Ok(self.read_var(frame, name, binding)),
            TypedExpr::New { class, actuals, .. } => {
                let vals = self.eval_args(actuals, frame)?;
                let id = self.construct(class, vals)?;
                Ok(Value::Obj(Some(id)))
            }
            TypedExpr::Call(call) => self.eval_call(call, frame),
            TypedExpr::Unop { op, operand, .. } => {
                let v = self.eval_expr(operand, frame)?;
                self.eval_unary(*op, v)
            }
            TypedExpr::Binop { lhs, op, rhs, .. } => self.eval_binary(*op, lhs, rhs, frame),
            TypedExpr::Ternary {
                cond,
                then_branch,
                else_branch,
                ..
            } => match self.eval_expr(cond, frame)? {
                Value::Bool(true) => self.eval_expr(then_branch, frame),
                Value::Bool(false) => self.eval_expr(else_branch, frame),
                _ => unreachable!("interpreter invariant: ternary condition is not bool"),
            },
            TypedExpr::Cast {
                target,
                operand,
                direction,
                ..
            } => {
                let v = self.eval_expr(operand, frame)?;
                self.eval_cast(target, *direction, v)
            }
            TypedExpr::InstanceOf { operand, class, .. } => {
                let v = self.eval_expr(operand, frame)?;
                Ok(Value::Bool(self.is_instance(v, class)))
            }
        }
    }

    fn eval_cast(&self, target: &Type, direction: CastDirection, v: Value) -> Exec<Value> {
        match direction {
            // Upcast is identity; a null-source cast always succeeds and yields
            // null — neither needs a runtime check.
            CastDirection::Upcast | CastDirection::Null => Ok(v),
            CastDirection::Downcast => {
                let Type::Class(target_class) = target else {
                    unreachable!("interpreter invariant: cast target is not a class type");
                };
                match v {
                    // A null value casts to any class (checked before the walk).
                    Value::Obj(None) => Ok(Value::Obj(None)),
                    Value::Obj(Some(id)) => {
                        let table = self.table;
                        let runtime_class = self.heap.class_of(id);
                        if table.is_subtype(runtime_class, target_class) {
                            Ok(Value::Obj(Some(id)))
                        } else {
                            Err(Signal::Abort(AbortKind::CastFailed {
                                from: runtime_class.to_string(),
                                to: target_class.clone(),
                            }))
                        }
                    }
                    _ => unreachable!("interpreter invariant: cast operand is not an object"),
                }
            }
        }
    }

    fn is_instance(&self, v: Value, class: &str) -> bool {
        match v {
            // Null is never an instance of anything, and never aborts.
            Value::Obj(None) => false,
            Value::Obj(Some(id)) => self.table.is_subtype(self.heap.class_of(id), class),
            _ => unreachable!("interpreter invariant: instanceof operand is not an object"),
        }
    }

    fn eval_unary(&mut self, op: Unop, v: Value) -> Exec<Value> {
        match (op, v) {
            (Unop::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
            (Unop::Neg, Value::Int(n)) => Ok(Value::Int(n.wrapping_neg())),
            (Unop::Neg, Value::Str(s)) => {
                let id = strings::reverse(&mut self.heap, s).map_err(Signal::Abort)?;
                Ok(Value::Str(id))
            }
            _ => unreachable!("interpreter invariant: checker rejects this unary operand"),
        }
    }

    fn eval_binary(
        &mut self,
        op: Binop,
        lhs: &TypedExpr,
        rhs: &TypedExpr,
        frame: &mut Frame,
    ) -> Exec<Value> {
        // `&` and `|` are boolean AND/OR and short-circuit: the right operand is
        // evaluated only when the left does not already decide the result.
        if matches!(op, Binop::And | Binop::Or) {
            let Value::Bool(l) = self.eval_expr(lhs, frame)? else {
                unreachable!("interpreter invariant: logical operand is not bool");
            };
            match (op, l) {
                (Binop::And, false) => return Ok(Value::Bool(false)),
                (Binop::Or, true) => return Ok(Value::Bool(true)),
                _ => {}
            }
            let Value::Bool(r) = self.eval_expr(rhs, frame)? else {
                unreachable!("interpreter invariant: logical operand is not bool");
            };
            return Ok(Value::Bool(r));
        }

        let l = self.eval_expr(lhs, frame)?;
        let r = self.eval_expr(rhs, frame)?;
        match (l, r) {
            (Value::Int(a), Value::Int(b)) => Ok(eval_int_binop(op, a, b)),
            (Value::Str(a), Value::Int(n)) if op == Binop::Mul => {
                let id = strings::repeat(&mut self.heap, a, n).map_err(Signal::Abort)?;
                Ok(Value::Str(id))
            }
            (Value::Str(a), Value::Str(b)) => self.eval_str_binop(op, a, b),
            // Reference equality. The checker admits `=` between objects only when
            // one side is the `null` literal (`x = null`, `null = null`), so this is
            // the null-check idiom: equal iff the same handle, with `None` (LO null)
            // equal only to `None`.
            (Value::Obj(a), Value::Obj(b)) if op == Binop::Eq => Ok(Value::Bool(a == b)),
            _ => {
                unreachable!("interpreter invariant: checker rejects this operand pair for {op:?}")
            }
        }
    }

    fn eval_str_binop(&mut self, op: Binop, a: StrId, b: StrId) -> Exec<Value> {
        match op {
            Binop::Add => {
                let id = strings::concat(&mut self.heap, a, b).map_err(Signal::Abort)?;
                Ok(Value::Str(id))
            }
            Binop::Lt => Ok(Value::Bool(
                strings::compare(&self.heap, a, b) == Ordering::Less,
            )),
            Binop::Gt => Ok(Value::Bool(
                strings::compare(&self.heap, a, b) == Ordering::Greater,
            )),
            Binop::Eq => Ok(Value::Bool(
                strings::compare(&self.heap, a, b) == Ordering::Equal,
            )),
            _ => unreachable!("interpreter invariant: {op:?} is not a string operator"),
        }
    }

    fn intern_literal(&mut self, s: &str) -> StrId {
        if let Some(&id) = self.literals.get(s) {
            return id;
        }
        let id = self.heap.intern_static(s);
        self.literals.insert(s.to_string(), id);
        id
    }

    fn eval_args(&mut self, args: &[TypedExpr], frame: &mut Frame) -> Exec<Vec<Value>> {
        // Left-to-right, which is observable (an actual may read input).
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            out.push(self.eval_expr(a, frame)?);
        }
        Ok(out)
    }

    // --- dispatch ---------------------------------------------------------

    fn eval_call(&mut self, call: &TypedMethodCall, frame: &mut Frame) -> Exec<Value> {
        match &call.resolution {
            MethodResolution::Virtual { .. } => {
                let receiver = self.eval_obj_name(&call.obj_name, frame)?;
                let id = match receiver {
                    Value::Obj(Some(id)) => id,
                    Value::Obj(None) => {
                        return Err(Signal::Abort(AbortKind::NullReceiver {
                            method: call.method_name.clone(),
                        }))
                    }
                    _ => unreachable!("interpreter invariant: virtual receiver is not an object"),
                };
                let args = self.eval_args(&call.actuals, frame)?;
                // Dispatch on the receiver's *runtime* class, then run the
                // override winner's body.
                let table = self.table;
                let runtime_class = self.heap.class_of(id).to_string();
                let owner = table
                    .get(&runtime_class)
                    .and_then(|info| info.effective_methods.get(&call.method_name))
                    .map(|entry| entry.owner.as_str())
                    .expect("interpreter invariant: method resolved by checker");
                let body = self.body(owner, &call.method_name);
                self.invoke(Some(id), body, args)
            }
            MethodResolution::Super { declaring_class } => {
                // Statically fixed to the ancestor the checker chose; `this` is
                // the current receiver and is never null in a method body.
                let args = self.eval_args(&call.actuals, frame)?;
                let body = self.body(declaring_class, &call.method_name);
                self.invoke(frame.this, body, args)
            }
            MethodResolution::Io { op } => {
                let receiver = self.eval_obj_name(&call.obj_name, frame)?;
                // A method dispatch on a null receiver aborts (102) regardless of
                // whether the method is user-defined or a built-in I/O op — same
                // check the virtual arm makes. `in`/`out`/`err` are never null, but
                // a null `Input`/`Output` variable can reach here.
                if let Value::Obj(None) = receiver {
                    return Err(Signal::Abort(AbortKind::NullReceiver {
                        method: call.method_name.clone(),
                    }));
                }
                let args = self.eval_args(&call.actuals, frame)?;
                self.run_io(op, receiver, args)
            }
        }
    }

    /// Run a built-in I/O operation. Print ops choose the stream by comparing the
    /// receiver's `ObjId` to `err`; the returned value is a discarded void
    /// sentinel for the print/println ops.
    fn run_io(&mut self, op: &IoOp, receiver: Value, args: Vec<Value>) -> Exec<Value> {
        let to_err = matches!(receiver, Value::Obj(Some(id)) if id == self.err_id);
        let void = Value::Obj(None);
        match op {
            IoOp::PrintInt => {
                let Value::Int(n) = args[0] else {
                    unreachable!("interpreter invariant: print_int arg is not int");
                };
                self.io.print_int(n, to_err);
                Ok(void)
            }
            IoOp::PrintBool => {
                let Value::Bool(b) = args[0] else {
                    unreachable!("interpreter invariant: print_bool arg is not bool");
                };
                self.io.print_bool(b, to_err);
                Ok(void)
            }
            IoOp::PrintString => {
                match args[0] {
                    Value::Str(id) => {
                        let s = self.heap.str_value(id);
                        self.io.print_string(&s, to_err);
                    }
                    // A null String argument prints nothing and does not abort.
                    Value::Obj(None) => {}
                    _ => unreachable!("interpreter invariant: print_string arg is not a string"),
                }
                Ok(void)
            }
            IoOp::Println => {
                self.io.println(to_err);
                Ok(void)
            }
            IoOp::ReadInt => self.io.read_int().map(Value::Int).map_err(Signal::Abort),
            IoOp::ReadBool => self.io.read_bool().map(Value::Bool).map_err(Signal::Abort),
            IoOp::ReadString => {
                let s = self.io.read_string();
                let id = self.heap.alloc_str(&s).map_err(Signal::Abort)?;
                Ok(Value::Str(id))
            }
            IoOp::Eof => Ok(Value::Bool(self.io.eof())),
        }
    }

    fn eval_obj_name(&mut self, obj_name: &TypedObjName, frame: &mut Frame) -> Exec<Value> {
        match obj_name {
            TypedObjName::This(_, _) | TypedObjName::Super(_) => Ok(Value::Obj(frame.this)),
            TypedObjName::Var { name, binding, .. } => Ok(self.read_var(frame, name, binding)),
            TypedObjName::Computed(e, _) => self.eval_expr(e, frame),
        }
    }

    /// The user-defined body declared in `owner` under `method` (detached `'p`).
    fn body(&self, owner: &str, method: &str) -> &'p TypedMethodDecl {
        self.method_bodies
            .get(owner)
            .and_then(|m| m.get(method))
            .copied()
            .expect("interpreter invariant: method body indexed at startup")
    }

    /// Run a user-defined method body in a fresh frame.
    fn invoke(
        &mut self,
        this: Option<ObjId>,
        method: &'p TypedMethodDecl,
        args: Vec<Value>,
    ) -> Exec<Value> {
        match &method.body {
            TypedMethodBody::UserDefined { locals, stmts } => {
                let empty = self.heap.empty();
                let mut frame = Frame::new(this, &method.formals, args, locals, empty);
                match self.exec_block(stmts, &mut frame) {
                    // A void method falls off the end; its result is never used
                    // (the checker forbids using a void call as a value), so this
                    // sentinel is only ever discarded by a `CallStmt`.
                    Ok(()) => Ok(Value::Obj(None)),
                    Err(Signal::Return(v)) => Ok(v),
                    Err(Signal::Break) => {
                        unreachable!("interpreter invariant: break escaped a method")
                    }
                    Err(abort @ Signal::Abort(_)) => Err(abort),
                }
            }
            TypedMethodBody::Io(_) => {
                unreachable!("interpreter invariant: I/O methods dispatch via MethodResolution::Io")
            }
        }
    }

    // --- objects and constructors ----------------------------------------

    /// Allocate an object of `class` (fields at their type-defaults, parent-first)
    /// and run the arity-selected constructor. Returns its handle.
    fn construct(&mut self, class: &str, args: Vec<Value>) -> Exec<ObjId> {
        let table = self.table;
        let info = table
            .get(class)
            .expect("interpreter invariant: class in table");
        let empty = self.heap.empty();
        let fields: Vec<Value> = info
            .effective_fields
            .iter()
            .map(|f| type_default(&f.ty, empty))
            .collect();
        let id = self
            .heap
            .alloc_obj(Rc::from(class), fields)
            .map_err(Signal::Abort)?;

        let decl = self
            .classes
            .get(class)
            .copied()
            .expect("interpreter invariant: class decl");
        let ctor = select_ctor(decl, args.len());
        self.run_ctor(id, class, ctor, args)?;
        Ok(id)
    }

    fn run_ctor(
        &mut self,
        this: ObjId,
        class: &str,
        ctor: &'p TypedConstructor,
        args: Vec<Value>,
    ) -> Exec<()> {
        match ctor {
            TypedConstructor::Implicit { fields } => {
                // `this.field_i = formal_i`, positionally, in field order.
                for (i, (name, _)) in fields.iter().enumerate() {
                    let slot = self.field_slot(this, name);
                    self.heap.set_field(this, slot, args[i]);
                }
                Ok(())
            }
            TypedConstructor::Explicit {
                formals,
                delegation,
                locals,
                stmts,
                ..
            } => {
                let empty = self.heap.empty();
                let mut frame = Frame::new(Some(this), formals, args, locals, empty);
                // Delegation runs first and to completion (parent portion first),
                // so exactly one constructor body runs per hierarchy level.
                if let Some(delegation) = delegation {
                    self.run_delegation(this, class, delegation, &mut frame)?;
                }
                match self.exec_block(stmts, &mut frame) {
                    Ok(()) => Ok(()),
                    Err(Signal::Return(_)) => {
                        unreachable!("interpreter invariant: constructors have no return")
                    }
                    Err(Signal::Break) => {
                        unreachable!("interpreter invariant: break in constructor")
                    }
                    Err(abort @ Signal::Abort(_)) => Err(abort),
                }
            }
        }
    }

    fn run_delegation(
        &mut self,
        this: ObjId,
        class: &str,
        delegation: &TypedDelegation,
        frame: &mut Frame,
    ) -> Exec<()> {
        match delegation {
            TypedDelegation::ThisCall { actuals, .. } => {
                let vals = self.eval_args(actuals, frame)?;
                let decl = self.classes.get(class).copied().expect("class decl");
                let target = select_ctor(decl, vals.len());
                self.run_ctor(this, class, target, vals)
            }
            TypedDelegation::SuperCall { actuals, .. } => {
                let vals = self.eval_args(actuals, frame)?;
                let table = self.table;
                let parent = table
                    .get(class)
                    .and_then(|info| info.parent.as_deref())
                    .expect("interpreter invariant: super in a root class");
                let decl = self
                    .classes
                    .get(parent)
                    .copied()
                    .expect("parent class decl");
                let target = select_ctor(decl, vals.len());
                self.run_ctor(this, parent, target, vals)
            }
        }
    }

    /// The slot index of field `name` in `obj`'s runtime-class layout. Sound
    /// because LO forbids field shadowing, so the name is unique.
    fn field_slot(&self, obj: ObjId, name: &str) -> usize {
        let table = self.table;
        let class = self.heap.class_of(obj);
        table
            .get(class)
            .expect("interpreter invariant: class in table")
            .effective_fields
            .iter()
            .position(|f| f.name == name)
            .expect("interpreter invariant: field resolved by checker")
    }

    // --- variables --------------------------------------------------------

    fn read_var(&self, frame: &Frame, name: &str, binding: &BindingInfo) -> Value {
        match binding {
            BindingInfo::Local(_) | BindingInfo::Formal(_) => frame.get(name),
            BindingInfo::Field { .. } => {
                let this = frame
                    .this
                    .expect("interpreter invariant: field access needs `this`");
                let slot = self.field_slot(this, name);
                self.heap.field(this, slot)
            }
            BindingInfo::Prebound(_) => match name {
                "in" => Value::Obj(Some(self.in_id)),
                "out" => Value::Obj(Some(self.out_id)),
                "err" => Value::Obj(Some(self.err_id)),
                _ => unreachable!("interpreter invariant: unknown prebound name {name}"),
            },
        }
    }

    fn write_var(&mut self, frame: &mut Frame, name: &str, binding: &BindingInfo, value: Value) {
        match binding {
            BindingInfo::Local(_) | BindingInfo::Formal(_) => frame.set(name, value),
            BindingInfo::Field { .. } => {
                let this = frame
                    .this
                    .expect("interpreter invariant: field write needs `this`");
                let slot = self.field_slot(this, name);
                self.heap.set_field(this, slot, value);
            }
            BindingInfo::Prebound(_) => {
                unreachable!("interpreter invariant: in/out/err are not assignable")
            }
        }
    }

    // --- statements -------------------------------------------------------

    pub fn exec_block(&mut self, stmts: &[TypedStmt], frame: &mut Frame) -> Exec<()> {
        for s in stmts {
            self.exec_stmt(s, frame)?;
        }
        Ok(())
    }

    fn exec_stmt(&mut self, s: &TypedStmt, frame: &mut Frame) -> Exec<()> {
        match s {
            TypedStmt::Assign {
                target,
                binding,
                value,
                ..
            } => {
                let v = self.eval_expr(value, frame)?;
                self.write_var(frame, target, binding, v);
                Ok(())
            }
            TypedStmt::Return(e, _) => {
                let v = self.eval_expr(e, frame)?;
                Err(Signal::Return(v))
            }
            TypedStmt::If(cond, then_body, else_body, _) => match self.eval_expr(cond, frame)? {
                Value::Bool(true) => self.exec_block(then_body, frame),
                Value::Bool(false) => self.exec_block(else_body, frame),
                _ => unreachable!("interpreter invariant: if condition is not bool"),
            },
            TypedStmt::While(cond, body, _) => {
                loop {
                    match self.eval_expr(cond, frame)? {
                        Value::Bool(true) => {}
                        Value::Bool(false) => break,
                        _ => unreachable!("interpreter invariant: while condition is not bool"),
                    }
                    match self.exec_block(body, frame) {
                        Ok(()) => {}
                        Err(Signal::Break) => break,
                        Err(other) => return Err(other),
                    }
                }
                Ok(())
            }
            TypedStmt::Break(_) => Err(Signal::Break),
            TypedStmt::Empty(_) => Ok(()),
            TypedStmt::CallStmt(call) => {
                self.eval_call(call, frame)?;
                Ok(())
            }
        }
    }
}

fn ctor_arity(ctor: &TypedConstructor) -> usize {
    match ctor {
        TypedConstructor::Explicit { formals, .. } => formals.len(),
        TypedConstructor::Implicit { fields } => fields.len(),
    }
}

fn select_ctor(decl: &TypedClassDecl, arity: usize) -> &TypedConstructor {
    decl.constructors
        .iter()
        .find(|c| ctor_arity(c) == arity)
        .expect("interpreter invariant: constructor arity resolved by checker")
}

/// Integer operator semantics — total, with no undefined behavior (design §7).
/// Rust's default operators panic on overflow in debug, so every arithmetic op
/// uses a `wrapping_*` form, and the two division special cases (divide-by-zero,
/// and `INT_MIN / -1`) follow LO's RISC-V-M convention rather than trapping.
fn eval_int_binop(op: Binop, a: i32, b: i32) -> Value {
    match op {
        Binop::Add => Value::Int(a.wrapping_add(b)),
        Binop::Sub => Value::Int(a.wrapping_sub(b)),
        Binop::Mul => Value::Int(a.wrapping_mul(b)),
        Binop::Div => Value::Int(lo_div(a, b)),
        Binop::Mod => Value::Int(lo_rem(a, b)),
        Binop::Lt => Value::Bool(a < b),
        Binop::Gt => Value::Bool(a > b),
        Binop::Eq => Value::Bool(a == b),
        Binop::And | Binop::Or => {
            unreachable!("interpreter invariant: logical ops handled before value dispatch")
        }
    }
}

/// `x / 0 == -1`; otherwise truncating division, with `INT_MIN / -1 == INT_MIN`
/// (`wrapping_div`, no overflow trap).
fn lo_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        -1
    } else {
        a.wrapping_div(b)
    }
}

/// `x % 0 == x`; otherwise remainder with the dividend's sign, and
/// `INT_MIN % -1 == 0` (`wrapping_rem`).
fn lo_rem(a: i32, b: i32) -> i32 {
    if b == 0 {
        a
    } else {
        a.wrapping_rem(b)
    }
}

#[cfg(test)]
mod tests {
    use super::super::heap::DEFAULT_HEAP_LIMIT;
    use super::*;

    fn checked(src: &str) -> (TypedProgram, ClassTable) {
        let tokens = crate::lexer::tokenize(src).expect("lex failed");
        let program = crate::parser::parse_program(&tokens).expect("parse failed");
        crate::type_checker::check_program(program).expect("check failed")
    }

    fn tiny() -> (TypedProgram, ClassTable) {
        checked("class Main() { int main() { return 0; } }")
    }

    // --- expression-level helpers (Phase 1) -------------------------------

    fn eval_scalar(e: &TypedExpr) -> Value {
        let (p, t) = tiny();
        let mut it = Interp::new(&p, &t, DEFAULT_HEAP_LIMIT);
        let empty = it.heap.empty();
        let mut f = Frame::new(None, &[], vec![], &[], empty);
        it.eval_expr(e, &mut f)
            .unwrap_or_else(|_| panic!("unexpected signal"))
    }

    fn num(n: i32) -> Box<TypedExpr> {
        Box::new(TypedExpr::Num(n, 0))
    }

    fn boolean(b: bool) -> Box<TypedExpr> {
        Box::new(TypedExpr::Bool(b, 0))
    }

    fn bin(op: Binop, l: Box<TypedExpr>, r: Box<TypedExpr>) -> TypedExpr {
        TypedExpr::Binop {
            lhs: l,
            op,
            rhs: r,
            ty: Type::Int,
            line: 0,
        }
    }

    #[test]
    fn integer_arithmetic_is_total() {
        assert_eq!(
            eval_scalar(&bin(Binop::Add, num(7), num(3))),
            Value::Int(10)
        );
        // Overflow wraps rather than panicking.
        assert_eq!(
            eval_scalar(&bin(Binop::Add, num(i32::MAX), num(1))),
            Value::Int(i32::MIN)
        );
        // Division/modulo by zero do not trap.
        assert_eq!(
            eval_scalar(&bin(Binop::Div, num(7), num(0))),
            Value::Int(-1)
        );
        assert_eq!(eval_scalar(&bin(Binop::Mod, num(7), num(0))), Value::Int(7));
        // INT_MIN / -1 wraps to INT_MIN; INT_MIN % -1 is 0.
        assert_eq!(
            eval_scalar(&bin(Binop::Div, num(i32::MIN), num(-1))),
            Value::Int(i32::MIN)
        );
        assert_eq!(
            eval_scalar(&bin(Binop::Mod, num(i32::MIN), num(-1))),
            Value::Int(0)
        );
        // Truncation toward zero, remainder takes the dividend's sign.
        assert_eq!(
            eval_scalar(&bin(Binop::Div, num(-7), num(2))),
            Value::Int(-3)
        );
        assert_eq!(
            eval_scalar(&bin(Binop::Mod, num(-7), num(2))),
            Value::Int(-1)
        );
    }

    #[test]
    fn integer_comparisons() {
        assert_eq!(
            eval_scalar(&bin(Binop::Lt, num(3), num(5))),
            Value::Bool(true)
        );
        assert_eq!(
            eval_scalar(&bin(Binop::Gt, num(3), num(5))),
            Value::Bool(false)
        );
        assert_eq!(
            eval_scalar(&bin(Binop::Eq, num(4), num(4))),
            Value::Bool(true)
        );
    }

    #[test]
    fn logical_operators_short_circuit() {
        // The right operand is a variable absent from the frame: evaluating it
        // would panic on the missing slot, so reaching a bool proves it was never
        // touched.
        let missing = || {
            Box::new(TypedExpr::Var {
                name: "missing".to_string(),
                binding: BindingInfo::Local(Type::Bool),
                line: 0,
            })
        };
        assert_eq!(
            eval_scalar(&bin(Binop::And, boolean(false), missing())),
            Value::Bool(false)
        );
        assert_eq!(
            eval_scalar(&bin(Binop::Or, boolean(true), missing())),
            Value::Bool(true)
        );
        assert_eq!(
            eval_scalar(&bin(Binop::And, boolean(true), boolean(false))),
            Value::Bool(false)
        );
        assert_eq!(
            eval_scalar(&bin(Binop::Or, boolean(false), boolean(true))),
            Value::Bool(true)
        );
    }

    #[test]
    fn unary_negation_and_not() {
        let neg = TypedExpr::Unop {
            op: Unop::Neg,
            operand: num(5),
            ty: Type::Int,
            line: 0,
        };
        assert_eq!(eval_scalar(&neg), Value::Int(-5));
        let not = TypedExpr::Unop {
            op: Unop::Not,
            operand: boolean(true),
            ty: Type::Bool,
            line: 0,
        };
        assert_eq!(eval_scalar(&not), Value::Bool(false));
    }

    #[test]
    fn ternary_evaluates_one_branch() {
        let missing = Box::new(TypedExpr::Var {
            name: "missing".to_string(),
            binding: BindingInfo::Local(Type::Int),
            line: 0,
        });
        let t = TypedExpr::Ternary {
            cond: boolean(true),
            then_branch: num(1),
            else_branch: missing,
            ty: Some(Type::Int),
            line: 0,
        };
        assert_eq!(eval_scalar(&t), Value::Int(1));
    }

    #[test]
    fn string_operators() {
        let (p, t) = tiny();
        let mut it = Interp::new(&p, &t, DEFAULT_HEAP_LIMIT);
        let empty = it.heap.empty();
        let mut f = Frame::new(None, &[], vec![], &[], empty);
        let lit = |s: &str| Box::new(TypedExpr::Str(s.to_string(), 0));
        let str_of = |it: &Interp, v: Value| match v {
            Value::Str(id) => it.heap.str_value(id).to_string(),
            other => panic!("expected string, got {other:?}"),
        };

        let cat = TypedExpr::Binop {
            lhs: lit("ab"),
            op: Binop::Add,
            rhs: lit("cd"),
            ty: Type::String,
            line: 0,
        };
        let v = it.eval_expr(&cat, &mut f).unwrap();
        assert_eq!(str_of(&it, v), "abcd");

        let rep = TypedExpr::Binop {
            lhs: lit("ab"),
            op: Binop::Mul,
            rhs: num(3),
            ty: Type::String,
            line: 0,
        };
        let v = it.eval_expr(&rep, &mut f).unwrap();
        assert_eq!(str_of(&it, v), "ababab");

        let rev = TypedExpr::Unop {
            op: Unop::Neg,
            operand: lit("aé"),
            ty: Type::String,
            line: 0,
        };
        let v = it.eval_expr(&rev, &mut f).unwrap();
        assert_eq!(str_of(&it, v), "éa");

        let lt = TypedExpr::Binop {
            lhs: lit("Z"),
            op: Binop::Lt,
            rhs: lit("a"),
            ty: Type::Bool,
            line: 0,
        };
        assert_eq!(it.eval_expr(&lt, &mut f).unwrap(), Value::Bool(true));
    }

    #[test]
    fn negative_repeat_aborts_120() {
        let (p, t) = tiny();
        let mut it = Interp::new(&p, &t, DEFAULT_HEAP_LIMIT);
        let empty = it.heap.empty();
        let mut f = Frame::new(None, &[], vec![], &[], empty);
        let rep = TypedExpr::Binop {
            lhs: Box::new(TypedExpr::Str("x".to_string(), 0)),
            op: Binop::Mul,
            rhs: num(-1),
            ty: Type::String,
            line: 0,
        };
        match it.eval_expr(&rep, &mut f) {
            Err(Signal::Abort(AbortKind::RepeatNegative(-1))) => {}
            other => panic!("expected RepeatNegative(-1) abort, got {other:?}"),
        }
    }

    #[test]
    fn string_literals_are_interned_uncharged() {
        let (p, t) = tiny();
        let mut it = Interp::new(&p, &t, DEFAULT_HEAP_LIMIT);
        let empty = it.heap.empty();
        let mut f = Frame::new(None, &[], vec![], &[], empty);
        let e = TypedExpr::Str("hello".to_string(), 0);
        let v1 = it.eval_expr(&e, &mut f).unwrap();
        let v2 = it.eval_expr(&e, &mut f).unwrap();
        assert_eq!(v1, v2);
        assert_eq!(it.heap.charged(), 0);
    }

    // --- statement-level helpers (Phase 1) --------------------------------

    #[test]
    fn assign_then_read() {
        let (p, t) = tiny();
        let mut it = Interp::new(&p, &t, DEFAULT_HEAP_LIMIT);
        let empty = it.heap.empty();
        let mut f = Frame::new(None, &[], vec![], &[("x".to_string(), Type::Int)], empty);
        let stmt = TypedStmt::Assign {
            target: "x".to_string(),
            binding: BindingInfo::Local(Type::Int),
            value: TypedExpr::Num(5, 0),
            line: 0,
        };
        it.exec_block(std::slice::from_ref(&stmt), &mut f).unwrap();
        assert_eq!(f.get("x"), Value::Int(5));
    }

    #[test]
    fn while_loop_accumulates() {
        // i = 0; s = 0; while (i < 3) { s = s + i; i = i + 1 }  ==>  s == 3
        let (p, t) = tiny();
        let mut it = Interp::new(&p, &t, DEFAULT_HEAP_LIMIT);
        let empty = it.heap.empty();
        let locals = [("i".to_string(), Type::Int), ("s".to_string(), Type::Int)];
        let mut f = Frame::new(None, &[], vec![], &locals, empty);
        let var = |n: &str| {
            Box::new(TypedExpr::Var {
                name: n.to_string(),
                binding: BindingInfo::Local(Type::Int),
                line: 0,
            })
        };
        let assign = |n: &str, v: TypedExpr| TypedStmt::Assign {
            target: n.to_string(),
            binding: BindingInfo::Local(Type::Int),
            value: v,
            line: 0,
        };
        let body = vec![
            assign("s", bin(Binop::Add, var("s"), var("i"))),
            assign("i", bin(Binop::Add, var("i"), num(1))),
        ];
        let loop_stmt = TypedStmt::While(bin(Binop::Lt, var("i"), num(3)), body, 0);
        it.exec_block(std::slice::from_ref(&loop_stmt), &mut f)
            .unwrap();
        assert_eq!(f.get("s"), Value::Int(3));
        assert_eq!(f.get("i"), Value::Int(3));
    }

    #[test]
    fn break_exits_the_loop() {
        let (p, t) = tiny();
        let mut it = Interp::new(&p, &t, DEFAULT_HEAP_LIMIT);
        let empty = it.heap.empty();
        let mut f = Frame::new(None, &[], vec![], &[], empty);
        let inner = TypedStmt::If(TypedExpr::Bool(true, 0), vec![TypedStmt::Break(0)], vec![], 0);
        let loop_stmt = TypedStmt::While(TypedExpr::Bool(true, 0), vec![inner], 0);
        it.exec_block(std::slice::from_ref(&loop_stmt), &mut f)
            .unwrap();
    }

    // --- object model (Phase 2), driven through Main.main() ---------------

    /// Construct `Main` and run `main()`, returning its result (or an abort).
    fn run_main_res(src: &str) -> Result<Value, AbortKind> {
        let (p, t) = checked(src);
        let mut it = Interp::new(&p, &t, DEFAULT_HEAP_LIMIT);
        let obj = it
            .construct("Main", vec![])
            .expect("constructing Main aborted");
        let body = it.body("Main", "main");
        match it.invoke(Some(obj), body, vec![]) {
            Ok(v) => Ok(v),
            Err(Signal::Abort(a)) => Err(a),
            Err(other) => panic!("unexpected signal from main: {other:?}"),
        }
    }

    fn run_main(src: &str) -> i32 {
        match run_main_res(src) {
            Ok(Value::Int(n)) => n,
            other => panic!("expected int return from main, got {other:?}"),
        }
    }

    #[test]
    fn virtual_dispatch_uses_runtime_class() {
        let src = "
            class Animal () { int kind() { return 1; } }
            class Dog extends Animal () [ Dog() { super(); } ] { int kind() { return 2; } }
            class Main () { int main() { Animal a; a = new Dog(); return a.kind(); } }
        ";
        assert_eq!(run_main(src), 2);
    }

    #[test]
    fn super_bypasses_the_override() {
        let src = "
            class Animal () { int kind() { return 1; } }
            class Dog extends Animal () [ Dog() { super(); } ] {
                int kind() { return 2; }
                int base() { return super.kind(); }
            }
            class Main () { int main() { Dog d; d = new Dog(); return d.base(); } }
        ";
        assert_eq!(run_main(src), 1);
    }

    #[test]
    fn constructor_initializes_fields() {
        let src = "
            class Point (int x; int y;) [ Point(int a, int b) { x = a; y = b; } ] {
                int sum() { return (x + y); }
            }
            class Main () { int main() { Point p; p = new Point(3, 4); return p.sum(); } }
        ";
        assert_eq!(run_main(src), 7);
    }

    #[test]
    fn unassigned_field_keeps_its_default() {
        let src = "
            class Box (int a; int b;) [ Box(int x) { a = x; } ] { int getb() { return b; } }
            class Main () { int main() { Box box; box = new Box(5); return box.getb(); } }
        ";
        assert_eq!(run_main(src), 0);
    }

    #[test]
    fn formal_shadows_field_leaves_field_at_default() {
        // In `C(int x) { x = x; }` both `x` resolve to the formal, so the field is
        // never written and keeps its default 0 — a real language quirk, reproduced.
        let src = "
            class C (int x;) [ C(int x) { x = x; } ] { int get() { return x; } }
            class Main () { int main() { C c; c = new C(5); return c.get(); } }
        ";
        assert_eq!(run_main(src), 0);
    }

    #[test]
    fn null_receiver_aborts_102() {
        let src = "
            class Animal () { int kind() { return 1; } }
            class Main () { int main() { Animal a; return a.kind(); } }
        ";
        match run_main_res(src) {
            Err(AbortKind::NullReceiver { method }) => assert_eq!(method, "kind"),
            other => panic!("expected NullReceiver abort, got {other:?}"),
        }
    }

    // --- casts and instanceof (Phase 3) -----------------------------------

    #[test]
    fn downcast_to_actual_class_succeeds() {
        let src = "
            class Animal () { int kind() { return 1; } }
            class Dog extends Animal () [ Dog() { super(); } ] {
                int kind() { return 2; }
                int bark() { return 9; }
            }
            class Main () { int main() { Animal a; Dog d; a = new Dog(); d = ((Dog) a); return d.bark(); } }
        ";
        assert_eq!(run_main(src), 9);
    }

    #[test]
    fn downcast_to_wrong_class_aborts_101_with_both_names() {
        let src = "
            class Animal () { int kind() { return 1; } }
            class Cat extends Animal () [ Cat() { super(); } ] { int kind() { return 3; } }
            class Dog extends Animal () [ Dog() { super(); } ] { int kind() { return 2; } }
            class Main () { int main() { Animal a; Dog d; a = new Cat(); d = ((Dog) a); return d.kind(); } }
        ";
        match run_main_res(src) {
            Err(AbortKind::CastFailed { from, to }) => {
                assert_eq!(from, "Cat");
                assert_eq!(to, "Dog");
            }
            other => panic!("expected CastFailed abort, got {other:?}"),
        }
    }

    #[test]
    fn null_downcast_succeeds() {
        let src = "
            class Animal () { int k() { return 1; } }
            class Dog extends Animal () [ Dog() { super(); } ] { int k() { return 2; } }
            class Main () { int main() { Animal a; Dog d; d = ((Dog) a); return 0; } }
        ";
        assert_eq!(run_main(src), 0);
    }

    #[test]
    fn instanceof_true_on_matching_runtime_class() {
        let src = "
            class Animal () { int kind() { return 1; } }
            class Dog extends Animal () [ Dog() { super(); } ] { int kind() { return 2; } }
            class Main () { int main() { Animal a; a = new Dog(); return ((a instanceof Dog) ? 1 : 0); } }
        ";
        assert_eq!(run_main(src), 1);
    }

    #[test]
    fn instanceof_null_is_false() {
        let src = "
            class Foo () { int x() { return 1; } }
            class Main () { int main() { Foo f; return ((f instanceof Foo) ? 1 : 0); } }
        ";
        assert_eq!(run_main(src), 0);
    }

    // --- I/O (Phase 4) ----------------------------------------------------

    use super::super::io::SharedBuf;
    use std::io::Cursor;

    /// Run `Main.main()` with the given stdin, returning (result, stdout, stderr).
    fn run_with_io(src: &str, input: &str) -> (Result<Value, AbortKind>, String, String) {
        let (p, t) = checked(src);
        let out = SharedBuf::new();
        let err = SharedBuf::new();
        let io = Io::new(
            Box::new(Cursor::new(input.as_bytes().to_vec())),
            Box::new(out.clone()),
            Box::new(err.clone()),
        );
        let mut it = Interp::with_io(&p, &t, DEFAULT_HEAP_LIMIT, io);
        let obj = it
            .construct("Main", vec![])
            .expect("constructing Main aborted");
        let body = it.body("Main", "main");
        let res = match it.invoke(Some(obj), body, vec![]) {
            Ok(v) => Ok(v),
            Err(Signal::Abort(a)) => Err(a),
            Err(other) => panic!("unexpected signal: {other:?}"),
        };
        (res, out.contents(), err.contents())
    }

    #[test]
    fn print_int_to_stdout() {
        let src = "class Main () { int main() { out.print_int(48); return 0; } }";
        let (res, out, _err) = run_with_io(src, "");
        assert_eq!(res, Ok(Value::Int(0)));
        assert_eq!(out, "48");
    }

    #[test]
    fn out_and_err_route_to_different_streams() {
        let src = "class Main () { int main() { out.print_int(1); err.print_int(2); return 0; } }";
        let (_res, out, err) = run_with_io(src, "");
        assert_eq!(out, "1");
        assert_eq!(err, "2");
    }

    #[test]
    fn print_string_then_println() {
        let src =
            "class Main () { int main() { out.print_string(\"hi\"); out.println(); return 0; } }";
        let (_res, out, _err) = run_with_io(src, "");
        assert_eq!(out, "hi\n");
    }

    #[test]
    fn read_int_returns_value() {
        let src = "class Main () { int main() { int n; n = in.read_int(); return n; } }";
        let (res, _out, _err) = run_with_io(src, "42\n");
        assert_eq!(res, Ok(Value::Int(42)));
    }

    #[test]
    fn read_int_eof_aborts_111() {
        let src = "class Main () { int main() { int n; n = in.read_int(); return n; } }";
        let (res, _out, _err) = run_with_io(src, "");
        assert_eq!(res, Err(AbortKind::ReadIntEof));
    }

    #[test]
    fn read_bool_invalid_aborts_112() {
        let src = "class Main () { int main() { bool b; b = in.read_bool(); return 0; } }";
        let (res, _out, _err) = run_with_io(src, "yes\n");
        assert_eq!(res, Err(AbortKind::ReadBoolInvalid));
    }

    #[test]
    fn read_string_reads_one_line() {
        let src = "class Main () { int main() { out.print_string(in.read_string()); return 0; } }";
        let (_res, out, _err) = run_with_io(src, "hello\nworld\n");
        assert_eq!(out, "hello");
    }

    #[test]
    fn eof_reflects_remaining_input() {
        let src = "class Main () { int main() { return ((in.eof()) ? 1 : 0); } }";
        let (empty, _o, _e) = run_with_io(src, "");
        assert_eq!(empty, Ok(Value::Int(1)));
        let (nonempty, _o, _e) = run_with_io(src, "x");
        assert_eq!(nonempty, Ok(Value::Int(0)));
    }

    #[test]
    fn null_output_receiver_aborts_102_and_prints_nothing() {
        // `o` is a null Output local; dispatching print_int on it must abort, not
        // silently route to stdout.
        let src = "class Main () { int main() { Output o; o.print_int(5); return 0; } }";
        let (res, out, _err) = run_with_io(src, "");
        assert_eq!(
            res,
            Err(AbortKind::NullReceiver {
                method: "print_int".to_string()
            })
        );
        assert_eq!(out, "");
    }

    #[test]
    fn null_input_receiver_aborts_102() {
        let src = "class Main () { int main() { Input i; int n; n = i.read_int(); return n; } }";
        let (res, _out, _err) = run_with_io(src, "5\n");
        assert_eq!(
            res,
            Err(AbortKind::NullReceiver {
                method: "read_int".to_string()
            })
        );
    }

    #[test]
    fn out_and_err_still_route_after_null_check() {
        // Regression guard: the null check must not disturb non-null routing.
        let src = "class Main () { int main() { out.print_int(1); err.print_int(2); return 0; } }";
        let (res, out, err) = run_with_io(src, "");
        assert_eq!(res, Ok(Value::Int(0)));
        assert_eq!(out, "1");
        assert_eq!(err, "2");
    }
}
