use crate::ast::Type;
use crate::type_checker::{BindingInfo, TypedExpr, TypedMethodCall, TypedStmt};

use super::function::{Control, Function, Value};
use super::is_reference;

impl Function<'_, '_> {
    // P12: statements in source order; block declarations were hoisted by the parser.
    pub(super) fn p12_block(&mut self, statements: &[TypedStmt]) {
        for statement in statements {
            match statement {
                TypedStmt::Return(expr, _) => self.p13_return(expr),
                TypedStmt::If(condition, yes, no, _) => self.p14_if(condition, yes, no),
                TypedStmt::While(condition, body, _) => self.p15_while(condition, body),
                TypedStmt::Break(_) => self.p16_break(),
                TypedStmt::Assign {
                    target,
                    binding,
                    value,
                    ..
                } => self.p17_assign(target, binding, value),
                TypedStmt::Empty(_) => self.p18_empty(),
                TypedStmt::CallStmt(call) => self.p19_call_statement(call),
            }
        }
    }

    // P13: save the value and branch through LEAVE.
    fn p13_return(&mut self, expr: &TypedExpr) {
        self.ins("# P13: return");
        let value = self.expression(expr);
        self.return_saved(value);
    }

    // P14: if/else contributes one level to every enclosed branch depth.
    fn p14_if(&mut self, condition: &TypedExpr, yes: &[TypedStmt], no: &[TypedStmt]) {
        self.ins("# P14: if/else");
        let condition = self.expression(condition);
        self.get(condition);
        self.release(condition);
        self.open_if(false);
        self.p12_block(yes);
        self.ins("else");
        self.p12_block(no);
        self.end_if();
    }

    // P15: an outer exit block encloses an inner loop head.
    fn p15_while(&mut self, condition: &TypedExpr, body: &[TypedStmt]) {
        self.ins("# P15: while");
        self.ins("block");
        self.controls.push(Control::LoopExit);
        self.ins("loop");
        self.controls.push(Control::LoopHead);
        let condition = self.expression(condition);
        self.get(condition);
        self.release(condition);
        self.ins("i32.eqz");
        self.branch(Control::LoopExit, true);
        self.p12_block(body);
        // A constant-true conditional branch is an unconditional back edge at
        // runtime, while keeping llvm-mc's stack inference valid for nested
        // loops in result-returning functions.
        self.ins("i32.const 1");
        self.branch(Control::LoopHead, true);
        self.controls.pop();
        self.ins("end_loop");
        self.controls.pop();
        self.ins("end_block");
    }

    // P16: exit the nearest loop, counting all intervening blocks and ifs.
    fn p16_break(&mut self) {
        self.ins("# P16: break");
        self.branch(Control::LoopExit, false);
    }

    // P17: evaluate the RHS before accessing a receiver that GC may move.
    fn p17_assign(&mut self, name: &str, binding: &BindingInfo, expr: &TypedExpr) {
        self.ins("# P17: assignment");
        let value = self.expression(expr);
        match binding {
            BindingInfo::Local(_) | BindingInfo::Formal(_) => {
                self.get(value);
                self.set(self.bindings[name]);
            }
            BindingInfo::Field { owner, ty } => {
                let offset = self.module.field_offset(owner, name);
                self.store_field(0, offset, value, ty);
            }
            BindingInfo::Prebound(_) => {
                self.ins(format!("i32.const lo_binding_{name}"));
                self.ins("i32.load 0");
                self.get(value);
                self.ins("i32.store 0");
            }
        }
        self.release(value);
    }

    pub(super) fn store_field(&mut self, receiver: Value, offset: usize, value: Value, ty: &Type) {
        if is_reference(ty) {
            let offset = self.constant(offset);
            // The barrier performs the store, including stores of null.
            self.call(
                "lo_gc_write_barrier",
                &[receiver, offset, value],
                &Type::Void,
            );
        } else {
            self.get(receiver);
            self.get(value);
            self.ins(format!("i32.store {offset}"));
        }
    }

    // P18: an empty statement emits nothing.
    fn p18_empty(&mut self) {}

    // P19: the checker guarantees a void call, so there is no result to discard.
    fn p19_call_statement(&mut self, call: &TypedMethodCall) {
        assert!(self.p23_call(call).is_none());
    }
}
