//! Intel-syntax text for `x86::Inst`. The only code in the back end that formats assembly.
//!
//! Output assumes `.intel_syntax noprefix`, which `file` emits as its header.

use super::x86::{Alu, Cond, Inst, Item, Mem, Operand, Reg, Reg8, Shift, ShiftCount, Unary, Width};

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
        Inst::Cqo => "cqo".into(),
        Inst::Unary(op, dst) => format!("{} {}", unary_name(*op), operand(dst)),
        Inst::Shift(op, dst, count) => {
            let count = match count {
                ShiftCount::Imm(n) => n.to_string(),
                ShiftCount::Cl => reg8_name(Reg8::Cl).into(),
            };
            format!("{} {}, {count}", shift_name(*op), operand(dst))
        }
        Inst::Jcc(cond, label) => format!("j{} {label}", cond_suffix(*cond)),
        Inst::SetCc(cond, dst) => format!("set{} {}", cond_suffix(*cond), reg8_name(*dst)),
        Inst::Jmp(label) => format!("jmp {label}"),
        Inst::JmpIndirect(target) => format!("jmp {}", operand(target)),
        Inst::Call(symbol) => format!("call {symbol}"),
        Inst::CallIndirect(target) => format!("call {}", operand(target)),
        Inst::Ret => "ret".into(),
        Inst::Leave => "leave".into(),
        Inst::Int3 => "int3".into(),
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
        Reg8::Cl => "cl",
    }
}

fn alu_name(op: Alu) -> &'static str {
    match op {
        Alu::Add => "add",
        Alu::Sub => "sub",
        Alu::And => "and",
        Alu::Or => "or",
        Alu::Xor => "xor",
        Alu::Cmp => "cmp",
        Alu::Test => "test",
    }
}

fn unary_name(op: Unary) -> &'static str {
    match op {
        Unary::Neg => "neg",
        Unary::Not => "not",
        Unary::Inc => "inc",
        Unary::Dec => "dec",
    }
}

fn shift_name(op: Shift) -> &'static str {
    match op {
        Shift::Shl => "shl",
        Shift::Shr => "shr",
        Shift::Sar => "sar",
    }
}

fn cond_suffix(cond: Cond) -> &'static str {
    match cond {
        Cond::E => "e",
        Cond::Ne => "ne",
        Cond::L => "l",
        Cond::Le => "le",
        Cond::G => "g",
        Cond::Ge => "ge",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Reg::*;

    fn text(inst: Inst) -> String {
        inst_text(&inst)
    }

    #[test]
    fn moves_and_addresses() {
        assert_eq!(
            text(Inst::Mov(
                Operand::r32(Rax),
                Operand::mem(Rbp, -8, Width::W32)
            )),
            "mov eax, dword ptr [rbp - 8]"
        );
        assert_eq!(
            text(Inst::Mov(
                Operand::mem(Rbp, 16, Width::W64),
                Operand::r64(Rdi)
            )),
            "mov qword ptr [rbp + 16], rdi"
        );
        assert_eq!(
            text(Inst::Mov(
                Operand::r64(R10),
                Operand::mem(Rax, 0, Width::W64)
            )),
            "mov r10, qword ptr [rax]"
        );
        assert_eq!(
            text(Inst::Mov(Operand::r32(R11), Operand::imm(-1))),
            "mov r11d, -1"
        );
        assert_eq!(
            text(Inst::Lea(Rax, Mem::Rip("lo_class_4_Main".into()))),
            "lea rax, [rip + lo_class_4_Main]"
        );
        assert_eq!(
            text(Inst::Lea(
                Rdi,
                Mem::Base {
                    base: Rbp,
                    disp: -48
                }
            )),
            "lea rdi, [rbp - 48]"
        );
    }

    #[test]
    fn arithmetic_and_compare() {
        assert_eq!(
            text(Inst::Alu(
                Alu::Add,
                Operand::r32(Rax),
                Operand::mem(Rbp, -16, Width::W32)
            )),
            "add eax, dword ptr [rbp - 16]"
        );
        assert_eq!(
            text(Inst::Alu(
                Alu::Cmp,
                Operand::mem(Rbp, -8, Width::W32),
                Operand::imm(0)
            )),
            "cmp dword ptr [rbp - 8], 0"
        );
        assert_eq!(
            text(Inst::Alu(Alu::Xor, Operand::r32(Rax), Operand::imm(1))),
            "xor eax, 1"
        );
        assert_eq!(
            text(Inst::Imul(Rax, Width::W32, Operand::r32(R10))),
            "imul eax, r10d"
        );
        assert_eq!(
            text(Inst::ImulImm(
                Rax,
                Width::W32,
                Operand::mem(Rbp, -8, Width::W32),
                12
            )),
            "imul eax, dword ptr [rbp - 8], 12"
        );
        assert_eq!(text(Inst::Cdq), "cdq");
        assert_eq!(text(Inst::Cqo), "cqo");
        assert_eq!(text(Inst::Idiv(Operand::r32(R10))), "idiv r10d");
        assert_eq!(text(Inst::Unary(Unary::Neg, Operand::r32(Rax))), "neg eax");
        assert_eq!(text(Inst::Unary(Unary::Not, Operand::r32(Rax))), "not eax");
    }

    #[test]
    fn shifts_flags_and_branches() {
        assert_eq!(
            text(Inst::Shift(
                Shift::Sar,
                Operand::r32(Rax),
                ShiftCount::Imm(31)
            )),
            "sar eax, 31"
        );
        assert_eq!(
            text(Inst::Shift(Shift::Shl, Operand::r32(Rax), ShiftCount::Cl)),
            "shl eax, cl"
        );
        assert_eq!(text(Inst::SetCc(Cond::L, Reg8::Al)), "setl al");
        assert_eq!(
            text(Inst::Movzx(Rax, Width::W32, Reg8::Al)),
            "movzx eax, al"
        );
        assert_eq!(text(Inst::Jcc(Cond::Ne, ".L_f_b2".into())), "jne .L_f_b2");
        assert_eq!(text(Inst::Jcc(Cond::Ge, ".L_f_b3".into())), "jge .L_f_b3");
        assert_eq!(text(Inst::Jmp(".L_f_b1".into())), "jmp .L_f_b1");
    }

    #[test]
    fn calls_and_frames() {
        assert_eq!(text(Inst::Call("lo_alloc".into())), "call lo_alloc");
        assert_eq!(text(Inst::CallIndirect(Operand::r64(R11))), "call r11");
        assert_eq!(
            text(Inst::JmpIndirect(Operand::mem(Rax, 8, Width::W64))),
            "jmp qword ptr [rax + 8]"
        );
        assert_eq!(text(Inst::Push(Operand::r64(Rbp))), "push rbp");
        assert_eq!(text(Inst::Pop(Operand::r64(Rbp))), "pop rbp");
        assert_eq!(text(Inst::Leave), "leave");
        assert_eq!(text(Inst::Ret), "ret");
        assert_eq!(text(Inst::Int3), "int3");
    }

    #[test]
    fn file_has_intel_header_labels_and_indented_instructions() {
        let items = [
            Item::Directive(".globl main".into()),
            Item::Label("main".into()),
            Item::Inst(Inst::Push(Operand::r64(Rbp))),
            Item::Inst(Inst::Ret),
        ];
        assert_eq!(
            file(&items),
            ".intel_syntax noprefix\n.text\n.globl main\nmain:\n    push rbp\n    ret\n"
        );
    }
}
