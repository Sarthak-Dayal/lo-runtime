//! Stack frame layout, prologue and epilogue for one function.
//!
//! The selector never computes a stack offset; it asks `Frame`. That keeps the
//! spill-everything and linear-scan allocators interchangeable: both produce a
//! `FunctionAllocation`, and `Frame` reads only its spill-slot count and the
//! callee-saved registers it used.
//!
//! ```text
//! [rbp + 16 + 8k]  stack argument k (arguments 7 and up)
//! [rbp + 8]        return address
//! [rbp]            saved rbp
//! [rbp - 8(i+1)]   saved callee-saved register i
//!                  shadow frame: parent, num_roots, roots[] (only if the function calls)
//!                  spill slots, 8 bytes each
//!                  ret_save (only if the function calls)
//!                  padding so rsp stays 16-byte aligned
//! ```

use crate::ir::register_allocator::{
    FunctionAllocation, PhysicalLocation, PhysicalRegId, SpillSlotId,
};
use crate::ir::{FunctionIr, IrType};
use crate::layout::{FrameLayout as ShadowLayout, NATIVE};

use super::moves::{parallel_move, Move};
use super::x86::{Alu, Inst, Mem, Operand, Reg, Width};

/// System V AMD64 integer argument registers, in order.
pub const ARGUMENT_REGISTERS: [Reg; 6] = [Reg::Rdi, Reg::Rsi, Reg::Rdx, Reg::Rcx, Reg::R8, Reg::R9];

const SLOT: i32 = 8;
const STACK_ALIGN: i32 = 16;
/// `[rbp + 16]` holds the first stack-passed argument (after saved rbp and return address).
const FIRST_STACK_ARGUMENT: i32 = 16;

/// The allocator's register names live in `ir` so it does not depend on the back
/// end; this maps them to x86 registers.
pub fn x86_reg(id: PhysicalRegId) -> Reg {
    use PhysicalRegId as P;
    match id {
        P::Rax => Reg::Rax,
        P::Rcx => Reg::Rcx,
        P::Rdx => Reg::Rdx,
        P::Rbx => Reg::Rbx,
        P::Rsp => Reg::Rsp,
        P::Rbp => Reg::Rbp,
        P::Rsi => Reg::Rsi,
        P::Rdi => Reg::Rdi,
        P::R8 => Reg::R8,
        P::R9 => Reg::R9,
        P::R10 => Reg::R10,
        P::R11 => Reg::R11,
        P::R12 => Reg::R12,
        P::R13 => Reg::R13,
        P::R14 => Reg::R14,
        P::R15 => Reg::R15,
    }
}

pub fn width_of(ty: IrType) -> Width {
    match ty {
        IrType::Int32 | IrType::Bool => Width::W32,
        IrType::Ref | IrType::Ptr | IrType::CodePtr(_) => Width::W64,
    }
}

/// An incoming parameter: its type and where the allocation wants it.
pub struct Param {
    pub ty: IrType,
    pub dest: Operand,
}

pub struct Frame {
    saved: Vec<Reg>,
    /// Present only in functions that contain a `Call`: they register a shadow
    /// frame (even with zero roots); leaf functions do not.
    shadow: Option<Shadow>,
    spill_start: i32,
    spill_slots: usize,
    ret_save: Option<i32>,
    /// Bytes subtracted from `rsp` after `push rbp ; mov rbp, rsp`. rsp is
    /// 16-aligned after `push rbp`, and this is a multiple of 16, so every call in
    /// the body starts aligned (System V §3.2.2).
    size: i32,
}

struct Shadow {
    layout: ShadowLayout,
    /// `rbp`-relative address of the frame's lowest byte (its `parent` field).
    base: i32,
    roots: u32,
}

impl Frame {
    /// The frame of `function` (after `insert_gc_roots`) under `allocation`.
    pub fn new(function: &FunctionIr, allocation: &FunctionAllocation) -> Result<Frame, String> {
        let has_calls = function.has_calls();
        let root_slots = function.root_slots;
        if !has_calls && root_slots > 0 {
            return Err(format!(
                "leaf function has {root_slots} root slots; only functions with calls have a shadow frame"
            ));
        }
        let saved: Vec<Reg> = allocation
            .used_callee_saved_registers
            .iter()
            .copied()
            .map(x86_reg)
            .collect();
        let mut used = SLOT * saved.len() as i32;

        let shadow = has_calls.then(|| {
            let layout = NATIVE.frame();
            used += layout.size(root_slots) as i32;
            Shadow {
                layout,
                base: -used,
                roots: root_slots,
            }
        });

        let spill_start = used;
        used += SLOT * allocation.spill_slot_count as i32;

        let ret_save = has_calls.then(|| {
            used += SLOT;
            -used
        });

        Ok(Frame {
            saved,
            shadow,
            spill_start,
            spill_slots: allocation.spill_slot_count,
            ret_save,
            size: round_up(used, STACK_ALIGN),
        })
    }

    /// Bytes reserved below the saved `rbp` (always a multiple of 16).
    pub fn size(&self) -> i32 {
        self.size
    }

    /// Where callee-saved register `i` is kept, in the prologue and every epilogue.
    fn saved_register_slot(i: usize) -> Operand {
        Operand::Mem(Self::at(-SLOT * (i as i32 + 1)), Width::W64)
    }

    fn at(disp: i32) -> Mem {
        Mem::Base {
            base: Reg::Rbp,
            disp,
        }
    }

    pub fn spill(&self, slot: SpillSlotId) -> Mem {
        assert!(
            slot.0 < self.spill_slots,
            "spill slot {} out of range",
            slot.0
        );
        Self::at(-(self.spill_start + SLOT * (slot.0 as i32 + 1)))
    }

    /// Address of shadow-stack root slot `slot`.
    pub fn root(&self, slot: u32) -> Mem {
        let shadow = self.shadow.as_ref().expect("root slot in a leaf function");
        assert!(slot < shadow.roots, "root slot {slot} out of range");
        Self::at(shadow.base + shadow.layout.root(slot) as i32)
    }

    /// The operand for an allocation result: a register, or a spill slot in memory.
    pub fn location(&self, location: PhysicalLocation, width: Width) -> Operand {
        match location {
            PhysicalLocation::Register(id) => Operand::Reg(x86_reg(id), width),
            PhysicalLocation::Spill(slot) => Operand::Mem(self.spill(slot), width),
        }
    }

    /// `push rbp ; mov rbp, rsp ; sub rsp, N`, then saves the callee-saved
    /// registers, then moves parameters to their locations, then builds and
    /// registers the shadow frame. Parameters move first because `lo_push_frame`
    /// is a call and may overwrite the argument registers. The allocator matches
    /// this by treating every parameter of a function with calls as surviving a
    /// call. Root slots start null so a collection never reads a stale pointer;
    /// `parent` is filled in by `lo_push_frame`.
    pub fn prologue(&self, params: &[Param]) -> Result<Vec<Inst>, String> {
        let mut out = vec![
            Inst::Push(Operand::r64(Reg::Rbp)),
            Inst::Mov(Operand::r64(Reg::Rbp), Operand::r64(Reg::Rsp)),
        ];
        if self.size > 0 {
            out.push(Inst::Alu(
                Alu::Sub,
                Operand::r64(Reg::Rsp),
                Operand::imm(i64::from(self.size)),
            ));
        }
        for (i, saved) in self.saved.iter().enumerate() {
            out.push(Inst::Mov(
                Self::saved_register_slot(i),
                Operand::r64(*saved),
            ));
        }
        self.move_params(params, &mut out)?;
        if let Some(shadow) = &self.shadow {
            let at = |offset: u32| Operand::Mem(Self::at(shadow.base + offset as i32), Width::W64);
            out.push(Inst::Mov(at(shadow.layout.parent), Operand::imm(0)));
            out.push(Inst::Mov(
                at(shadow.layout.num_roots),
                Operand::imm(i64::from(shadow.roots)),
            ));
            for slot in 0..shadow.roots {
                out.push(Inst::Mov(at(shadow.layout.root(slot)), Operand::imm(0)));
            }
            out.push(Inst::Lea(Reg::Rdi, Self::at(shadow.base)));
            out.push(Inst::Call("lo_push_frame".into()));
        }
        Ok(out)
    }

    /// Moves incoming parameters to their allocated locations, in three phases so
    /// nothing is overwritten before it is read:
    ///   1. stores to memory (they only read argument registers, and stack
    ///      arguments to memory go through rax, which is no one's source);
    ///   2. register-to-register moves as one parallel move (they may form cycles
    ///      when a leaf function allocates rsi/rdi/r8/r9, which are also the
    ///      incoming argument registers);
    ///   3. stack-passed arguments into registers (memory sources are never
    ///      clobbered, but their destinations may be argument registers still
    ///      needed in phase 2).
    fn move_params(&self, params: &[Param], out: &mut Vec<Inst>) -> Result<(), String> {
        let mut register_moves = vec![];
        let mut stack_loads = vec![];
        for (i, param) in params.iter().enumerate() {
            let width = width_of(param.ty);
            let stack_source = || {
                Operand::Mem(
                    Self::at(FIRST_STACK_ARGUMENT + SLOT * (i - ARGUMENT_REGISTERS.len()) as i32),
                    width,
                )
            };
            match (&param.dest, ARGUMENT_REGISTERS.get(i)) {
                (Operand::Mem(..), Some(arg)) => {
                    out.push(Inst::Mov(param.dest.clone(), Operand::Reg(*arg, width)));
                }
                (Operand::Mem(..), None) => {
                    out.push(Inst::Mov(Operand::Reg(Reg::Rax, width), stack_source()));
                    out.push(Inst::Mov(param.dest.clone(), Operand::Reg(Reg::Rax, width)));
                }
                (Operand::Reg(dst, _), Some(arg)) => register_moves.push(Move {
                    dst: *dst,
                    src: *arg,
                    width,
                }),
                (Operand::Reg(dst, _), None) => {
                    stack_loads.push(Inst::Mov(Operand::Reg(*dst, width), stack_source()));
                }
                (Operand::Imm(_), _) => {
                    return Err(format!("parameter {i}: destination cannot be an immediate"))
                }
            }
        }
        out.extend(parallel_move(&register_moves)?);
        out.extend(stack_loads);
        Ok(())
    }

    /// Everything from "return value is in `rax`" to `ret`. `lo_pop_frame` is a call
    /// and clobbers `rax`, so a value return is parked in `ret_save` across it.
    pub fn epilogue(&self, returns_value: bool) -> Vec<Inst> {
        let mut out = vec![];
        if let Some(ret_save) = self.ret_save {
            let slot = Operand::Mem(Self::at(ret_save), Width::W64);
            if returns_value {
                out.push(Inst::Mov(slot.clone(), Operand::r64(Reg::Rax)));
            }
            out.push(Inst::Call("lo_pop_frame".into()));
            if returns_value {
                out.push(Inst::Mov(Operand::r64(Reg::Rax), slot));
            }
        }
        for (i, saved) in self.saved.iter().enumerate() {
            out.push(Inst::Mov(
                Operand::r64(*saved),
                Self::saved_register_slot(i),
            ));
        }
        out.push(Inst::Leave);
        out.push(Inst::Ret);
        out
    }
}

fn round_up(value: i32, multiple: i32) -> i32 {
    (value + multiple - 1) / multiple * multiple
}
