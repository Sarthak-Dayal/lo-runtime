use crate::ast::{Binop, Type, Unop};
use crate::type_checker::{
    BindingInfo, CastDirection, MethodResolution, TypedExpr, TypedMethodCall, TypedObjName,
};

use super::function::{Function, Value};
use super::{class_symbol, ctor_symbol, method_symbol};

impl Function<'_, '_> {
    // P20-P32: the typed AST has already erased P28's parentheses and P32's wrapper.
    // Each handler returns an owned scratch local; its caller releases it after use.
    pub(super) fn expression(&mut self, expr: &TypedExpr) -> Value {
        match expr {
            TypedExpr::Num(n, _) => self.p40_number(*n),
            TypedExpr::Bool(b, _) => self.p41_p42_boolean(*b),
            TypedExpr::Str(s, _) => self.p43_string(s),
            TypedExpr::Null(_) => self.p21_null(),
            TypedExpr::This(class, _) => self.p20_this(class),
            TypedExpr::Var { name, binding, .. } => self.p31_variable(name, binding),
            TypedExpr::New { class, actuals, .. } => self.p22_new(class, actuals),
            TypedExpr::Call(call) => self.p23_call(call).expect("checked value call"),
            TypedExpr::Ternary {
                cond,
                then_branch,
                else_branch,
                ..
            } => self.p25_ternary(cond, then_branch, else_branch, &expr_type(expr)),
            TypedExpr::Binop { lhs, op, rhs, .. } => self.p26_binary(lhs, *op, rhs),
            TypedExpr::Unop { op, operand, .. } => self.p27_unary(*op, operand),
            TypedExpr::Cast {
                target,
                operand,
                direction,
                ..
            } => self.p29_cast(target, operand, *direction),
            TypedExpr::InstanceOf { operand, class, .. } => self.p30_instanceof(operand, class),
        }
    }

    // P20: this -> receiver parameter 0.
    fn p20_this(&mut self, class: &str) -> Value {
        self.copy(0, &Type::Class(class.into()))
    }

    // P21: null -> zero; it does not denote a heap allocation.
    fn p21_null(&mut self) -> Value {
        self.constant(0)
    }

    // P22: evaluate arguments before allocating or running the constructor.
    fn p22_new(&mut self, class: &str, actuals: &[TypedExpr]) -> Value {
        self.ins("# P22: new");
        let args = self.p10_actuals(actuals);
        let object = self.allocate(class, &args);
        self.release_all(&args);
        object
    }

    pub(super) fn allocate(&mut self, class: &str, actuals: &[Value]) -> Value {
        let descriptor = self.constant(class_symbol(class));
        let object = self
            .call("lo_alloc", &[descriptor], &Type::Class(class.into()))
            .unwrap();
        let string_offsets: Vec<_> = self
            .module
            .classes
            .get(class)
            .unwrap()
            .effective_fields
            .iter()
            .enumerate()
            .filter(|(_, field)| field.ty == Type::String)
            .map(|(i, _)| 12 + 4 * i)
            .collect();
        for offset in string_offsets {
            let empty = self.constant("LO_EMPTY_STRING");
            self.store_field(object, offset, empty, &Type::String);
        }
        let args: Vec<_> = std::iter::once(object)
            .chain(actuals.iter().copied())
            .collect();
        self.call(&ctor_symbol(class, actuals.len()), &args, &Type::Void);
        object
    }

    // P23/P19: receiver, arguments, null guard, then direct or virtual dispatch.
    pub(super) fn p23_call(&mut self, call: &TypedMethodCall) -> Option<Value> {
        self.ins("# P23/P19: method call");
        let receiver = self.p46_p49_receiver(&call.obj_name);
        let args = self.p10_actuals(&call.actuals);
        self.null_guard(receiver, &call.method_name);
        let result = match &call.resolution {
            MethodResolution::Virtual { static_class } => {
                let slot =
                    self.module.classes.get(static_class).unwrap().method_slot[&call.method_name];
                self.virtual_call(receiver, &args, slot, &call.return_type)
            }
            MethodResolution::Super { declaring_class } => {
                let args: Vec<_> = std::iter::once(receiver)
                    .chain(args.iter().copied())
                    .collect();
                self.call(
                    &method_symbol(declaring_class, &call.method_name),
                    &args,
                    &call.return_type,
                )
            }
            MethodResolution::Io { op } => {
                use crate::ast::IoOp;
                let class = match op {
                    IoOp::ReadInt | IoOp::ReadBool | IoOp::ReadString | IoOp::Eof => "Input",
                    _ => "Output",
                };
                let args: Vec<_> = std::iter::once(receiver)
                    .chain(args.iter().copied())
                    .collect();
                self.call(
                    &method_symbol(class, &call.method_name),
                    &args,
                    &call.return_type,
                )
            }
        };
        self.release(receiver);
        self.release_all(&args);
        result
    }

    fn null_guard(&mut self, receiver: Value, method: &str) {
        self.get(receiver);
        self.ins("i32.eqz");
        self.open_if(false);
        let symbol = self.module.bytes(method.as_bytes());
        let bytes = self.constant(symbol);
        let length = self.constant(method.len());
        self.call("lo_abort_null_receiver", &[bytes, length], &Type::Void);
        self.ins("unreachable");
        self.end_if();
    }

    // P10: evaluate once, left to right; earlier reference arguments stay rooted.
    pub(super) fn p10_actuals(&mut self, actuals: &[TypedExpr]) -> Vec<Value> {
        actuals.iter().map(|expr| self.expression(expr)).collect()
    }

    // P46-P49: variable, this, super, or one evaluated receiver expression.
    fn p46_p49_receiver(&mut self, receiver: &TypedObjName) -> Value {
        match receiver {
            TypedObjName::Var { name, binding, .. } => self.p31_variable(name, binding),
            TypedObjName::This(class, _) => self.p20_this(class),
            TypedObjName::Super(_) => self.p20_this(&self.class.clone()),
            TypedObjName::Computed(expr, _) => self.expression(expr),
        }
    }

    // P25: exactly one value branch executes.
    fn p25_ternary(
        &mut self,
        condition: &TypedExpr,
        yes: &TypedExpr,
        no: &TypedExpr,
        ty: &Type,
    ) -> Value {
        let result = self.temporary(ty);
        let condition = self.expression(condition);
        self.get(condition);
        self.release(condition);
        self.open_if(false);
        let value = self.expression(yes);
        self.get(value);
        self.set(result);
        self.release(value);
        self.ins("else");
        let value = self.expression(no);
        self.get(value);
        self.set(result);
        self.release(value);
        self.end_if();
        result
    }

    // P26/P33: checked operand types select integer, boolean, or String operations.
    fn p26_binary(&mut self, lhs: &TypedExpr, op: Binop, rhs: &TypedExpr) -> Value {
        let left = self.expression(lhs);
        if matches!(op, Binop::And | Binop::Or) {
            return self.short_circuit(left, op, rhs);
        }
        let right = self.expression(rhs);
        let result = if expr_type(lhs) == Type::String {
            let (name, result_type) = match op {
                Binop::Add => ("lo_string_concat", Type::String),
                Binop::Mul => ("lo_string_repeat", Type::String),
                Binop::Lt | Binop::Gt | Binop::Eq => ("lo_string_compare", Type::Int),
                _ => unreachable!("checked String operator"),
            };
            let value = self.call(name, &[left, right], &result_type).unwrap();
            if matches!(op, Binop::Lt | Binop::Gt | Binop::Eq) {
                self.get(value);
                self.ins("i32.const 0");
                self.ins(integer_instruction(op));
                self.set(value);
            }
            value
        } else {
            let result = self.temporary(&Type::Int);
            if matches!(op, Binop::Div | Binop::Mod) {
                self.total_division(left, op, right);
            } else {
                self.get(left);
                self.get(right);
                self.ins(integer_instruction(op));
            }
            self.set(result);
            result
        };
        self.release(left);
        self.release(right);
        result
    }

    fn short_circuit(&mut self, left: Value, op: Binop, rhs: &TypedExpr) -> Value {
        let result = self.temporary(&Type::Bool);
        self.get(left);
        self.open_if(false);
        if op == Binop::Or {
            self.ins("i32.const 1");
            self.set(result);
        } else {
            let right = self.expression(rhs);
            self.get(right);
            self.set(result);
            self.release(right);
        }
        self.ins("else");
        if op == Binop::And {
            self.ins("i32.const 0");
            self.set(result);
        } else {
            let right = self.expression(rhs);
            self.get(right);
            self.set(result);
            self.release(right);
        }
        self.end_if();
        self.release(left);
        result
    }

    fn total_division(&mut self, left: Value, op: Binop, right: Value) {
        self.get(right);
        self.ins("i32.eqz");
        self.open_if(true);
        if op == Binop::Div {
            self.ins("i32.const -1");
        } else {
            self.get(left);
        }
        self.ins("else");
        if op == Binop::Div {
            self.get(left);
            self.ins("i32.const -2147483648");
            self.ins("i32.eq");
            self.get(right);
            self.ins("i32.const -1");
            self.ins("i32.eq");
            self.ins("i32.and");
            self.open_if(true);
            self.ins("i32.const -2147483648");
            self.ins("else");
            self.get(left);
            self.get(right);
            self.ins("i32.div_s");
            self.end_if();
        } else {
            self.get(left);
            self.get(right);
            self.ins("i32.rem_s");
        }
        self.end_if();
    }

    // P27/P34: boolean not, wrapping integer negation, or String reversal.
    fn p27_unary(&mut self, op: Unop, operand: &TypedExpr) -> Value {
        let value = self.expression(operand);
        if expr_type(operand) == Type::String {
            let result = self
                .call("lo_string_reverse", &[value], &Type::String)
                .unwrap();
            self.release(value);
            result
        } else {
            if op == Unop::Neg {
                self.ins("i32.const 0");
            }
            self.get(value);
            self.ins(if op == Unop::Not {
                "i32.eqz"
            } else {
                "i32.sub"
            });
            self.set(value);
            value
        }
    }

    // P29: only a checked downcast calls the runtime.
    fn p29_cast(&mut self, target: &Type, operand: &TypedExpr, direction: CastDirection) -> Value {
        let value = self.expression(operand);
        if direction != CastDirection::Downcast {
            return value;
        }
        let Type::Class(class) = target else {
            unreachable!("checked cast target")
        };
        let descriptor = self.constant(class_symbol(class));
        let result = self
            .call("lo_cast_check", &[value, descriptor], target)
            .unwrap();
        self.release(value);
        result
    }

    // P30: the runtime walks descriptor parents; null produces false.
    fn p30_instanceof(&mut self, operand: &TypedExpr, class: &str) -> Value {
        let value = self.expression(operand);
        let descriptor = self.constant(class_symbol(class));
        let result = self
            .call("lo_instanceof", &[value, descriptor], &Type::Bool)
            .unwrap();
        self.release(value);
        result
    }

    // P31/P50: read the binding resolved by the checker.
    fn p31_variable(&mut self, name: &str, binding: &BindingInfo) -> Value {
        let result = self.temporary(binding.ty());
        match binding {
            BindingInfo::Local(_) | BindingInfo::Formal(_) => self.get(self.bindings[name]),
            BindingInfo::Field { owner, .. } => {
                self.get(0);
                self.ins(format!(
                    "i32.load {}",
                    self.module.field_offset(owner, name)
                ));
            }
            BindingInfo::Prebound(_) => {
                self.ins(format!("i32.const lo_binding_{name}"));
                self.ins("i32.load 0 # address of the persistent root slot");
                self.ins("i32.load 0 # current object address");
            }
        }
        self.set(result);
        result
    }

    // P40/P51: lexer-checked i32 value; llvm-mc writes its signed LEB128 encoding.
    fn p40_number(&mut self, number: i32) -> Value {
        self.constant(number)
    }

    // P41/P42: true = 1, false = 0.
    fn p41_p42_boolean(&mut self, value: bool) -> Value {
        self.constant(i32::from(value))
    }

    // P43/P52: decoded UTF-8 bytes are static; the resulting String is managed.
    fn p43_string(&mut self, string: &str) -> Value {
        if string.is_empty() {
            let result = self.temporary(&Type::String);
            self.ins("i32.const LO_EMPTY_STRING");
            self.set(result);
            return result;
        }
        let symbol = self.module.bytes(string.as_bytes());
        let bytes = self.constant(symbol);
        let length = self.constant(string.len());
        self.call("lo_string_new", &[bytes, length], &Type::String)
            .unwrap()
    }
}

fn integer_instruction(op: Binop) -> &'static str {
    match op {
        Binop::Add => "i32.add",
        Binop::Sub => "i32.sub",
        Binop::Mul => "i32.mul",
        Binop::Lt => "i32.lt_s",
        Binop::Gt => "i32.gt_s",
        Binop::Eq => "i32.eq",
        _ => unreachable!("operator has a separate lowering"),
    }
}

fn expr_type(expr: &TypedExpr) -> Type {
    match expr {
        TypedExpr::Num(..) => Type::Int,
        TypedExpr::Bool(..) | TypedExpr::InstanceOf { .. } => Type::Bool,
        TypedExpr::Str(..) => Type::String,
        TypedExpr::Null(_) | TypedExpr::Ternary { ty: None, .. } => Type::Class("<null>".into()),
        TypedExpr::This(class, _) | TypedExpr::New { class, .. } => Type::Class(class.clone()),
        TypedExpr::Var { binding, .. } => binding.ty().clone(),
        TypedExpr::Call(call) => call.return_type.clone(),
        TypedExpr::Ternary { ty: Some(ty), .. }
        | TypedExpr::Binop { ty, .. }
        | TypedExpr::Unop { ty, .. } => ty.clone(),
        TypedExpr::Cast { target, .. } => target.clone(),
    }
}
