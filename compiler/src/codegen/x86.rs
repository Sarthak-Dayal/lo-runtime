//! Typed x86-64 instructions, restricted to the LO subset (language reference,
//! chapter 8, "LO Architecture Reference").
//!
//! The selector emits `Inst` values; `print.rs` is the only code that formats text.
//! Only instructions the selector emits are modeled, and the types rule out most
//! out-of-subset forms (no SIMD, no unsigned jumps, `al` as the only byte
//! register). `Inst::check` rejects the rest: operand forms the subset excludes,
//! such as `push` from memory, and combinations x86 cannot encode, such as
//! memory-to-memory moves.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

/// The only byte register the selector uses: `al`, written by `setcc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reg8 {
    Al,
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
    G,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alu {
    Add,
    Sub,
    Xor,
    Cmp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unary {
    Neg,
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
    /// `add`, `sub`, `xor`, `cmp`: `op dst, src`
    Alu(Alu, Operand, Operand),
    /// Two-operand `imul dst, src`.
    Imul(Reg, Width, Operand),
    /// Three-operand `imul dst, src, imm`.
    ImulImm(Reg, Width, Operand, i32),
    /// Signed divide of `edx:eax` (or `rdx:rax`) by the operand.
    Idiv(Operand),
    Cdq,
    Unary(Unary, Operand),
    Jcc(Cond, String),
    SetCc(Cond, Reg8),
    Jmp(String),
    Call(String),
    CallIndirect(Operand),
    Ret,
    Leave,
}

/// One line of an assembly file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Label(String),
    Inst(Inst),
    /// A verbatim assembler directive such as `.globl main`.
    Directive(String),
    /// An empty line between sections of the file.
    Blank,
}

impl Operand {
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

fn check_immediate_fits(src: &Operand, dst_width: Width, allow_imm64: bool) -> Result<(), String> {
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
                check_immediate_fits(src, dst.width().unwrap_or(Width::W64), allow64)
            }
            Inst::Push(src) => match src {
                Operand::Mem(..) => Err("push takes a register or an immediate".into()),
                Operand::Reg(_, Width::W32) => Err("push takes a 64-bit operand".into()),
                _ => check_immediate_fits(src, Width::W64, false),
            },
            Inst::Pop(dst) => match dst {
                Operand::Reg(_, Width::W64) => Ok(()),
                _ => Err("pop takes a 64-bit register".into()),
            },
            Inst::Movzx(_, _, _) => Ok(()),
            Inst::Alu(op, dst, src) => {
                not_imm(dst, "first operand")?;
                if dst.is_mem() && src.is_mem() {
                    return Err(format!("{op:?} cannot have two memory operands"));
                }
                same_width(dst, src)?;
                check_immediate_fits(src, dst.width().unwrap_or(Width::W64), false)
            }
            Inst::Imul(_, w, src) | Inst::ImulImm(_, w, src, _) => {
                not_imm(src, "imul source")?;
                match src.width() {
                    Some(sw) if sw != *w => Err("imul operand widths differ".into()),
                    _ => Ok(()),
                }
            }
            Inst::Idiv(src) => not_imm(src, "idiv operand"),
            Inst::Unary(_, dst) => not_imm(dst, "operand"),
            Inst::CallIndirect(target) => match target {
                Operand::Reg(_, Width::W64) | Operand::Mem(_, Width::W64) => Ok(()),
                _ => Err("indirect call target must be a 64-bit register or memory".into()),
            },
            Inst::Lea(..)
            | Inst::Cdq
            | Inst::Jcc(..)
            | Inst::SetCc(..)
            | Inst::Jmp(_)
            | Inst::Call(_)
            | Inst::Ret
            | Inst::Leave => Ok(()),
        }
    }
}

/// Runs `Inst::check` on every instruction in `items`, naming `function` in the error.
pub fn check_instructions(function: &str, items: &[Item]) -> Result<(), String> {
    for item in items {
        if let Item::Inst(inst) = item {
            inst.check()
                .map_err(|error| format!("{function}: {error}: {inst:?}"))?;
        }
    }
    Ok(())
}
