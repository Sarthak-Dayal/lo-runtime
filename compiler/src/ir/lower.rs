use std::collections::HashMap;

use crate::ast::{Binop, Type, Unop};
use crate::type_checker::{BindingInfo, TypedExpr, TypedMethodBody, TypedMethodDecl, TypedStmt};

use super::*;

// The caller registers the method symbol/signature, with the receiver first.
pub fn lower_method(symbol: SymbolId, method: &TypedMethodDecl) -> Result<FunctionIr, String> {
    let TypedMethodBody::UserDefined { locals, stmts } = &method.body else {
        return Err(format!(
            "IR line {}: I/O lowering is not implemented",
            method.line
        ));
    };
    if method.return_type != Type::Void {
        value_type(&method.return_type)?;
    }
    let mut builder = Builder {
        function: FunctionIr {
            symbol,
            params: vec![VirtualRegId(0)],
            register_types: vec![IrType::Ref],
            register_names: vec![Some("this".into())],
            root_slots: 0,
            blocks: vec![],
            entry: BlockId(0),
        },
        blocks: vec![],
        current: None,
        bindings: HashMap::new(),
        loop_exits: vec![],
    };
    builder.current = Some(builder.block());
    for (name, ty) in &method.formals {
        let id = builder.value(value_type(ty)?);
        builder.function.register_names[id.0] = Some(name.clone());
        builder.function.params.push(id);
        builder.bindings.insert(name.clone(), id);
    }
    for (name, ty) in locals {
        let id = builder.value(value_type(ty)?);
        builder.function.register_names[id.0] = Some(name.clone());
        builder.bindings.insert(name.clone(), id);
        let src = match ty {
            Type::Int => Operand::Int(0),
            Type::Bool => Operand::Bool(false),
            Type::Class(_) => Operand::Null,
            _ => return Err("IR: unsupported local type".into()),
        };
        builder.emit(InstructionKind::Copy { dst: id, src }, method.line);
    }
    builder.statements(stmts)?;
    if builder.current.is_some() {
        if method.return_type != Type::Void {
            return Err(format!(
                "IR line {}: non-void method falls through",
                method.line
            ));
        }
        builder.finish(Terminator::Return(None));
    }
    builder.function.blocks = builder
        .blocks
        .into_iter()
        .map(|(instructions, terminator)| {
            Ok(BasicBlock {
                instructions,
                terminator: terminator.ok_or("IR: unterminated block")?,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(builder.function)
}

fn value_type(ty: &Type) -> Result<IrType, String> {
    match ty {
        Type::Int => Ok(IrType::Int32),
        Type::Bool => Ok(IrType::Bool),
        Type::Class(_) => Ok(IrType::Ref),
        Type::String => Err("IR: String lowering is not implemented".into()),
        Type::Void => Err("IR: void is not a value type".into()),
    }
}

struct Builder {
    function: FunctionIr,
    blocks: Vec<(Vec<Instruction>, Option<Terminator>)>,
    current: Option<BlockId>,
    bindings: HashMap<String, VirtualRegId>,
    loop_exits: Vec<BlockId>,
}

impl Builder {
    fn value(&mut self, ty: IrType) -> VirtualRegId {
        self.function.new_register(ty, None)
    }

    fn block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len());
        self.blocks.push((vec![], None));
        id
    }

    fn emit(&mut self, kind: InstructionKind, line: u32) {
        let block = self.current.expect("instruction needs an open block");
        self.blocks[block.0].0.push(Instruction { kind, line });
    }

    fn finish(&mut self, terminator: Terminator) {
        let block = self.current.take().expect("terminator needs an open block");
        self.blocks[block.0].1 = Some(terminator);
    }

    fn binding(
        &self,
        name: &str,
        binding: &BindingInfo,
        line: u32,
    ) -> Result<VirtualRegId, String> {
        if !matches!(binding, BindingInfo::Local(_) | BindingInfo::Formal(_)) {
            return Err(format!(
                "IR line {line}: field/prebound lowering is not implemented"
            ));
        }
        self.bindings
            .get(name)
            .copied()
            .ok_or_else(|| format!("IR line {line}: unknown local {name}"))
    }

    fn statements(&mut self, stmts: &[TypedStmt]) -> Result<(), String> {
        for stmt in stmts {
            if self.current.is_none() {
                break;
            }
            match stmt {
                TypedStmt::Assign {
                    target,
                    binding,
                    value,
                    line,
                } => {
                    let dst = self.binding(target, binding, *line)?;
                    let src = self.expression(value)?;
                    self.emit(InstructionKind::Copy { dst, src }, *line);
                }
                TypedStmt::Return(expr, _) => {
                    let value = self.expression(expr)?;
                    self.finish(Terminator::Return(Some(value)));
                }
                TypedStmt::If(cond, yes, no, _) => {
                    let condition = self.expression(cond)?;
                    let then_block = self.block();
                    let else_block = self.block();
                    self.finish(Terminator::Branch {
                        condition,
                        then_block,
                        else_block,
                    });
                    self.current = Some(then_block);
                    self.statements(yes)?;
                    let then_end = self.current.take();
                    self.current = Some(else_block);
                    self.statements(no)?;
                    let else_end = self.current.take();
                    if then_end.is_some() || else_end.is_some() {
                        let join = self.block();
                        for end in [then_end, else_end].into_iter().flatten() {
                            self.current = Some(end);
                            self.finish(Terminator::Jump(join));
                        }
                        self.current = Some(join);
                    }
                }
                TypedStmt::While(cond, body, _) => {
                    let test = self.block();
                    let body_block = self.block();
                    let exit = self.block();
                    self.finish(Terminator::Jump(test));
                    self.current = Some(test);
                    let condition = self.expression(cond)?;
                    self.finish(Terminator::Branch {
                        condition,
                        then_block: body_block,
                        else_block: exit,
                    });
                    self.loop_exits.push(exit);
                    self.current = Some(body_block);
                    self.statements(body)?;
                    if self.current.is_some() {
                        self.finish(Terminator::Jump(test));
                    }
                    self.loop_exits.pop();
                    self.current = Some(exit);
                }
                TypedStmt::Break(line) => {
                    let exit = *self
                        .loop_exits
                        .last()
                        .ok_or_else(|| format!("IR line {line}: break outside loop"))?;
                    self.finish(Terminator::Jump(exit));
                }
                TypedStmt::Empty(_) => {}
                TypedStmt::CallStmt(call) => {
                    return Err(format!(
                        "IR line {}: call lowering is not implemented",
                        call.line
                    ))
                }
            }
        }
        Ok(())
    }

    fn expression(&mut self, expr: &TypedExpr) -> Result<Operand, String> {
        Ok(match expr {
            TypedExpr::Num(n, _) => Operand::Int(*n),
            TypedExpr::Bool(b, _) => Operand::Bool(*b),
            TypedExpr::Null(_) => Operand::Null,
            TypedExpr::This(_, _) => Operand::Value(self.function.params[0]),
            TypedExpr::Var {
                name,
                binding,
                line,
            } => {
                let src = Operand::Value(self.binding(name, binding, *line)?);
                // Snapshot mutable locals before evaluating subsequent operands.
                let dst = self.value(value_type(binding.ty())?);
                self.emit(InstructionKind::Copy { dst, src }, *line);
                Operand::Value(dst)
            }
            TypedExpr::Unop {
                op,
                operand,
                ty,
                line,
            } => {
                let src = self.expression(operand)?;
                let dst = self.value(value_type(ty)?);
                let op = match op {
                    Unop::Neg => UnaryOp::Neg,
                    Unop::Not => UnaryOp::Not,
                };
                self.emit(InstructionKind::Unary { dst, op, src }, *line);
                Operand::Value(dst)
            }
            TypedExpr::Binop {
                lhs,
                op: Binop::And,
                rhs,
                line,
                ..
            } => {
                let no = TypedExpr::Bool(false, *line);
                self.choose(lhs, rhs, &no, IrType::Bool, *line)?
            }
            TypedExpr::Binop {
                lhs,
                op: Binop::Or,
                rhs,
                line,
                ..
            } => {
                let yes = TypedExpr::Bool(true, *line);
                self.choose(lhs, &yes, rhs, IrType::Bool, *line)?
            }
            TypedExpr::Binop {
                lhs,
                op,
                rhs,
                ty,
                line,
            } => {
                let lhs = self.expression(lhs)?;
                let rhs = self.expression(rhs)?;
                let dst = self.value(value_type(ty)?);
                let op = match op {
                    Binop::Add => BinaryOp::Add,
                    Binop::Sub => BinaryOp::Sub,
                    Binop::Mul => BinaryOp::Mul,
                    Binop::Div => BinaryOp::Div,
                    Binop::Mod => BinaryOp::Mod,
                    Binop::Eq => BinaryOp::Eq,
                    Binop::Lt => BinaryOp::Lt,
                    Binop::Gt => BinaryOp::Gt,
                    Binop::And | Binop::Or => unreachable!(),
                };
                if matches!(op, BinaryOp::Div | BinaryOp::Mod) {
                    self.division(dst, op, lhs, rhs, *line);
                } else {
                    self.emit(InstructionKind::Binary { dst, op, lhs, rhs }, *line);
                }
                Operand::Value(dst)
            }
            TypedExpr::Ternary {
                cond,
                then_branch,
                else_branch,
                ty,
                line,
            } => {
                let ty = ty
                    .as_ref()
                    .map(value_type)
                    .transpose()?
                    .unwrap_or(IrType::Ref);
                self.choose(cond, then_branch, else_branch, ty, *line)?
            }
            TypedExpr::Str(_, line) => {
                return Err(format!(
                    "IR line {line}: String lowering is not implemented"
                ))
            }
            TypedExpr::New { line, .. }
            | TypedExpr::Cast { line, .. }
            | TypedExpr::InstanceOf { line, .. } => {
                return Err(format!(
                    "IR line {line}: object/runtime lowering is not implemented"
                ));
            }
            TypedExpr::Call(call) => {
                return Err(format!(
                    "IR line {}: call lowering is not implemented",
                    call.line
                ))
            }
        })
    }

    fn division(&mut self, dst: VirtualRegId, op: BinaryOp, lhs: Operand, rhs: Operand, line: u32) {
        // Both operands have already been evaluated, even for a special divisor.
        if let Operand::Int(divisor) = rhs {
            match divisor {
                0 => self.division_special(dst, op, lhs, false, line),
                -1 => self.division_special(dst, op, lhs, true, line),
                _ => self.emit(InstructionKind::Binary { dst, op, lhs, rhs }, line),
            }
            return;
        }
        let zero = self.block();
        let check_negative_one = self.block();
        let negative_one = self.block();
        let ordinary = self.block();
        let join = self.block();
        let condition = self.value(IrType::Bool);
        self.emit(
            InstructionKind::Binary {
                dst: condition,
                op: BinaryOp::Eq,
                lhs: rhs,
                rhs: Operand::Int(0),
            },
            line,
        );
        self.finish(Terminator::Branch {
            condition: Operand::Value(condition),
            then_block: zero,
            else_block: check_negative_one,
        });
        self.current = Some(zero);
        self.division_special(dst, op, lhs, false, line);
        self.finish(Terminator::Jump(join));

        self.current = Some(check_negative_one);
        let condition = self.value(IrType::Bool);
        self.emit(
            InstructionKind::Binary {
                dst: condition,
                op: BinaryOp::Eq,
                lhs: rhs,
                rhs: Operand::Int(-1),
            },
            line,
        );
        self.finish(Terminator::Branch {
            condition: Operand::Value(condition),
            then_block: negative_one,
            else_block: ordinary,
        });
        self.current = Some(negative_one);
        self.division_special(dst, op, lhs, true, line);
        self.finish(Terminator::Jump(join));

        self.current = Some(ordinary);
        self.emit(InstructionKind::Binary { dst, op, lhs, rhs }, line);
        self.finish(Terminator::Jump(join));
        self.current = Some(join);
    }

    fn division_special(
        &mut self,
        dst: VirtualRegId,
        op: BinaryOp,
        lhs: Operand,
        negative_one: bool,
        line: u32,
    ) {
        let kind = match (op, negative_one) {
            (BinaryOp::Div, false) => InstructionKind::Copy {
                dst,
                src: Operand::Int(-1),
            },
            (BinaryOp::Mod, false) => InstructionKind::Copy { dst, src: lhs },
            (BinaryOp::Div, true) => InstructionKind::Unary {
                dst,
                op: UnaryOp::Neg,
                src: lhs,
            },
            (BinaryOp::Mod, true) => InstructionKind::Copy {
                dst,
                src: Operand::Int(0),
            },
            _ => unreachable!("division special case needs Div or Mod"),
        };
        self.emit(kind, line);
    }

    fn choose(
        &mut self,
        cond: &TypedExpr,
        yes: &TypedExpr,
        no: &TypedExpr,
        ty: IrType,
        line: u32,
    ) -> Result<Operand, String> {
        let condition = self.expression(cond)?;
        let then_block = self.block();
        let else_block = self.block();
        let join = self.block();
        let dst = self.value(ty);
        self.finish(Terminator::Branch {
            condition,
            then_block,
            else_block,
        });
        for (block, expr) in [(then_block, yes), (else_block, no)] {
            self.current = Some(block);
            let src = self.expression(expr)?;
            self.emit(InstructionKind::Copy { dst, src }, line);
            self.finish(Terminator::Jump(join));
        }
        self.current = Some(join);
        Ok(Operand::Value(dst))
    }
}
