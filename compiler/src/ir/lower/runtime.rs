use crate::ast::{Binop, IoOp};
use crate::type_checker::{
    BindingInfo, MethodResolution, TypedExpr, TypedMethodCall, TypedObjName,
};

use super::{class_symbol, ctor_symbol, length, method_symbol, value_type, Builder};
use crate::ir::*;

impl Builder<'_, '_> {
    pub(super) fn load(&mut self, base: Operand, offset: i32, ty: IrType, line: u32) -> Operand {
        let dst = self.value(ty);
        self.emit(InstructionKind::Load { dst, base, offset }, line);
        Operand::Value(dst)
    }

    pub(super) fn call(
        &mut self,
        symbol: SymbolId,
        args: Vec<Operand>,
        line: u32,
    ) -> Option<Operand> {
        let sig = self.ctx.signature(symbol);
        self.invoke(CallTarget::Direct(symbol), sig, args, line)
    }

    fn invoke(
        &mut self,
        target: CallTarget,
        signature: SignatureId,
        args: Vec<Operand>,
        line: u32,
    ) -> Option<Operand> {
        let dst = self.ctx.ir.signatures[signature.0]
            .result
            .map(|ty| self.value(ty));
        self.emit(InstructionKind::Call { dst, target, args }, line);
        dst.map(Operand::Value)
    }

    pub(super) fn runtime(
        &mut self,
        name: &str,
        args: Vec<Operand>,
        line: u32,
    ) -> Result<Option<Operand>, String> {
        let symbol = self.ctx.symbol(name)?;
        Ok(self.call(symbol, args, line))
    }

    pub(super) fn runtime_value(
        &mut self,
        name: &str,
        args: Vec<Operand>,
        line: u32,
    ) -> Result<Operand, String> {
        self.runtime(name, args, line)?
            .ok_or_else(|| format!("IR: {name} has no result"))
    }

    pub(super) fn read_binding(
        &mut self,
        name: &str,
        binding: &BindingInfo,
        line: u32,
    ) -> Result<Operand, String> {
        let ty = value_type(binding.ty())?;
        Ok(match binding {
            BindingInfo::Local(_) | BindingInfo::Formal(_) => {
                // Snapshot the value before evaluating later operands/arguments.
                let src = Operand::Value(self.binding(name, binding, line)?);
                let dst = self.value(ty);
                self.emit(InstructionKind::Copy { dst, src }, line);
                Operand::Value(dst)
            }
            BindingInfo::Field { owner, .. } => {
                let offset = self.ctx.field_offset(owner, name)?;
                self.load(self.receiver(), offset, ty, line)
            }
            BindingInfo::Prebound(_) => {
                let frame = self.ctx.symbol("lo_bindings")?;
                let offset = self.ctx.target.binding_offset(name)?;
                self.load(Operand::Symbol(frame), offset, IrType::Ref, line)
            }
        })
    }

    pub(super) fn assign(
        &mut self,
        name: &str,
        binding: &BindingInfo,
        value: Operand,
        line: u32,
    ) -> Result<(), String> {
        match binding {
            BindingInfo::Local(_) | BindingInfo::Formal(_) => {
                let dst = self.binding(name, binding, line)?;
                self.emit(InstructionKind::Copy { dst, src: value }, line);
            }
            BindingInfo::Field { owner, ty } => {
                let offset = self.ctx.field_offset(owner, name)?;
                self.store_field(self.receiver(), offset, value, value_type(ty)?, line)?;
            }
            BindingInfo::Prebound(_) => {
                self.store_binding(name, value, line)?;
            }
        }
        Ok(())
    }

    pub(super) fn store_binding(
        &mut self,
        name: &str,
        value: Operand,
        line: u32,
    ) -> Result<(), String> {
        let frame = self.ctx.symbol("lo_bindings")?;
        let offset = self.ctx.target.binding_offset(name)?;
        self.emit(
            InstructionKind::Store {
                base: Operand::Symbol(frame),
                offset,
                value,
                ty: IrType::Ref,
            },
            line,
        );
        Ok(())
    }

    pub(super) fn store_field(
        &mut self,
        object: Operand,
        offset: i32,
        value: Operand,
        ty: IrType,
        line: u32,
    ) -> Result<(), String> {
        // Static defaults cannot move; other Ref writes (including null) use
        // the barrier, which performs the store rather than merely notifying GC.
        let static_ref = matches!(value, Operand::Symbol(id) if matches!(self.ctx.ir.symbols[id.0].kind, SymbolKind::StaticRef));
        if ty == IrType::Ref && !static_ref {
            self.runtime(
                "lo_gc_write_barrier",
                vec![object, Operand::Int(offset), value],
                line,
            )?;
        } else {
            self.emit(
                InstructionKind::Store {
                    base: object,
                    offset,
                    value,
                    ty,
                },
                line,
            );
        }
        Ok(())
    }

    pub(super) fn actuals(&mut self, actuals: &[TypedExpr]) -> Result<Vec<Operand>, String> {
        actuals.iter().map(|e| self.expression(e)).collect()
    }

    pub(super) fn allocate(
        &mut self,
        class: &str,
        actuals: Vec<Operand>,
        line: u32,
    ) -> Result<Operand, String> {
        let descriptor = self.ctx.symbol(&class_symbol(class))?;
        let object = self.runtime_value("lo_alloc", vec![Operand::Symbol(descriptor)], line)?;
        let fields = self
            .ctx
            .classes
            .get(class)
            .ok_or("IR: missing allocation class")?
            .effective_fields
            .clone();
        for field in fields.iter().filter(|f| f.ty == crate::ast::Type::String) {
            let offset = self.ctx.field_offset(class, &field.name)?;
            let empty = Operand::Symbol(self.ctx.symbol("LO_EMPTY_STRING")?);
            self.store_field(object, offset, empty, IrType::Ref, line)?;
        }
        let ctor = self.ctx.symbol(&ctor_symbol(class, actuals.len()))?;
        let mut args = vec![object];
        args.extend(actuals);
        self.call(ctor, args, line);
        Ok(object)
    }

    fn null_guard(&mut self, receiver: Operand, method: &str, line: u32) -> Result<(), String> {
        let condition = self.value(IrType::Bool);
        self.emit(
            InstructionKind::Binary {
                dst: condition,
                op: BinaryOp::Eq,
                lhs: receiver,
                rhs: Operand::Null,
            },
            line,
        );
        let abort = self.block();
        let next = self.block();
        self.finish(Terminator::Branch {
            condition: Operand::Value(condition),
            then_block: abort,
            else_block: next,
        });
        self.current = Some(abort);
        let bytes = self.ctx.bytes(method.as_bytes())?;
        let target = self.ctx.symbol("lo_abort_null_receiver")?;
        self.finish(Terminator::Abort {
            target,
            args: vec![Operand::Symbol(bytes), Operand::Int(length(method.len())?)],
        });
        self.current = Some(next);
        Ok(())
    }

    pub(super) fn dispatch(
        &mut self,
        receiver: Operand,
        class: &str,
        method: &str,
        actuals: Vec<Operand>,
        line: u32,
    ) -> Result<Option<Operand>, String> {
        self.null_guard(receiver, method, line)?;
        let info = self
            .ctx
            .classes
            .get(class)
            .ok_or("IR: missing dispatch class")?;
        let slot = *info
            .method_slot
            .get(method)
            .ok_or("IR: missing method slot")?;
        let entry = info
            .effective_methods
            .get(method)
            .ok_or("IR: missing effective method")?;
        let symbol = self.ctx.symbol(&method_symbol(&entry.owner, method))?;
        let signature = self.ctx.signature(symbol);
        let descriptor = self.load(receiver, 0, IrType::Ptr, line);
        let table = self.load(
            descriptor,
            self.ctx.target.descriptor_vtable_offset(),
            IrType::Ptr,
            line,
        );
        let byte_offset = slot
            .checked_mul(self.ctx.target.pointer_bytes() as usize)
            .ok_or("IR: vtable offset overflow")?;
        let pointer = self.load(
            table,
            length(byte_offset)?,
            IrType::CodePtr(signature),
            line,
        );
        let mut args = vec![receiver];
        args.extend(actuals);
        Ok(self.invoke(CallTarget::Indirect(pointer), signature, args, line))
    }

    pub(super) fn method_call(
        &mut self,
        call: &TypedMethodCall,
    ) -> Result<Option<Operand>, String> {
        let receiver = match &call.obj_name {
            TypedObjName::This(..) | TypedObjName::Super(_) => self.receiver(),
            TypedObjName::Var {
                name,
                binding,
                line,
            } => self.read_binding(name, binding, *line)?,
            TypedObjName::Computed(expr, _) => self.expression(expr)?,
        };
        let actuals = self.actuals(&call.actuals)?;
        match &call.resolution {
            MethodResolution::Virtual { static_class } => self.dispatch(
                receiver,
                static_class,
                &call.method_name,
                actuals,
                call.line,
            ),
            MethodResolution::Super { declaring_class } => {
                self.null_guard(receiver, &call.method_name, call.line)?;
                let symbol = self
                    .ctx
                    .symbol(&method_symbol(declaring_class, &call.method_name))?;
                let mut args = vec![receiver];
                args.extend(actuals);
                Ok(self.call(symbol, args, call.line))
            }
            MethodResolution::Io { op } => {
                self.null_guard(receiver, &call.method_name, call.line)?;
                let class = if matches!(
                    op,
                    IoOp::ReadInt | IoOp::ReadBool | IoOp::ReadString | IoOp::Eof
                ) {
                    "Input"
                } else {
                    "Output"
                };
                let symbol = self.ctx.symbol(&method_symbol(class, &call.method_name))?;
                let mut args = vec![receiver];
                args.extend(actuals);
                Ok(self.call(symbol, args, call.line))
            }
        }
    }

    pub(super) fn io_wrapper(&mut self, op: &IoOp, line: u32) -> Result<(), String> {
        let (name, output) = match op {
            IoOp::ReadInt => ("lo_read_int", false),
            IoOp::ReadBool => ("lo_read_bool", false),
            IoOp::ReadString => ("lo_read_string", false),
            IoOp::Eof => ("lo_eof", false),
            IoOp::PrintInt => ("lo_print_int", true),
            IoOp::PrintBool => ("lo_print_bool", true),
            IoOp::PrintString => ("lo_print_string", true),
            IoOp::Println => ("lo_println", true),
        };
        let mut args: Vec<_> = self
            .function
            .params
            .iter()
            .skip(1)
            .copied()
            .map(Operand::Value)
            .collect();
        if output {
            args.push(self.load(
                self.receiver(),
                self.ctx.target.object_header_bytes() as i32,
                IrType::Int32,
                line,
            ));
        }
        let result = self.runtime(name, args, line)?;
        self.finish(Terminator::Return(result));
        Ok(())
    }

    pub(super) fn string_binary(
        &mut self,
        op: Binop,
        lhs: Operand,
        rhs: Operand,
        line: u32,
    ) -> Result<Operand, String> {
        let name = match op {
            Binop::Add => "lo_string_concat",
            Binop::Mul => "lo_string_repeat",
            Binop::Eq | Binop::Lt | Binop::Gt => "lo_string_compare",
            _ => return Err(format!("IR line {line}: unsupported String operator")),
        };
        let result = self.runtime_value(name, vec![lhs, rhs], line)?;
        if matches!(op, Binop::Eq | Binop::Lt | Binop::Gt) {
            let dst = self.value(IrType::Bool);
            let op = match op {
                Binop::Eq => BinaryOp::Eq,
                Binop::Lt => BinaryOp::Lt,
                _ => BinaryOp::Gt,
            };
            self.emit(
                InstructionKind::Binary {
                    dst,
                    op,
                    lhs: result,
                    rhs: Operand::Int(0),
                },
                line,
            );
            Ok(Operand::Value(dst))
        } else {
            Ok(result)
        }
    }
}
