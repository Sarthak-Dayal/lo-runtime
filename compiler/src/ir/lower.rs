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

mod operations;
mod program;

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
    let mut builder = Builder::new(
        ctx,
        symbol,
        /* has_receiver */ true,
        &method.formals,
        locals,
        method.line,
    )?;
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
        TypedConstructor::Implicit { fields } => (fields.as_slice(), &[][..], GENERATED_LINE),
    };
    let symbol = ctx.symbol(&symbols::constructor(class, formals.len()))?;
    let mut builder = Builder::new(
        ctx, symbol, /* has_receiver */ true, formals, locals, line,
    )?;
    match ctor {
        TypedConstructor::Explicit {
            delegation, stmts, ..
        } => {
            if let Some(delegation) = delegation {
                delegate(&mut builder, class, delegation)?;
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

/// `this(...)` or `super(...)` at the start of a constructor: call the target
/// constructor on the same receiver.
fn delegate(
    builder: &mut Builder<'_, '_>,
    class: &str,
    delegation: &TypedDelegation,
) -> Result<(), String> {
    let (target, actuals, line) = match delegation {
        TypedDelegation::ThisCall { actuals, line } => (class.to_owned(), actuals, *line),
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
    Ok(())
}

fn lower_startup(ctx: &mut Context<'_>) -> Result<FunctionIr, String> {
    let symbol = ctx.symbol("lo_entry")?;
    let mut builder = Builder::new(
        ctx,
        symbol,
        /* has_receiver */ false,
        &[],
        &[],
        GENERATED_LINE,
    )?;
    // Link the static lo_bindings frame into the shadow stack so the collector
    // sees (and updates) in, out and err.
    let frame = builder.ctx.symbol(symbols::BINDINGS)?;
    builder.runtime(
        "lo_push_frame",
        vec![Operand::Symbol(frame)],
        GENERATED_LINE,
    )?;
    for name in symbols::PREBOUND {
        // Output destination: 0 = stdout, 1 = stderr.
        let (class, destination) = match name {
            "in" => ("Input", None),
            "out" => ("Output", Some(0)),
            _ => ("Output", Some(1)),
        };
        let object = builder.allocate(class, vec![], GENERATED_LINE)?;
        if let Some(destination) = destination {
            let offset = builder.ctx.output_destination()?;
            builder.store_field(
                object,
                offset,
                Operand::Int(destination),
                IrType::Int32,
                GENERATED_LINE,
            )?;
        }
        builder.store_binding(name, object, GENERATED_LINE)?;
    }
    // A direct call, as in LO-3 §4.6: no class may extend Main, and lo_alloc
    // aborts rather than return null, so neither dispatch nor a null check is needed.
    let main = builder.allocate("Main", vec![], GENERATED_LINE)?;
    let main_method = builder.ctx.symbol(&symbols::method("Main", "main"))?;
    let result = builder.call(main_method, vec![main], GENERATED_LINE);
    builder.runtime("lo_pop_frame", vec![], GENERATED_LINE)?;
    builder.finish(Terminator::Return(result));
    builder.complete(&Type::Int, GENERATED_LINE)
}

/// A byte count as the `i32` length the runtime takes.
fn byte_length(len: usize) -> Result<i32, String> {
    i32::try_from(len).map_err(|_| "IR: length exceeds signed IR range".into())
}

fn value_type(ty: &Type) -> Result<IrType, String> {
    declare::ir_type(ty).ok_or_else(|| "IR: void is not a value type".into())
}

/// A block under construction; its terminator is set once, by `finish`.
struct OpenBlock {
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

/// Builds one function's blocks. `current` is the block being filled; `None`
/// means the last statement was a `return` or `break`, so what follows is dead
/// and is not lowered.
struct Builder<'ctx, 'ast> {
    ctx: &'ctx mut Context<'ast>,
    function: FunctionIr,
    blocks: Vec<OpenBlock>,
    current: Option<BlockId>,
    bindings: HashMap<String, VirtualRegId>,
    loop_exits: Vec<BlockId>,
}

impl<'ctx, 'ast> Builder<'ctx, 'ast> {
    fn new(
        ctx: &'ctx mut Context<'ast>,
        symbol: SymbolId,
        has_receiver: bool,
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
        if has_receiver {
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
            .map(|block| {
                Ok(BasicBlock {
                    instructions: block.instructions,
                    terminator: block.terminator.ok_or("IR: unterminated block")?,
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
        self.blocks.push(OpenBlock {
            instructions: vec![],
            terminator: None,
        });
        id
    }

    fn emit(&mut self, kind: InstructionKind, line: u32) {
        let block = self.current.expect("instruction needs an open block");
        self.blocks[block.0]
            .instructions
            .push(Instruction { kind, line });
    }

    fn finish(&mut self, terminator: Terminator) {
        let block = self.current.take().expect("terminator needs an open block");
        self.blocks[block.0].terminator = Some(terminator);
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
                TypedStmt::If(cond, yes, no, _) => self.if_statement(cond, yes, no)?,
                TypedStmt::While(cond, body, _) => self.while_statement(cond, body)?,
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

    fn if_statement(
        &mut self,
        cond: &TypedExpr,
        yes: &[TypedStmt],
        no: &[TypedStmt],
    ) -> Result<(), String> {
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
        // A join block only if some branch falls through; otherwise the code
        // after the `if` is dead.
        if then_end.is_some() || else_end.is_some() {
            let join = self.block();
            for end in [then_end, else_end].into_iter().flatten() {
                self.current = Some(end);
                self.finish(Terminator::Jump(join));
            }
            self.current = Some(join);
        }
        Ok(())
    }

    fn while_statement(&mut self, cond: &TypedExpr, body: &[TypedStmt]) -> Result<(), String> {
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
            } => self.unary(*op, operand, ty, *line)?,
            // && and || short-circuit, so they lower as a ternary.
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
            } => self.binary(lhs, *op, rhs, ty, *line)?,
            TypedExpr::Ternary {
                cond,
                then_branch,
                else_branch,
                ty,
                line,
            } => {
                // A ternary whose branches are both null has no static type;
                // null is a Ref.
                let ty = ty
                    .as_ref()
                    .map(value_type)
                    .transpose()?
                    .unwrap_or(IrType::Ref);
                self.choose(cond, then_branch, else_branch, ty, *line)?
            }
            TypedExpr::Str(string, line) => self.string_literal(string, *line)?,
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
            } => self.cast(target, operand, *direction, *line)?,
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

    fn unary(
        &mut self,
        op: Unop,
        operand: &TypedExpr,
        ty: &Type,
        line: u32,
    ) -> Result<Operand, String> {
        let src = self.expression(operand)?;
        if *ty == Type::String {
            return self.runtime_value("lo_string_reverse", vec![src], line);
        }
        let dst = self.value(value_type(ty)?);
        let op = match op {
            Unop::Neg => UnaryOp::Neg,
            Unop::Not => UnaryOp::Not,
        };
        self.emit(InstructionKind::Unary { dst, op, src }, line);
        Ok(Operand::Value(dst))
    }

    /// Every binary operator except `&&` and `||`, on Int, Bool or String operands.
    fn binary(
        &mut self,
        lhs: &TypedExpr,
        op: Binop,
        rhs: &TypedExpr,
        ty: &Type,
        line: u32,
    ) -> Result<Operand, String> {
        let string = lhs.ty() == Some(Type::String);
        let lhs = self.expression(lhs)?;
        let rhs = self.expression(rhs)?;
        if string {
            return self.string_binary(op, lhs, rhs, line);
        }
        let dst = self.value(value_type(ty)?);
        let op = binary_op(op);
        if matches!(op, BinaryOp::Div | BinaryOp::Mod) {
            self.division(dst, op, lhs, rhs, line);
        } else {
            self.emit(InstructionKind::Binary { dst, op, lhs, rhs }, line);
        }
        Ok(Operand::Value(dst))
    }

    fn string_literal(&mut self, string: &str, line: u32) -> Result<Operand, String> {
        if string.is_empty() {
            return Ok(Operand::Symbol(self.ctx.symbol(symbols::EMPTY_STRING)?));
        }
        let bytes = self.ctx.bytes(string.as_bytes())?;
        self.runtime_value(
            "lo_string_new",
            vec![
                Operand::Symbol(bytes),
                Operand::Int(byte_length(string.len())?),
            ],
            line,
        )
    }

    /// An upcast needs no code; a downcast is checked by the runtime.
    fn cast(
        &mut self,
        target: &Type,
        operand: &TypedExpr,
        direction: CastDirection,
        line: u32,
    ) -> Result<Operand, String> {
        let src = self.expression(operand)?;
        if direction != CastDirection::Downcast {
            return Ok(src);
        }
        let Type::Class(class) = target else {
            return Err("IR: non-class downcast".into());
        };
        let descriptor = self.ctx.symbol(&symbols::class_descriptor(class))?;
        self.runtime_value(
            "lo_cast_check",
            vec![src, Operand::Symbol(descriptor)],
            line,
        )
    }

    /// LO division is total (language reference, LO-2 integer semantics, RISC-V
    /// convention): x / 0 = -1, x % 0 = x, and for divisor -1, x / -1 = -x
    /// (wrapping, so INT_MIN stays INT_MIN) and x % -1 = 0. Both special divisors
    /// are routed around `idiv`, which would fault on 0 and on INT_MIN / -1.
    fn division(&mut self, dst: VirtualRegId, op: BinaryOp, lhs: Operand, rhs: Operand, line: u32) {
        // Both operands have already been evaluated, even for a special divisor.
        if let Operand::Int(divisor) = rhs {
            match divisor {
                0 | -1 => self.division_special(dst, op, lhs, divisor, line),
                _ => self.emit(InstructionKind::Binary { dst, op, lhs, rhs }, line),
            }
            return;
        }
        let zero = self.block();
        let check_negative_one = self.block();
        let negative_one = self.block();
        let ordinary = self.block();
        let join = self.block();
        self.branch_if_equal(rhs, 0, zero, check_negative_one, line);
        self.current = Some(zero);
        self.division_special(dst, op, lhs, 0, line);
        self.finish(Terminator::Jump(join));

        self.current = Some(check_negative_one);
        self.branch_if_equal(rhs, -1, negative_one, ordinary, line);
        self.current = Some(negative_one);
        self.division_special(dst, op, lhs, -1, line);
        self.finish(Terminator::Jump(join));

        self.current = Some(ordinary);
        self.emit(InstructionKind::Binary { dst, op, lhs, rhs }, line);
        self.finish(Terminator::Jump(join));
        self.current = Some(join);
    }

    /// Ends the current block with "if `value` == `constant` go to `then_block`,
    /// else to `else_block`".
    fn branch_if_equal(
        &mut self,
        value: Operand,
        constant: i32,
        then_block: BlockId,
        else_block: BlockId,
        line: u32,
    ) {
        let condition = self.value(IrType::Bool);
        self.emit(
            InstructionKind::Binary {
                dst: condition,
                op: BinaryOp::Eq,
                lhs: value,
                rhs: Operand::Int(constant),
            },
            line,
        );
        self.finish(Terminator::Branch {
            condition: Operand::Value(condition),
            then_block,
            else_block,
        });
    }

    /// `lhs op divisor` for divisor 0 or -1, without `idiv`.
    fn division_special(
        &mut self,
        dst: VirtualRegId,
        op: BinaryOp,
        lhs: Operand,
        divisor: i32,
        line: u32,
    ) {
        let kind = match (op, divisor) {
            (BinaryOp::Div, 0) => InstructionKind::Copy {
                dst,
                src: Operand::Int(-1),
            },
            (BinaryOp::Mod, 0) => InstructionKind::Copy { dst, src: lhs },
            (BinaryOp::Div, -1) => InstructionKind::Unary {
                dst,
                op: UnaryOp::Neg,
                src: lhs,
            },
            (BinaryOp::Mod, -1) => InstructionKind::Copy {
                dst,
                src: Operand::Int(0),
            },
            _ => unreachable!("division special case needs Div or Mod by 0 or -1"),
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

/// The IR operator for an AST binary operator other than `&&` and `||`.
fn binary_op(op: Binop) -> BinaryOp {
    match op {
        Binop::Add => BinaryOp::Add,
        Binop::Sub => BinaryOp::Sub,
        Binop::Mul => BinaryOp::Mul,
        Binop::Div => BinaryOp::Div,
        Binop::Mod => BinaryOp::Mod,
        Binop::Eq => BinaryOp::Eq,
        Binop::Lt => BinaryOp::Lt,
        Binop::Gt => BinaryOp::Gt,
        Binop::And | Binop::Or => unreachable!("&& and || are lowered as a ternary"),
    }
}
