use super::*;
use std::fmt::{self, Write};

impl ProgramIr {
    pub fn dump(&self) -> String {
        let mut out = String::new();
        for (id, symbol) in self.symbols.iter().enumerate() {
            writeln!(out, "@{id} = {:?} {:?}", symbol.name, symbol.kind).unwrap();
        }
        for (id, sig) in self.signatures.iter().enumerate() {
            writeln!(out, "sig{id} {:?} -> {:?}", sig.params, sig.result).unwrap();
        }
        for f in &self.functions {
            write!(out, "\nfunc @{}(", f.symbol.0).unwrap();
            for (i, param) in f.params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write!(out, "t{}", param.0).unwrap();
            }
            writeln!(out, ") entry .L{} {{", f.entry.0).unwrap();
            for (id, ty) in f.value_types.iter().enumerate() {
                writeln!(out, "  t{id}: {ty:?}").unwrap();
            }
            for (id, block) in f.blocks.iter().enumerate() {
                writeln!(out, ".L{id}:").unwrap();
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
            Self::Value(id) => write!(f, "t{}", id.0),
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
            Self::Copy { dst, src } => write!(f, "t{} = {src}", dst.0),
            Self::Binary { dst, op, lhs, rhs } => {
                let name = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => "%",
                    BinaryOp::Eq => "==",
                    BinaryOp::Ne => "!=",
                    BinaryOp::Lt => "<",
                    BinaryOp::Le => "<=",
                    BinaryOp::Gt => ">",
                    BinaryOp::Ge => ">=",
                };
                write!(f, "t{} = {lhs} {name} {rhs}", dst.0)
            }
            Self::Unary { dst, op, src } => write!(
                f,
                "t{} = {} {src}",
                dst.0,
                match op {
                    UnaryOp::Neg => "neg",
                    UnaryOp::Not => "!",
                }
            ),
            Self::Load { dst, base, offset } => write!(f, "t{} = load [{base} + {offset}]", dst.0),
            Self::Store {
                base,
                offset,
                value,
                ty,
            } => write!(f, "store {ty:?} [{base} + {offset}], {value}"),
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
                    write!(f, "t{} = ", dst.0)?;
                }
                match target {
                    CallTarget::Direct(id) => write!(f, "call @{}", id.0)?,
                    CallTarget::Indirect(pointer) => write!(f, "call_indirect {pointer}")?,
                }

                for arg in args {
                    write!(f, ", {arg}")?;
                }
                write!(f, " ; sig{} may_gc={}", signature.0, effects.may_gc)
            }
        }
    }
}
impl fmt::Display for Terminator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Jump(block) => write!(f, "br .L{}", block.0),
            Self::Branch {
                condition,
                then_block,
                else_block,
            } => write!(f, "cbr {condition}, .L{}, .L{}", then_block.0, else_block.0),
            Self::Return(Some(value)) => write!(f, "ret {value}"),
            Self::Return(None) => write!(f, "ret"),
            Self::Abort { target, args } => {
                write!(f, "abort @{}", target.0)?;
                for arg in args {
                    write!(f, ", {arg}")?;
                }
                Ok(())
            }
        }
    }
}
