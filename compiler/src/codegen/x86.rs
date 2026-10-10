//! Typed x86-64 instructions, restricted to the LO subset (instruction_subset.md).
//!
//! The selector emits `Inst` values; `print.rs` is the only code that formats text.
//! The types make out-of-subset instructions unrepresentable (no SIMD, no unsigned
//! jumps, 8-bit registers only as `al`/`cl`), and `Inst::check` rejects operand
//! combinations x86 cannot encode, such as memory-to-memory moves.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reg {
    Rax,
    Rcx,
    Rdx,
    Rbx,
    Rsp,
    Rbp,
    Rsi,
    Rdi,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
}

/// The only byte registers the subset allows: `al` for `setcc`, `cl` for shift counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reg8 {
    Al,
    Cl,
}

/// LO `int` and `bool` are 32-bit; references, pointers and code pointers are 64-bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    W32,
    W64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mem {
    /// `[base + disp]`
    Base { base: Reg, disp: i32 },
    /// `[rip + symbol]`, required for globals, descriptors and string literals.
    Rip(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operand {
    Reg(Reg, Width),
    Imm(i64),
    Mem(Mem, Width),
}

/// Signed conditions only; the subset excludes unsigned jumps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cond {
    E,
    Ne,
    L,
    Le,
    G,
    Ge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alu {
    Add,
    Sub,
    And,
    Or,
    Xor,
    Cmp,
    Test,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unary {
    Neg,
    Not,
    Inc,
    Dec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    Shl,
    Shr,
    Sar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShiftCount {
    Imm(u8),
    Cl,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inst {
    Mov(Operand, Operand),
    /// `lea dst64, [addr]`
    Lea(Reg, Mem),
    Push(Operand),
    Pop(Operand),
    /// `movzx dst, src8`
    Movzx(Reg, Width, Reg8),
    /// `add`, `sub`, `and`, `or`, `xor`, `cmp`, `test`: `op dst, src`
    Alu(Alu, Operand, Operand),
    /// Two-operand `imul dst, src`.
    Imul(Reg, Width, Operand),
    /// Three-operand `imul dst, src, imm`.
    ImulImm(Reg, Width, Operand, i32),
    /// Signed divide of `edx:eax` (or `rdx:rax`) by the operand.
    Idiv(Operand),
    Cdq,
    Cqo,
    Unary(Unary, Operand),
    Shift(Shift, Operand, ShiftCount),
    Jcc(Cond, String),
    SetCc(Cond, Reg8),
    Jmp(String),
    JmpIndirect(Operand),
    Call(String),
    CallIndirect(Operand),
    Ret,
    Leave,
    Int3,
}

/// One line of an assembly file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Label(String),
    Inst(Inst),
    /// A verbatim assembler directive such as `.globl main`.
    Directive(String),
}

impl Operand {
    pub fn reg(reg: Reg, width: Width) -> Operand {
        Operand::Reg(reg, width)
    }

    pub fn r32(reg: Reg) -> Operand {
        Operand::Reg(reg, Width::W32)
    }

    pub fn r64(reg: Reg) -> Operand {
        Operand::Reg(reg, Width::W64)
    }

    pub fn imm(value: i64) -> Operand {
        Operand::Imm(value)
    }

    pub fn mem(base: Reg, disp: i32, width: Width) -> Operand {
        Operand::Mem(Mem::Base { base, disp }, width)
    }

    pub fn rip(symbol: impl Into<String>, width: Width) -> Operand {
        Operand::Mem(Mem::Rip(symbol.into()), width)
    }

    fn width(&self) -> Option<Width> {
        match self {
            Operand::Reg(_, w) | Operand::Mem(_, w) => Some(*w),
            Operand::Imm(_) => None,
        }
    }

    fn is_mem(&self) -> bool {
        matches!(self, Operand::Mem(..))
    }

    fn is_imm(&self) -> bool {
        matches!(self, Operand::Imm(_))
    }
}

fn fits_i32(value: i64) -> bool {
    i32::try_from(value).is_ok()
}

fn imm_ok(src: &Operand, dst_width: Width, allow_imm64: bool) -> Result<(), String> {
    match src {
        Operand::Imm(v) if !fits_i32(*v) && !(allow_imm64 && dst_width == Width::W64) => {
            Err(format!("immediate {v} does not fit in 32 bits"))
        }
        _ => Ok(()),
    }
}

fn same_width(a: &Operand, b: &Operand) -> Result<(), String> {
    match (a.width(), b.width()) {
        (Some(x), Some(y)) if x != y => Err(format!("operand widths differ: {x:?} vs {y:?}")),
        _ => Ok(()),
    }
}

fn not_imm(operand: &Operand, what: &str) -> Result<(), String> {
    if operand.is_imm() {
        Err(format!("{what} cannot be an immediate"))
    } else {
        Ok(())
    }
}

impl Inst {
    /// Rejects operand combinations x86 cannot encode. The selector's `src`/`dst`
    /// helpers are responsible for never producing these; this is the safety net.
    pub fn check(&self) -> Result<(), String> {
        match self {
            Inst::Mov(dst, src) => {
                not_imm(dst, "mov destination")?;
                if dst.is_mem() && src.is_mem() {
                    return Err("mov cannot have two memory operands".into());
                }
                same_width(dst, src)?;
                // `mov reg64, imm64` is the only form taking a 64-bit literal.
                let allow64 = !dst.is_mem();
                imm_ok(src, dst.width().unwrap_or(Width::W64), allow64)
            }
            Inst::Lea(_, Mem::Base { .. } | Mem::Rip(_)) => Ok(()),
            Inst::Push(src) => match src {
                Operand::Reg(_, Width::W32) | Operand::Mem(_, Width::W32) => {
                    Err("push takes a 64-bit operand".into())
                }
                _ => imm_ok(src, Width::W64, false),
            },
            Inst::Pop(dst) => {
                not_imm(dst, "pop destination")?;
                match dst.width() {
                    Some(Width::W64) => Ok(()),
                    _ => Err("pop takes a 64-bit operand".into()),
                }
            }
            Inst::Movzx(_, _, _) => Ok(()),
            Inst::Alu(op, dst, src) => {
                not_imm(dst, "first operand")?;
                if dst.is_mem() && src.is_mem() {
                    return Err(format!("{op:?} cannot have two memory operands"));
                }
                same_width(dst, src)?;
                imm_ok(src, dst.width().unwrap_or(Width::W64), false)
            }
            Inst::Imul(_, w, src) => {
                not_imm(src, "imul source")?;
                match src.width() {
                    Some(sw) if sw != *w => Err("imul operand widths differ".into()),
                    _ => Ok(()),
                }
            }
            Inst::ImulImm(_, w, src, _) => {
                not_imm(src, "imul source")?;
                match src.width() {
                    Some(sw) if sw != *w => Err("imul operand widths differ".into()),
                    _ => Ok(()),
                }
            }
            Inst::Idiv(src) => not_imm(src, "idiv operand"),
            Inst::Unary(_, dst) => not_imm(dst, "operand"),
            Inst::Shift(_, dst, _) => not_imm(dst, "shift destination"),
            Inst::JmpIndirect(target) | Inst::CallIndirect(target) => match target {
                Operand::Reg(_, Width::W64) | Operand::Mem(_, Width::W64) => Ok(()),
                _ => Err("indirect jump/call target must be a 64-bit register or memory".into()),
            },
            Inst::Cdq
            | Inst::Cqo
            | Inst::Jcc(..)
            | Inst::SetCc(..)
            | Inst::Jmp(_)
            | Inst::Call(_)
            | Inst::Ret
            | Inst::Leave
            | Inst::Int3 => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Reg::*;

    #[test]
    fn memory_to_memory_is_rejected() {
        let both_mem = Inst::Mov(
            Operand::mem(Rbp, -8, Width::W32),
            Operand::mem(Rbp, -16, Width::W32),
        );
        assert!(both_mem.check().is_err());
        let cmp = Inst::Alu(
            Alu::Cmp,
            Operand::mem(Rbp, -8, Width::W32),
            Operand::mem(Rbp, -16, Width::W32),
        );
        assert!(cmp.check().is_err());
    }

    #[test]
    fn width_mismatch_and_immediate_destination_are_rejected() {
        let wide = Inst::Mov(Operand::r32(Rax), Operand::r64(Rbx));
        assert!(wide.check().is_err());
        let imm_dst = Inst::Mov(Operand::imm(1), Operand::r32(Rax));
        assert!(imm_dst.check().is_err());
    }

    #[test]
    fn only_register_destination_takes_a_64_bit_literal() {
        let big = 1_i64 << 40;
        assert!(Inst::Mov(Operand::r64(Rax), Operand::imm(big))
            .check()
            .is_ok());
        assert!(
            Inst::Mov(Operand::mem(Rbp, -8, Width::W64), Operand::imm(big))
                .check()
                .is_err()
        );
        assert!(Inst::Alu(Alu::Add, Operand::r64(Rax), Operand::imm(big))
            .check()
            .is_err());
    }

    #[test]
    fn well_formed_instructions_pass() {
        let ok = [
            Inst::Mov(Operand::r32(Rax), Operand::mem(Rbp, -8, Width::W32)),
            Inst::Mov(Operand::mem(Rbp, -8, Width::W32), Operand::imm(0)),
            Inst::Alu(Alu::Cmp, Operand::mem(Rbp, -8, Width::W32), Operand::imm(0)),
            Inst::Imul(Rax, Width::W32, Operand::mem(Rbp, -8, Width::W32)),
            Inst::Idiv(Operand::r32(R10)),
            Inst::Push(Operand::r64(Rbp)),
            Inst::CallIndirect(Operand::r64(R11)),
        ];
        for inst in ok {
            assert_eq!(inst.check(), Ok(()), "{inst:?}");
        }
    }

    #[test]
    fn push_pop_and_indirect_targets_must_be_64_bit() {
        assert!(Inst::Push(Operand::r32(Rax)).check().is_err());
        assert!(Inst::Pop(Operand::r32(Rax)).check().is_err());
        assert!(Inst::CallIndirect(Operand::r32(R11)).check().is_err());
    }
}
