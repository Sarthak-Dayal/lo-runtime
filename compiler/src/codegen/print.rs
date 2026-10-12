//! Intel-syntax text for `x86::Inst`. The only code in the back end that formats assembly.
//!
//! Output assumes `.intel_syntax noprefix`, which `file` emits as its header.

use super::x86::{Alu, Cond, Inst, Item, Mem, Operand, Reg, Reg8, Unary, Width};

/// The assembly file preamble: Intel syntax without register prefixes, then code.
pub const HEADER: &str = ".intel_syntax noprefix\n.text\n";

/// Renders a whole file: header, then one line per item.
pub fn file(items: &[Item]) -> String {
    let mut out = String::from(HEADER);
    for item in items {
        out.push_str(&item_text(item));
        out.push('\n');
    }
    out
}

pub fn item_text(item: &Item) -> String {
    match item {
        Item::Label(name) => format!("{name}:"),
        Item::Inst(inst) => format!("    {}", inst_text(inst)),
        Item::Directive(text) => text.clone(),
    }
}

pub fn inst_text(inst: &Inst) -> String {
    match inst {
        Inst::Mov(dst, src) => binary("mov", dst, src),
        // `lea` computes an address and never touches memory, so no size prefix.
        Inst::Lea(dst, src) => format!("lea {}, {}", reg_name(*dst, Width::W64), address(src)),
        Inst::Push(src) => format!("push {}", operand(src)),
        Inst::Pop(dst) => format!("pop {}", operand(dst)),
        Inst::Movzx(dst, width, src) => {
            format!("movzx {}, {}", reg_name(*dst, *width), reg8_name(*src))
        }
        Inst::Alu(op, dst, src) => binary(alu_name(*op), dst, src),
        Inst::Imul(dst, width, src) => {
            format!("imul {}, {}", reg_name(*dst, *width), operand(src))
        }
        Inst::ImulImm(dst, width, src, imm) => {
            format!("imul {}, {}, {imm}", reg_name(*dst, *width), operand(src))
        }
        Inst::Idiv(src) => format!("idiv {}", operand(src)),
        Inst::Cdq => "cdq".into(),
        Inst::Unary(op, dst) => format!("{} {}", unary_name(*op), operand(dst)),
        Inst::Jcc(cond, label) => format!("j{} {label}", cond_suffix(*cond)),
        Inst::SetCc(cond, dst) => format!("set{} {}", cond_suffix(*cond), reg8_name(*dst)),
        Inst::Jmp(label) => format!("jmp {label}"),
        Inst::Call(symbol) => format!("call {symbol}"),
        Inst::CallIndirect(target) => format!("call {}", operand(target)),
        Inst::Ret => "ret".into(),
        Inst::Leave => "leave".into(),
    }
}

fn binary(mnemonic: &str, dst: &Operand, src: &Operand) -> String {
    format!("{mnemonic} {}, {}", operand(dst), operand(src))
}

fn operand(operand: &Operand) -> String {
    match operand {
        Operand::Reg(reg, width) => reg_name(*reg, *width).into(),
        Operand::Imm(value) => value.to_string(),
        Operand::Mem(mem, width) => {
            let size = match width {
                Width::W32 => "dword",
                Width::W64 => "qword",
            };
            format!("{size} ptr {}", address(mem))
        }
    }
}

fn address(mem: &Mem) -> String {
    match mem {
        Mem::Base { base, disp } => {
            let base = reg_name(*base, Width::W64);
            match *disp {
                0 => format!("[{base}]"),
                d if d < 0 => format!("[{base} - {}]", -i64::from(d)),
                d => format!("[{base} + {d}]"),
            }
        }
        Mem::Rip(symbol) => format!("[rip + {symbol}]"),
    }
}

fn reg_name(reg: Reg, width: Width) -> &'static str {
    use Reg::*;
    match (reg, width) {
        (Rax, Width::W64) => "rax",
        (Rax, Width::W32) => "eax",
        (Rcx, Width::W64) => "rcx",
        (Rcx, Width::W32) => "ecx",
        (Rdx, Width::W64) => "rdx",
        (Rdx, Width::W32) => "edx",
        (Rbx, Width::W64) => "rbx",
        (Rbx, Width::W32) => "ebx",
        (Rsp, Width::W64) => "rsp",
        (Rsp, Width::W32) => "esp",
        (Rbp, Width::W64) => "rbp",
        (Rbp, Width::W32) => "ebp",
        (Rsi, Width::W64) => "rsi",
        (Rsi, Width::W32) => "esi",
        (Rdi, Width::W64) => "rdi",
        (Rdi, Width::W32) => "edi",
        (R8, Width::W64) => "r8",
        (R8, Width::W32) => "r8d",
        (R9, Width::W64) => "r9",
        (R9, Width::W32) => "r9d",
        (R10, Width::W64) => "r10",
        (R10, Width::W32) => "r10d",
        (R11, Width::W64) => "r11",
        (R11, Width::W32) => "r11d",
        (R12, Width::W64) => "r12",
        (R12, Width::W32) => "r12d",
        (R13, Width::W64) => "r13",
        (R13, Width::W32) => "r13d",
        (R14, Width::W64) => "r14",
        (R14, Width::W32) => "r14d",
        (R15, Width::W64) => "r15",
        (R15, Width::W32) => "r15d",
    }
}

fn reg8_name(reg: Reg8) -> &'static str {
    match reg {
        Reg8::Al => "al",
    }
}

fn alu_name(op: Alu) -> &'static str {
    match op {
        Alu::Add => "add",
        Alu::Sub => "sub",
        Alu::Xor => "xor",
        Alu::Cmp => "cmp",
    }
}

fn unary_name(op: Unary) -> &'static str {
    match op {
        Unary::Neg => "neg",
    }
}

fn cond_suffix(cond: Cond) -> &'static str {
    match cond {
        Cond::E => "e",
        Cond::Ne => "ne",
        Cond::L => "l",
        Cond::G => "g",
    }
}
