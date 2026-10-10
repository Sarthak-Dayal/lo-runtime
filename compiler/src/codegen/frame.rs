//! Stack frame layout, prologue and epilogue for one function.
//!
//! The selector never computes a stack offset; it asks `Frame`. That keeps the
//! spill-everything and linear-scan allocators interchangeable, since both hand
//! `Frame` only a spill-slot count and the callee-saved registers they used.
//! See planning/p2_docs/p2-codegen-frame-and-shadow-stack.md.
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
use crate::ir::{FunctionIr, InstructionKind, IrType};
use crate::layout::{FrameLayout as ShadowLayout, NATIVE};

use super::x86::{Alu, Inst, Mem, Operand, Reg, Width};

/// System V AMD64 integer argument registers, in order.
pub const ARGUMENT_REGISTERS: [Reg; 6] = [Reg::Rdi, Reg::Rsi, Reg::Rdx, Reg::Rcx, Reg::R8, Reg::R9];

const SLOT: i32 = 8;
const STACK_ALIGN: i32 = 16;
/// `[rbp + 16]` holds the first stack-passed argument (after saved rbp and return address).
const FIRST_STACK_ARGUMENT: i32 = 16;

pub fn reg(id: PhysicalRegId) -> Reg {
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

/// `Int32` and `Bool` are 32-bit; references, pointers and code pointers are 64-bit.
pub fn width_of(ty: IrType) -> Width {
    match ty {
        IrType::Int32 | IrType::Bool => Width::W32,
        IrType::Ref | IrType::Ptr | IrType::CodePtr(_) => Width::W64,
    }
}

/// What the allocation and the function body require of the frame.
pub struct FrameParams {
    /// Shadow-stack root slots (`FunctionIr.root_slots`, after `insert_gc_roots`).
    pub root_slots: u32,
    /// From `FunctionAllocation.spill_slot_count`.
    pub spill_slot_count: usize,
    /// From `FunctionAllocation.used_callee_saved_registers`.
    pub callee_saved: Vec<PhysicalRegId>,
    /// True if the function contains a `Call`. Such functions register a shadow
    /// frame (even with zero roots); leaf functions do not.
    pub has_calls: bool,
    /// True for the startup function: it must call `lo_runtime_init` before any
    /// frame is registered, so registration is emitted separately by the selector
    /// via `Frame::register_shadow`.
    pub defer_registration: bool,
}

impl FrameParams {
    /// Frame requirements of a function after `insert_gc_roots` and allocation.
    /// `is_startup` is `function.symbol == ProgramIr.startup`.
    pub fn for_function(
        function: &FunctionIr,
        allocation: &FunctionAllocation,
        is_startup: bool,
    ) -> FrameParams {
        // `Abort` is not a safepoint (gc_roots.rs), so an abort-only function is a leaf.
        let has_calls = function
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(instruction.kind, InstructionKind::Call { .. }));
        FrameParams {
            root_slots: function.root_slots,
            spill_slot_count: allocation.spill_slot_count,
            callee_saved: allocation.used_callee_saved_registers.clone(),
            has_calls,
            defer_registration: is_startup && has_calls,
        }
    }
}

/// An incoming parameter: its type and where the allocation wants it.
pub struct Param {
    pub ty: IrType,
    pub dest: Operand,
}

pub struct Frame {
    saved: Vec<Reg>,
    shadow: Option<Shadow>,
    spill_start: i32,
    spill_slots: usize,
    ret_save: Option<i32>,
    /// Bytes subtracted from `rsp` after `push rbp ; mov rbp, rsp`.
    size: i32,
    defer_registration: bool,
}

struct Shadow {
    layout: ShadowLayout,
    /// `rbp`-relative address of the frame's lowest byte (its `parent` field).
    base: i32,
    roots: u32,
}

impl Frame {
    pub fn new(params: FrameParams) -> Result<Frame, String> {
        if !params.has_calls && params.root_slots > 0 {
            return Err(format!(
                "leaf function has {} root slots; only functions with calls have a shadow frame",
                params.root_slots
            ));
        }
        let saved: Vec<Reg> = params.callee_saved.iter().copied().map(reg).collect();
        let mut used = SLOT * saved.len() as i32;

        let shadow = params.has_calls.then(|| {
            let layout = NATIVE.frame();
            used += layout.size(params.root_slots) as i32;
            Shadow {
                layout,
                base: -used,
                roots: params.root_slots,
            }
        });

        let spill_start = used;
        used += SLOT * params.spill_slot_count as i32;

        let ret_save = params.has_calls.then(|| {
            used += SLOT;
            -used
        });

        Ok(Frame {
            saved,
            shadow,
            spill_start,
            spill_slots: params.spill_slot_count,
            ret_save,
            size: (used + STACK_ALIGN - 1) / STACK_ALIGN * STACK_ALIGN,
            defer_registration: params.defer_registration,
        })
    }

    /// Bytes reserved below the saved `rbp` (always a multiple of 16).
    pub fn size(&self) -> i32 {
        self.size
    }

    pub fn has_shadow_frame(&self) -> bool {
        self.shadow.is_some()
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

    fn shadow_base(&self) -> Mem {
        Self::at(self.shadow.as_ref().expect("no shadow frame").base)
    }

    /// The operand for an allocation result: a register, or a spill slot in memory.
    pub fn location(&self, location: PhysicalLocation, width: Width) -> Operand {
        match location {
            PhysicalLocation::Register(id) => Operand::Reg(reg(id), width),
            PhysicalLocation::Spill(slot) => Operand::Mem(self.spill(slot), width),
        }
    }

    /// `push rbp ; mov rbp, rsp ; sub rsp, N`, saves callee-saved registers, moves
    /// parameters to their locations, then builds (and, unless deferred, registers)
    /// the shadow frame.
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
                Operand::Mem(Self::at(-SLOT * (i as i32 + 1)), Width::W64),
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
            if !self.defer_registration {
                out.extend(self.register_shadow());
            }
        }
        Ok(out)
    }

    /// `lea rdi, [frame] ; call lo_push_frame`. For the startup function the
    /// selector emits this right after its `call lo_runtime_init`.
    pub fn register_shadow(&self) -> Vec<Inst> {
        vec![
            Inst::Lea(Reg::Rdi, self.shadow_base()),
            Inst::Call("lo_push_frame".into()),
        ]
    }

    fn move_params(&self, params: &[Param], out: &mut Vec<Inst>) -> Result<(), String> {
        for (i, param) in params.iter().enumerate() {
            let width = width_of(param.ty);
            let Operand::Mem(..) = param.dest else {
                // Register destinations may overlap argument registers and need a
                // parallel move; that arrives with linear-scan integration.
                return Err(format!(
                    "parameter {i}: register destinations are not supported yet"
                ));
            };
            let source = match ARGUMENT_REGISTERS.get(i) {
                Some(arg) => Operand::Reg(*arg, width),
                None => {
                    let stack = Operand::Mem(
                        Self::at(
                            FIRST_STACK_ARGUMENT + SLOT * (i - ARGUMENT_REGISTERS.len()) as i32,
                        ),
                        width,
                    );
                    out.push(Inst::Mov(Operand::Reg(Reg::Rax, width), stack));
                    Operand::Reg(Reg::Rax, width)
                }
            };
            out.push(Inst::Mov(param.dest.clone(), source));
        }
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
                Operand::Mem(Self::at(-SLOT * (i as i32 + 1)), Width::W64),
            ));
        }
        out.push(Inst::Leave);
        out.push(Inst::Ret);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::print::inst_text;

    fn params(roots: u32, spills: usize, saved: &[PhysicalRegId], calls: bool) -> FrameParams {
        FrameParams {
            root_slots: roots,
            spill_slot_count: spills,
            callee_saved: saved.to_vec(),
            has_calls: calls,
            defer_registration: false,
        }
    }

    fn text(insts: &[Inst]) -> Vec<String> {
        insts.iter().map(inst_text).collect()
    }

    fn mem_disp(mem: Mem) -> i32 {
        match mem {
            Mem::Base { disp, .. } => disp,
            Mem::Rip(_) => unreachable!(),
        }
    }

    #[test]
    fn empty_leaf_has_no_stack_adjustment() {
        let frame = Frame::new(params(0, 0, &[], false)).unwrap();
        assert_eq!(frame.size(), 0);
        assert_eq!(
            text(&frame.prologue(&[]).unwrap()),
            ["push rbp", "mov rbp, rsp"]
        );
        assert_eq!(text(&frame.epilogue(true)), ["leave", "ret"]);
    }

    #[test]
    fn leaf_with_spills_rounds_up_to_sixteen() {
        let frame = Frame::new(params(0, 3, &[], false)).unwrap();
        assert_eq!(frame.size(), 32);
        assert_eq!(mem_disp(frame.spill(SpillSlotId(0))), -8);
        assert_eq!(mem_disp(frame.spill(SpillSlotId(2))), -24);
        assert!(text(&frame.prologue(&[]).unwrap()).contains(&"sub rsp, 32".to_string()));
        assert!(!frame.has_shadow_frame());
    }

    #[test]
    fn function_with_calls_registers_a_shadow_frame_even_with_no_roots() {
        let frame = Frame::new(params(0, 0, &[], true)).unwrap();
        // 16 shadow bytes + 8 ret_save = 24, rounded to 32.
        assert_eq!(frame.size(), 32);
        assert_eq!(
            text(&frame.prologue(&[]).unwrap()),
            [
                "push rbp",
                "mov rbp, rsp",
                "sub rsp, 32",
                "mov qword ptr [rbp - 16], 0",
                "mov qword ptr [rbp - 8], 0",
                "lea rdi, [rbp - 16]",
                "call lo_push_frame",
            ]
        );
    }

    #[test]
    fn full_frame_offsets() {
        use PhysicalRegId::Rbx;
        let frame = Frame::new(params(2, 1, &[Rbx], true)).unwrap();
        // saved 8 + shadow (16 + 2*8 = 32) + spill 8 + ret_save 8 = 56 -> 64.
        assert_eq!(frame.size(), 64);
        assert_eq!(mem_disp(frame.root(0)), -24); // base -40, roots at +16
        assert_eq!(mem_disp(frame.root(1)), -16);
        assert_eq!(mem_disp(frame.spill(SpillSlotId(0))), -48);
        let epilogue = text(&frame.epilogue(true));
        assert_eq!(
            epilogue,
            [
                "mov qword ptr [rbp - 56], rax",
                "call lo_pop_frame",
                "mov rax, qword ptr [rbp - 56]",
                "mov rbx, qword ptr [rbp - 8]",
                "leave",
                "ret",
            ]
        );
        let prologue = text(&frame.prologue(&[]).unwrap());
        assert!(prologue.contains(&"mov qword ptr [rbp - 8], rbx".to_string()));
        assert!(prologue.contains(&"mov qword ptr [rbp - 40], 0".to_string())); // parent
        assert!(prologue.contains(&"mov qword ptr [rbp - 32], 2".to_string())); // num_roots
    }

    #[test]
    fn void_epilogue_does_not_park_a_return_value() {
        let frame = Frame::new(params(0, 0, &[], true)).unwrap();
        assert_eq!(
            text(&frame.epilogue(false)),
            ["call lo_pop_frame", "leave", "ret"]
        );
    }

    #[test]
    fn parameters_come_from_registers_then_the_stack() {
        let frame = Frame::new(params(0, 8, &[], false)).unwrap();
        let params: Vec<Param> = (0..8)
            .map(|i| Param {
                ty: if i == 1 { IrType::Ref } else { IrType::Int32 },
                dest: Operand::Mem(
                    frame.spill(SpillSlotId(i)),
                    width_of(if i == 1 { IrType::Ref } else { IrType::Int32 }),
                ),
            })
            .collect();
        let lines = text(&frame.prologue(&params).unwrap());
        assert!(lines.contains(&"mov dword ptr [rbp - 8], edi".to_string()));
        assert!(lines.contains(&"mov qword ptr [rbp - 16], rsi".to_string()));
        assert!(lines.contains(&"mov dword ptr [rbp - 48], r9d".to_string()));
        // Seventh and eighth arguments are on the caller's stack.
        let seventh = lines
            .iter()
            .position(|l| l == "mov eax, dword ptr [rbp + 16]")
            .unwrap();
        assert_eq!(lines[seventh + 1], "mov dword ptr [rbp - 56], eax");
        let eighth = lines
            .iter()
            .position(|l| l == "mov eax, dword ptr [rbp + 24]")
            .unwrap();
        assert_eq!(lines[eighth + 1], "mov dword ptr [rbp - 64], eax");
    }

    #[test]
    fn startup_frame_defers_registration() {
        let mut p = params(1, 0, &[], true);
        p.defer_registration = true;
        let frame = Frame::new(p).unwrap();
        let prologue = text(&frame.prologue(&[]).unwrap());
        assert!(!prologue.iter().any(|l| l.contains("lo_push_frame")));
        // The frame is built and nulled, just not yet linked.
        assert!(prologue.iter().any(|l| l.ends_with(", 1"))); // num_roots
        assert_eq!(
            text(&frame.register_shadow()),
            ["lea rdi, [rbp - 24]", "call lo_push_frame"]
        );
    }

    #[test]
    fn register_destination_parameters_are_rejected_for_now() {
        let frame = Frame::new(params(0, 0, &[], false)).unwrap();
        let param = Param {
            ty: IrType::Int32,
            dest: Operand::r32(Reg::Rbx),
        };
        assert!(frame.prologue(&[param]).is_err());
    }

    #[test]
    fn leaf_functions_cannot_have_root_slots() {
        assert!(Frame::new(params(1, 0, &[], false)).is_err());
    }

    #[test]
    fn every_function_of_a_lowered_program_gets_a_valid_frame() {
        use crate::ir::lower::{lower_program, TargetLayout};
        use crate::ir::register_allocator::spill_all::spill_everything;
        let source = "class Node (int v; Node next;) [ Node(int x) { v = x; next = null; } ] \
            { int get() { return v; } int pick(int a, int b, int c, int d, int e, int f, int g) { return g; } } \
            class Main () { int main() { Node n; n = new Node(3); return (n.get() + n.pick(1,2,3,4,5,6,7)); } }";
        let tokens = crate::lexer::tokenize(source).unwrap();
        let ast = crate::parser::parse_program(&tokens).unwrap();
        let (typed, classes) = crate::type_checker::check_program(ast).unwrap();
        let ir = lower_program(&typed, &classes, TargetLayout::X86_64).unwrap();
        let program = crate::ir::gc_roots::insert_gc_roots(ir).verify().unwrap();
        let program = program.program();
        let mut saw_leaf = false;
        let mut saw_startup = false;
        for function in &program.functions {
            let allocation = spill_everything(function);
            let is_startup = program.startup == Some(function.symbol);
            let p = FrameParams::for_function(function, &allocation, is_startup);
            saw_leaf |= !p.has_calls;
            saw_startup |= p.defer_registration;
            let frame = Frame::new(p)
                .unwrap_or_else(|e| panic!("{}: {e}", program.symbols[function.symbol.0].name));
            assert_eq!(frame.size() % 16, 0);
            let dests: Vec<Param> = function
                .params
                .iter()
                .map(|r| {
                    let ty = function.register_types[r.0];
                    let loc = allocation.register_locations[r];
                    Param {
                        ty,
                        dest: frame.location(loc, width_of(ty)),
                    }
                })
                .collect();
            frame.prologue(&dests).unwrap();
            frame.epilogue(true);
        }
        assert!(saw_leaf, "expected a leaf function such as Node.get");
        assert!(saw_startup, "expected lo_entry to defer registration");
    }

    #[test]
    fn frames_are_aligned_and_regions_do_not_overlap() {
        use PhysicalRegId::{Rbx, R12, R13};
        let saved_sets: [&[PhysicalRegId]; 3] = [&[], &[Rbx], &[Rbx, R12, R13]];
        for calls in [false, true] {
            for saved in saved_sets {
                for spills in 0..=8 {
                    for roots in if calls { 0..=8 } else { 0..=0 } {
                        let frame = Frame::new(params(roots, spills, saved, calls)).unwrap();
                        assert_eq!(frame.size() % 16, 0, "size misaligned");
                        // (start, len) of every region, rbp-relative.
                        let mut regions: Vec<(i32, i32)> =
                            (0..saved.len()).map(|i| (-8 * (i as i32 + 1), 8)).collect();
                        if calls {
                            regions.push((mem_disp(frame.shadow_base()), 16 + 8 * roots as i32));
                            regions.push((frame.ret_save.unwrap(), 8));
                        }
                        for s in 0..spills {
                            regions.push((mem_disp(frame.spill(SpillSlotId(s))), 8));
                        }
                        if regions.is_empty() {
                            assert_eq!(frame.size(), 0);
                            continue;
                        }
                        regions.sort();
                        for pair in regions.windows(2) {
                            assert!(pair[0].0 + pair[0].1 <= pair[1].0, "overlap {pair:?}");
                        }
                        let (lowest, _) = regions[0];
                        assert!(lowest >= -frame.size(), "region below rsp");
                        let (last, len) = *regions.last().unwrap();
                        assert!(last + len <= 0, "region above rbp");
                    }
                }
            }
        }
    }
}
