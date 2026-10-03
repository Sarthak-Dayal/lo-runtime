use super::*;
use std::fmt::{self, Write};

impl ProgramIr {
    pub fn dump(&self) -> String {
        let mut out = String::new();
        for (id, symbol) in self.symbols.iter().enumerate() {
            writeln!(out, "@{id} = {:?}", symbol.name).unwrap();
        }
        for (id, sig) in self.signatures.iter().enumerate() {
            writeln!(out, "sig{id} {:?} -> {:?}", sig.params, sig.result).unwrap();
        }
        for f in &self.functions {
            write!(out, "\nfunction @{}(", f.symbol.0).unwrap();
            for (i, param) in f.params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write!(out, "v{}", param.0).unwrap();
            }
            writeln!(out, ") entry b{} {{", f.entry.0).unwrap();
            for (id, ty) in f.value_types.iter().enumerate() {
                writeln!(out, "  v{id}: {ty:?}").unwrap();
            }
            for (id, block) in f.blocks.iter().enumerate() {
                writeln!(out, "b{id}:").unwrap();
                for inst in &block.instructions {
                    writeln!(out, "  {} ; line {}", inst.kind, inst.line).unwrap();
                }
                writeln!(out, "  {}", block.terminator).unwrap();
            }
            out.push_str("}\n");
        }
        out
    }
}

impl fmt::Display for Operand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Value(id) => write!(f, "v{}", id.0),
            Self::Int(value) => write!(f, "{value}"),
            Self::Bool(value) => write!(f, "{value}"),
            Self::Null => write!(f, "null"),
            Self::Symbol(id) => write!(f, "@{}", id.0),
        }
    }
}
impl fmt::Display for InstructionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Copy { dst, src } => write!(f, "v{} = copy {src}", dst.0),
            Self::Binary { dst, op, lhs, rhs } => {
                let name = match op {
                    BinaryOp::Add => "add",
                    BinaryOp::Sub => "sub",
                    BinaryOp::Mul => "mul",
                    BinaryOp::Div => "div",
                    BinaryOp::Mod => "mod",
                    BinaryOp::Eq => "eq",
                    BinaryOp::Ne => "ne",
                    BinaryOp::Lt => "lt",
                    BinaryOp::Le => "le",
                    BinaryOp::Gt => "gt",
                    BinaryOp::Ge => "ge",
                };
                write!(f, "v{} = {name} {lhs}, {rhs}", dst.0)
            }
            Self::Unary { dst, op, src } => write!(
                f,
                "v{} = {} {src}",
                dst.0,
                match op {
                    UnaryOp::Neg => "neg",
                    UnaryOp::Not => "not",
                }
            ),
            Self::Load { dst, base, offset } => write!(f, "v{} = load [{base} + {offset}]", dst.0),
            Self::Store {
                base,
                offset,
                value,
                ty,
            } => write!(f, "store {ty:?} [{base} + {offset}], {value}"),
            Self::StoreRef {
                object,
                offset,
                value,
            } => write!(f, "store_ref [{object} + {offset}], {value}"),
            Self::NullCheck {
                receiver,
                method_name,
            } => write!(f, "nullcheck {receiver}, {method_name:?}"),
            Self::Call {
                dst,
                target,
                signature,
                args,
                effects,
            } => {
                if let Some(dst) = dst {
                    write!(f, "v{} = ", dst.0)?;
                }
                match target {
                    CallTarget::Direct(id) => write!(f, "call @{}", id.0)?,
                    CallTarget::Indirect(pointer) => write!(f, "call_indirect {pointer}")?,
                }
                write!(f, " sig{}(", signature.0)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                write!(
                    f,
                    ") may_gc={} no_return={}",
                    effects.may_gc, effects.no_return
                )
            }
        }
    }
}
impl fmt::Display for Terminator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Jump(block) => write!(f, "jump b{}", block.0),
            Self::Branch {
                condition,
                then_block,
                else_block,
            } => write!(
                f,
                "branch {condition}, b{}, b{}",
                then_block.0, else_block.0
            ),
            Self::Return(Some(value)) => write!(f, "return {value}"),
            Self::Return(None) => write!(f, "return"),
            Self::Unreachable => write!(f, "unreachable"),
        }
    }
}
