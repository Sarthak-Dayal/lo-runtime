//! Typed AST -> semantic IR, one function per method and constructor plus
//! `lo_entry`. Every runtime operation is an explicit call, and null checks,
//! dispatch and division guards are explicit control flow; GC roots are added
//! later by `gc_roots`.

use std::collections::HashMap;

use crate::ast::{Binop, Type, Unop};
use crate::type_checker::{
    CastDirection, TypedConstructor, TypedDelegation, TypedExpr, TypedMethodBody, TypedMethodDecl,
    TypedStmt,
};

use super::*;

mod program;
mod runtime;

use crate::symbols;
pub use program::lower_program;
use program::Context;

fn lower_method(
    ctx: &mut Context<'_>,
    class: &str,
    method: &TypedMethodDecl,
) -> Result<FunctionIr, String> {
    let locals = match &method.body {
        TypedMethodBody::UserDefined { locals, .. } => locals.as_slice(),
        TypedMethodBody::Io(_) => &[],
    };
    let symbol = ctx.symbol(&symbols::method(class, &method.method_name))?;
    let mut builder = Builder::new(ctx, symbol, true, &method.formals, locals, method.line)?;
    match &method.body {
        TypedMethodBody::UserDefined { stmts, .. } => builder.statements(stmts)?,
        TypedMethodBody::Io(op) => builder.io_wrapper(op, method.line)?,
    }
    builder.complete(&method.return_type, method.line)
}

fn lower_constructor(
    ctx: &mut Context<'_>,
    class: &str,
    ctor: &TypedConstructor,
) -> Result<FunctionIr, String> {
    let (formals, locals, line) = match ctor {
        TypedConstructor::Explicit {
            formals,
            locals,
            line,
            ..
        } => (formals.as_slice(), locals.as_slice(), *line),
        TypedConstructor::Implicit { fields } => (fields.as_slice(), &[][..], 0),
    };
    let symbol = ctx.symbol(&symbols::constructor(class, formals.len()))?;
    let mut builder = Builder::new(ctx, symbol, true, formals, locals, line)?;
    match ctor {
        TypedConstructor::Explicit {
            delegation, stmts, ..
        } => {
            if let Some(delegation) = delegation {
                let (target, actuals, line) = match delegation {
                    TypedDelegation::ThisCall { actuals, line } => {
                        (class.to_owned(), actuals, *line)
                    }
                    TypedDelegation::SuperCall { actuals, line } => (
                        builder
                            .ctx
                            .classes
                            .get(class)
                            .and_then(|c| c.parent.clone())
                            .ok_or("IR: missing constructor parent")?,
                        actuals,
                        *line,
                    ),
                };
                let mut args = vec![builder.receiver()];
                args.extend(builder.actuals(actuals)?);
                let symbol = builder
                    .ctx
                    .symbol(&symbols::constructor(&target, actuals.len()))?;
                builder.call(symbol, args, line);
            }
            builder.statements(stmts)?;
        }
        TypedConstructor::Implicit { fields } => {
            for (name, ty) in fields {
                let value = Operand::Value(builder.bindings[name]);
                let offset = builder.ctx.field_offset(class, name)?;
                builder.store_field(builder.receiver(), offset, value, value_type(ty)?, line)?;
            }
        }
    }
    builder.complete(&Type::Void, line)
}

fn lower_startup(ctx: &mut Context<'_>) -> Result<FunctionIr, String> {
    let symbol = ctx.symbol("lo_entry")?;
    let mut builder = Builder::new(ctx, symbol, false, &[], &[], 0)?;
    let frame = builder.ctx.symbol(symbols::BINDINGS)?;
    builder.runtime("lo_push_frame", vec![Operand::Symbol(frame)], 0)?;
    for name in symbols::PREBOUND {
        let class = if name == "in" { "Input" } else { "Output" };
        let object = builder.allocate(class, vec![], 0)?;
        if class == "Output" {
            let destination = builder.ctx.output_destination()?;
            builder.store_field(
                object,
                destination,
                Operand::Int(i32::from(name == "err")),
                IrType::Int32,
                0,
            )?;
        }
        builder.store_binding(name, object, 0)?;
    }
    // A direct call, as in LO-3 §4.6: no class may extend Main, and lo_alloc
    // aborts rather than return null, so neither dispatch nor a null check is needed.
    let main = builder.allocate("Main", vec![], 0)?;
    let main_method = builder.ctx.symbol(&symbols::method("Main", "main"))?;
    let result = builder.call(main_method, vec![main], 0);
    builder.runtime("lo_pop_frame", vec![], 0)?;
    builder.finish(Terminator::Return(result));
    builder.complete(&Type::Int, 0)
}

fn length(len: usize) -> Result<i32, String> {
    i32::try_from(len).map_err(|_| "IR: length exceeds signed IR range".into())
}

fn value_type(ty: &Type) -> Result<IrType, String> {
    declare::ir_type(ty).ok_or_else(|| "IR: void is not a value type".into())
}

struct Builder<'ctx, 'ast> {
    ctx: &'ctx mut Context<'ast>,
    function: FunctionIr,
    blocks: Vec<(Vec<Instruction>, Option<Terminator>)>,
    current: Option<BlockId>,
    bindings: HashMap<String, VirtualRegId>,
    loop_exits: Vec<BlockId>,
}

impl<'ctx, 'ast> Builder<'ctx, 'ast> {
    fn new(
        ctx: &'ctx mut Context<'ast>,
        symbol: SymbolId,
        receiver: bool,
        formals: &[(String, Type)],
        locals: &[(String, Type)],
        line: u32,
    ) -> Result<Self, String> {
        let mut builder = Self {
            ctx,
            function: FunctionIr {
                symbol,
                params: vec![],
                register_types: vec![],
                register_names: vec![],
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
        if receiver {
            let id = builder
                .function
                .new_register(IrType::Ref, Some("this".into()));
            builder.function.params.push(id);
        }
        for (name, ty) in formals {
            let id = builder
                .function
                .new_register(value_type(ty)?, Some(name.clone()));
            builder.function.params.push(id);
            builder.bindings.insert(name.clone(), id);
        }
        for (name, ty) in locals {
            let id = builder
                .function
                .new_register(value_type(ty)?, Some(name.clone()));
            builder.bindings.insert(name.clone(), id);
            let src = match ty {
                Type::Int => Operand::Int(0),
                Type::Bool => Operand::Bool(false),
                Type::Class(_) => Operand::Null,
                Type::String => Operand::Symbol(builder.ctx.symbol(symbols::EMPTY_STRING)?),
                Type::Void => return Err("IR: void local".into()),
            };
            builder.emit(InstructionKind::Copy { dst: id, src }, line);
        }
        Ok(builder)
    }

    fn complete(mut self, result: &Type, line: u32) -> Result<FunctionIr, String> {
        if self.current.is_some() {
            if *result != Type::Void {
                return Err(format!("IR line {line}: non-void method falls through"));
            }
            self.finish(Terminator::Return(None));
        }
        self.function.blocks = self
            .blocks
            .into_iter()
            .map(|(instructions, terminator)| {
                Ok(BasicBlock {
                    instructions,
                    terminator: terminator.ok_or("IR: unterminated block")?,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(self.function)
    }

    fn receiver(&self) -> Operand {
        Operand::Value(self.function.params[0])
    }

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

    /// The register that holds a local or formal for the whole function.
    fn local(&self, name: &str, line: u32) -> Result<VirtualRegId, String> {
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
                    let src = self.expression(value)?;
                    self.assign(target, binding, src, *line)?;
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
                    self.method_call(call)?;
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
            TypedExpr::This(_, _) => self.receiver(),
            TypedExpr::Var {
                name,
                binding,
                line,
            } => self.read_binding(name, binding, *line)?,
            TypedExpr::Unop {
                op,
                operand,
                ty,
                line,
            } => {
                let src = self.expression(operand)?;
                if *ty == Type::String {
                    return self.runtime_value("lo_string_reverse", vec![src], *line);
                }
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
                let string = lhs.ty() == Some(Type::String);
                let lhs = self.expression(lhs)?;
                let rhs = self.expression(rhs)?;
                if string {
                    return self.string_binary(*op, lhs, rhs, *line);
                }
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
            TypedExpr::Str(string, line) => {
                if string.is_empty() {
                    Operand::Symbol(self.ctx.symbol(symbols::EMPTY_STRING)?)
                } else {
                    let bytes = self.ctx.bytes(string.as_bytes())?;
                    self.runtime_value(
                        "lo_string_new",
                        vec![Operand::Symbol(bytes), Operand::Int(length(string.len())?)],
                        *line,
                    )?
                }
            }
            TypedExpr::New {
                class,
                actuals,
                line,
            } => {
                let args = self.actuals(actuals)?;
                self.allocate(class, args, *line)?
            }
            TypedExpr::Cast {
                target,
                operand,
                direction,
                line,
            } => {
                let src = self.expression(operand)?;
                if *direction == CastDirection::Downcast {
                    let Type::Class(class) = target else {
                        return Err("IR: non-class downcast".into());
                    };
                    let descriptor = self.ctx.symbol(&symbols::class_descriptor(class))?;
                    self.runtime_value(
                        "lo_cast_check",
                        vec![src, Operand::Symbol(descriptor)],
                        *line,
                    )?
                } else {
                    src
                }
            }
            TypedExpr::InstanceOf {
                operand,
                class,
                line,
            } => {
                let src = self.expression(operand)?;
                let descriptor = self.ctx.symbol(&symbols::class_descriptor(class))?;
                self.runtime_value(
                    "lo_instanceof",
                    vec![src, Operand::Symbol(descriptor)],
                    *line,
                )?
            }
            TypedExpr::Call(call) => self
                .method_call(call)?
                .ok_or("IR: void call used as value")?,
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
