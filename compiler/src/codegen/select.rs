//! Instruction selection: IR instructions to `x86::Inst`, one emitter per operator.
//!
//! The discipline (planning/p2_docs/p2-codegen-instruction-selection.md):
//! fetch operands into scratch registers, operate, write the result back. Two
//! helpers carry every memory-operand rule. `src` gives an operand's location
//! (register, memory or immediate; a symbol address is materialized into the
//! scratch register given). `fetch` is `src`, but a memory operand is loaded into
//! the scratch register, so the result is always a register or an immediate.
//!
//! Scratch registers are rax, rcx, rdx, r10 and r11, which the allocator never
//! uses. Widths come from IR types: Int32 and Bool are 32-bit, everything else
//! 64-bit.

use crate::ir::register_allocator::{FunctionAllocation, PhysicalLocation};
use crate::ir::{
    BinaryOp, BlockId, CallTarget, FunctionIr, InstructionKind, IrType, Operand as IrOperand,
    ProgramIr, Terminator, UnaryOp, VirtualRegId,
};

use super::frame::{reg, width_of, Frame, FrameParams, Param, ARGUMENT_REGISTERS};
use super::x86::{Alu, Cond, Inst, Item, Mem, Operand, Reg, Reg8, Unary, Width};

const RAX: Reg = Reg::Rax;
const R10: Reg = Reg::R10;
const R11: Reg = Reg::R11;

/// Emits the assembly for one function: label, prologue, blocks.
pub fn select_function(
    program: &ProgramIr,
    function: &FunctionIr,
    allocation: &FunctionAllocation,
    is_startup: bool,
) -> Result<Vec<Item>, String> {
    let name = program.symbols[function.symbol.0].name.clone();
    let frame = Frame::new(FrameParams::for_function(function, allocation, is_startup))?;
    let mut selector = Selector {
        program,
        function,
        allocation,
        frame,
        name: name.clone(),
        is_startup,
        registered: false,
        next_block: None,
        out: vec![],
    };
    selector
        .function_body()
        .map_err(|error| format!("{name}: {error}"))?;
    for item in &selector.out {
        if let Item::Inst(inst) = item {
            inst.check()
                .map_err(|error| format!("{name}: {error}: {inst:?}"))?;
        }
    }
    Ok(selector.out)
}

struct Selector<'a> {
    program: &'a ProgramIr,
    function: &'a FunctionIr,
    allocation: &'a FunctionAllocation,
    frame: Frame,
    name: String,
    is_startup: bool,
    /// The startup frame is registered right after `lo_runtime_init`.
    registered: bool,
    /// The block emitted next, so jumps to it can be omitted.
    next_block: Option<BlockId>,
    out: Vec<Item>,
}

impl Selector<'_> {
    fn emit(&mut self, inst: Inst) {
        self.out.push(Item::Inst(inst));
    }

    fn label(&self, block: BlockId) -> String {
        format!(".L{}_{}", self.name, block.0)
    }

    fn symbol_name(&self, id: crate::ir::SymbolId) -> String {
        self.program.symbols[id.0].name.clone()
    }

    fn register_type(&self, v: VirtualRegId) -> IrType {
        self.function.register_types[v.0]
    }

    fn operand_width(&self, operand: IrOperand) -> Result<Width, String> {
        Ok(width_of(self.program.operand_type(self.function, operand)?))
    }

    fn location(&self, v: VirtualRegId) -> Result<PhysicalLocation, String> {
        self.allocation
            .register_locations
            .get(&v)
            .copied()
            .ok_or_else(|| format!("virtual register {} has no location", v.0))
    }

    /// The register a virtual register lives in, as an operand of its own width.
    fn reg_operand(&self, v: VirtualRegId) -> Result<Operand, String> {
        Ok(self
            .frame
            .location(self.location(v)?, width_of(self.register_type(v))))
    }

    fn function_body(&mut self) -> Result<(), String> {
        self.out.push(Item::Directive(String::new()));
        self.out
            .push(Item::Directive(format!(".globl {}", self.name)));
        self.out.push(Item::Label(self.name.clone()));

        let params = self
            .function
            .params
            .iter()
            .map(|&p| {
                let ty = self.register_type(p);
                Ok(Param {
                    ty,
                    dest: self.reg_operand(p)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        for inst in self.frame.prologue(&params)? {
            self.emit(inst);
        }
        if self.function.entry != BlockId(0) {
            self.emit(Inst::Jmp(self.label(self.function.entry)));
        }

        for (index, block) in self.function.blocks.iter().enumerate() {
            let id = BlockId(index);
            self.next_block = (index + 1 < self.function.blocks.len()).then(|| BlockId(index + 1));
            self.out.push(Item::Label(self.label(id)));
            for instruction in &block.instructions {
                self.instruction(&instruction.kind)
                    .map_err(|e| format!("line {}: {e}", instruction.line))?;
            }
            self.terminator(&block.terminator)?;
        }
        if self.is_startup && self.frame.has_shadow_frame() && !self.registered {
            return Err("startup function never calls lo_runtime_init".into());
        }
        Ok(())
    }

    // ---- operand helpers ----

    /// Where an operand is: register, memory or immediate. A symbol address is
    /// materialized into `scratch` (a 64-bit register operand).
    fn src(&mut self, operand: IrOperand, scratch: Reg) -> Result<Operand, String> {
        Ok(match operand {
            IrOperand::Value(v) => self.reg_operand(v)?,
            IrOperand::Int(n) => Operand::imm(i64::from(n)),
            IrOperand::Bool(b) => Operand::imm(i64::from(b)),
            IrOperand::Null => Operand::imm(0),
            IrOperand::Symbol(id) => {
                let name = self.symbol_name(id);
                self.emit(Inst::Lea(scratch, Mem::Rip(name)));
                Operand::r64(scratch)
            }
        })
    }

    /// Like `src`, but a memory operand is loaded into `scratch` first.
    fn fetch(&mut self, operand: IrOperand, scratch: Reg) -> Result<Operand, String> {
        match self.src(operand, scratch)? {
            Operand::Mem(mem, width) => {
                let loaded = Operand::Reg(scratch, width);
                self.emit(Inst::Mov(loaded.clone(), Operand::Mem(mem, width)));
                Ok(loaded)
            }
            other => Ok(other),
        }
    }

    fn mov(&mut self, dst: Operand, src: Operand) {
        if dst != src {
            self.emit(Inst::Mov(dst, src));
        }
    }

    /// A 64-bit register holding the address in `base`.
    fn address(&mut self, base: IrOperand) -> Result<Reg, String> {
        match self.fetch(base, R10)? {
            Operand::Reg(register, Width::W64) => Ok(register),
            other => Err(format!(
                "address operand must be a 64-bit register: {other:?}"
            )),
        }
    }

    fn rax(width: Width) -> Operand {
        Operand::Reg(RAX, width)
    }

    // ---- instructions ----

    fn instruction(&mut self, kind: &InstructionKind) -> Result<(), String> {
        match kind {
            InstructionKind::Copy { dst, src } => self.copy(*dst, *src),
            InstructionKind::Binary { dst, op, lhs, rhs } => self.binary(*dst, *op, *lhs, *rhs),
            InstructionKind::Unary { dst, op, src } => self.unary(*dst, *op, *src),
            InstructionKind::Load { dst, base, offset } => self.load(*dst, *base, *offset),
            InstructionKind::Store {
                base,
                offset,
                value,
                ty,
            } => self.store(*base, *offset, *value, *ty),
            InstructionKind::RootStore { slot, value } => {
                let value = self.fetch(*value, RAX)?;
                let slot = Operand::Mem(self.frame.root(*slot), Width::W64);
                self.emit(Inst::Mov(slot, value));
                Ok(())
            }
            InstructionKind::RootLoad { dst, slot } => {
                let slot = Operand::Mem(self.frame.root(*slot), Width::W64);
                self.mov(Self::rax(Width::W64), slot);
                let dst = self.reg_operand(*dst)?;
                self.mov(dst, Self::rax(Width::W64));
                Ok(())
            }
            InstructionKind::Call { dst, target, args } => self.call(*dst, target, args, true),
        }
    }

    /// `dst = src` through r11 when memory-to-memory.
    fn copy(&mut self, dst: VirtualRegId, src: IrOperand) -> Result<(), String> {
        let value = self.fetch(src, R11)?;
        let dst = self.reg_operand(dst)?;
        self.mov(dst, value);
        Ok(())
    }

    fn binary(
        &mut self,
        dst: VirtualRegId,
        op: BinaryOp,
        lhs: IrOperand,
        rhs: IrOperand,
    ) -> Result<(), String> {
        match op {
            BinaryOp::Eq | BinaryOp::Lt | BinaryOp::Gt => return self.compare(dst, op, lhs, rhs),
            _ => {}
        }
        let acc = Self::rax(Width::W32);
        let l = self.src(lhs, R10)?;
        self.mov(acc.clone(), l);
        match op {
            BinaryOp::Add | BinaryOp::Sub => {
                let r = self.src(rhs, R10)?;
                let alu = if matches!(op, BinaryOp::Add) {
                    Alu::Add
                } else {
                    Alu::Sub
                };
                self.emit(Inst::Alu(alu, acc.clone(), r));
            }
            BinaryOp::Mul => match self.src(rhs, R10)? {
                Operand::Imm(n) => self.emit(Inst::ImulImm(RAX, Width::W32, acc.clone(), n as i32)),
                r => self.emit(Inst::Imul(RAX, Width::W32, r)),
            },
            BinaryOp::Div | BinaryOp::Mod => {
                // Lowering has already routed divisors 0 and -1 around this (LO
                // division is total), so idiv cannot fault.
                let r = self.src(rhs, R10)?;
                let divisor = Operand::r32(R10);
                self.emit(Inst::Mov(divisor.clone(), r));
                self.emit(Inst::Cdq);
                self.emit(Inst::Idiv(divisor));
            }
            BinaryOp::Eq | BinaryOp::Lt | BinaryOp::Gt => unreachable!("handled above"),
        }
        let result = if matches!(op, BinaryOp::Mod) {
            Operand::r32(Reg::Rdx)
        } else {
            acc
        };
        let dst = self.reg_operand(dst)?;
        self.emit(Inst::Mov(dst, result));
        Ok(())
    }

    /// `cmp` at the operands' width, `setcc al`, zero-extend to a 32-bit Bool.
    fn compare(
        &mut self,
        dst: VirtualRegId,
        op: BinaryOp,
        lhs: IrOperand,
        rhs: IrOperand,
    ) -> Result<(), String> {
        let acc = Self::rax(self.operand_width(lhs)?);
        let l = self.src(lhs, R10)?;
        self.mov(acc.clone(), l);
        let r = self.src(rhs, R10)?;
        self.emit(Inst::Alu(Alu::Cmp, acc, r));
        let cond = match op {
            BinaryOp::Eq => Cond::E,
            BinaryOp::Lt => Cond::L,
            _ => Cond::G,
        };
        self.emit(Inst::SetCc(cond, Reg8::Al));
        self.emit(Inst::Movzx(RAX, Width::W32, Reg8::Al));
        let dst = self.reg_operand(dst)?;
        self.emit(Inst::Mov(dst, Self::rax(Width::W32)));
        Ok(())
    }

    fn unary(&mut self, dst: VirtualRegId, op: UnaryOp, src: IrOperand) -> Result<(), String> {
        let acc = Self::rax(Width::W32);
        let s = self.src(src, R10)?;
        self.mov(acc.clone(), s);
        match op {
            UnaryOp::Neg => self.emit(Inst::Unary(Unary::Neg, acc.clone())),
            // Bool is 0 or 1, so logical not is xor 1.
            UnaryOp::Not => self.emit(Inst::Alu(Alu::Xor, acc.clone(), Operand::imm(1))),
        }
        let dst = self.reg_operand(dst)?;
        self.emit(Inst::Mov(dst, acc));
        Ok(())
    }

    fn load(&mut self, dst: VirtualRegId, base: IrOperand, offset: i32) -> Result<(), String> {
        let width = width_of(self.register_type(dst));
        let base = self.address(base)?;
        let acc = Self::rax(width);
        self.emit(Inst::Mov(acc.clone(), Operand::mem(base, offset, width)));
        let dst = self.reg_operand(dst)?;
        self.mov(dst, acc);
        Ok(())
    }

    fn store(
        &mut self,
        base: IrOperand,
        offset: i32,
        value: IrOperand,
        ty: IrType,
    ) -> Result<(), String> {
        let base = self.address(base)?;
        let value = self.fetch(value, RAX)?;
        self.emit(Inst::Mov(Operand::mem(base, offset, width_of(ty)), value));
        Ok(())
    }

    // ---- calls ----

    /// System V call: first six arguments in registers, the rest pushed right to
    /// left, `rsp` 16-aligned at the `call`. `returns` is false for `Abort`.
    fn call(
        &mut self,
        dst: Option<VirtualRegId>,
        target: &CallTarget,
        args: &[IrOperand],
        returns: bool,
    ) -> Result<(), String> {
        self.reject_argument_register_sources(target, args)?;
        let register_args = args.len().min(ARGUMENT_REGISTERS.len());
        let stack_args = &args[register_args..];

        // An odd number of pushes would leave rsp misaligned, so pad first.
        let padding = if stack_args.len() % 2 == 1 { 8 } else { 0 };
        if padding > 0 {
            self.emit(Inst::Alu(Alu::Sub, Operand::r64(Reg::Rsp), Operand::imm(8)));
        }
        for &arg in stack_args.iter().rev() {
            let value = match self.fetch(arg, RAX)? {
                Operand::Reg(register, _) => Operand::r64(register),
                other => other,
            };
            self.emit(Inst::Push(value));
        }
        for (index, &arg) in args[..register_args].iter().enumerate() {
            let destination = ARGUMENT_REGISTERS[index];
            if let IrOperand::Symbol(id) = arg {
                let name = self.symbol_name(id);
                self.emit(Inst::Lea(destination, Mem::Rip(name)));
            } else {
                let width = self.operand_width(arg)?;
                let value = self.src(arg, RAX)?;
                self.emit(Inst::Mov(Operand::Reg(destination, width), value));
            }
        }

        let callee_name = match target {
            CallTarget::Direct(id) => {
                let name = self.symbol_name(*id);
                self.emit(Inst::Call(name.clone()));
                Some(name)
            }
            CallTarget::Indirect(pointer) => {
                let value = self.fetch(*pointer, R11)?;
                self.mov(Operand::r64(R11), value);
                self.emit(Inst::CallIndirect(Operand::r64(R11)));
                None
            }
        };

        if returns {
            let cleanup = padding + 8 * stack_args.len() as i64;
            if cleanup > 0 {
                self.emit(Inst::Alu(
                    Alu::Add,
                    Operand::r64(Reg::Rsp),
                    Operand::imm(cleanup),
                ));
            }
        }
        if let Some(dst) = dst {
            self.take_result(dst)?;
        }
        if self.is_startup && !self.registered && callee_name.as_deref() == Some("lo_runtime_init")
        {
            // The runtime resets its frame chain in init, so the startup frame
            // may only be linked after it.
            self.registered = true;
            for inst in self.frame.register_shadow() {
                self.emit(inst);
            }
        }
        Ok(())
    }

    /// Sequential argument moves are only correct when no argument lives in an
    /// argument register. Spill-everything guarantees that; a register allocator
    /// must use a parallel move instead.
    fn reject_argument_register_sources(
        &self,
        target: &CallTarget,
        args: &[IrOperand],
    ) -> Result<(), String> {
        let callee = match target {
            CallTarget::Indirect(pointer) => Some(*pointer),
            CallTarget::Direct(_) => None,
        };
        for operand in args.iter().copied().chain(callee) {
            if let IrOperand::Value(v) = operand {
                if let PhysicalLocation::Register(id) = self.location(v)? {
                    if ARGUMENT_REGISTERS.contains(&reg(id)) {
                        return Err(format!(
                            "call operand v{} is in argument register {:?}; a parallel move is required",
                            v.0,
                            reg(id)
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Copy a call's result out of rax. SysV leaves the upper bits of a `bool`
    /// return undefined, so Bool results are zero-extended from `al`.
    fn take_result(&mut self, dst: VirtualRegId) -> Result<(), String> {
        let ty = self.register_type(dst);
        let dst = self.reg_operand(dst)?;
        match ty {
            IrType::Bool => {
                self.emit(Inst::Movzx(RAX, Width::W32, Reg8::Al));
                self.mov(dst, Self::rax(Width::W32));
            }
            _ => self.mov(dst, Self::rax(width_of(ty))),
        }
        Ok(())
    }

    // ---- terminators ----

    fn terminator(&mut self, terminator: &Terminator) -> Result<(), String> {
        match terminator {
            Terminator::Jump(target) => {
                if self.next_block != Some(*target) {
                    self.emit(Inst::Jmp(self.label(*target)));
                }
            }
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => {
                let value = match self.src(*condition, RAX)? {
                    Operand::Imm(n) => {
                        let acc = Self::rax(Width::W32);
                        self.emit(Inst::Mov(acc.clone(), Operand::imm(n)));
                        acc
                    }
                    other => other,
                };
                self.emit(Inst::Alu(Alu::Cmp, value, Operand::imm(0)));
                let (then_label, else_label) = (self.label(*then_block), self.label(*else_block));
                if self.next_block == Some(*then_block) {
                    self.emit(Inst::Jcc(Cond::E, else_label));
                } else {
                    self.emit(Inst::Jcc(Cond::Ne, then_label));
                    if self.next_block != Some(*else_block) {
                        self.emit(Inst::Jmp(else_label));
                    }
                }
            }
            Terminator::Return(value) => {
                if let Some(value) = value {
                    let acc = Self::rax(self.operand_width(*value)?);
                    let source = self.src(*value, RAX)?;
                    self.mov(acc, source);
                }
                for inst in self.frame.epilogue(value.is_some()) {
                    self.emit(inst);
                }
            }
            Terminator::Abort { target, args } => {
                // Never returns: no stack cleanup and no code follows.
                self.call(None, &CallTarget::Direct(*target), args, false)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::print::inst_text;
    use crate::ir::register_allocator::spill_all::spill_everything;
    use crate::ir::{BasicBlock, Instruction, SymbolId};
    use IrType::{Bool, Int32, Ptr, Ref};

    /// A one-block function `f(params...)` over registers of the given types.
    struct Case {
        program: ProgramIr,
        function: FunctionIr,
    }

    fn case(
        types: &[IrType],
        params: &[usize],
        setup: impl FnOnce(&mut ProgramIr) -> (Vec<InstructionKind>, Terminator),
    ) -> Case {
        let mut program = ProgramIr {
            symbols: vec![],
            signatures: vec![],
            functions: vec![],
            data: vec![],
            startup: None,
        };
        let symbol = program
            .declare_callable("f", params.iter().map(|&p| types[p]).collect(), None)
            .unwrap();
        let (instructions, terminator) = setup(&mut program);
        let mut function = FunctionIr {
            symbol,
            params: params.iter().map(|&p| VirtualRegId(p)).collect(),
            register_types: vec![],
            register_names: vec![],
            root_slots: 0,
            blocks: vec![],
            entry: BlockId(0),
        };
        for ty in types {
            function.new_register(*ty, None);
        }
        function.blocks.push(BasicBlock {
            instructions: instructions
                .into_iter()
                .map(|kind| Instruction { kind, line: 1 })
                .collect(),
            terminator,
        });
        Case { program, function }
    }

    fn lines(case: &Case) -> Vec<String> {
        let allocation = spill_everything(&case.function);
        select_function(&case.program, &case.function, &allocation, false)
            .unwrap()
            .iter()
            .filter_map(|item| match item {
                Item::Inst(inst) => Some(inst_text(inst)),
                _ => None,
            })
            .collect()
    }

    /// Instructions after the first block label, with the epilogue removed.
    fn body(case: &Case) -> Vec<String> {
        let allocation = spill_everything(&case.function);
        let items = select_function(&case.program, &case.function, &allocation, false).unwrap();
        let start = items
            .iter()
            .position(|item| matches!(item, Item::Label(l) if l.starts_with(".Lf_")))
            .unwrap();
        let mut out: Vec<String> = items[start..]
            .iter()
            .filter_map(|item| match item {
                Item::Inst(inst) => Some(inst_text(inst)),
                _ => None,
            })
            .collect();
        if out.last().map(String::as_str) == Some("ret") {
            out.pop();
            out.pop(); // leave
            if out.last().map(String::as_str) == Some("call lo_pop_frame") {
                out.pop();
            }
        }
        out
    }

    /// `body`, starting at the first line equal to `from`.
    fn body_from(case: &Case, from: &str) -> Vec<String> {
        let b = body(case);
        let at = b
            .iter()
            .position(|l| l == from)
            .unwrap_or_else(|| panic!("{from} not in {b:#?}"));
        b[at..].to_vec()
    }

    fn v(n: usize) -> IrOperand {
        IrOperand::Value(VirtualRegId(n))
    }

    fn binary(op: BinaryOp, dst: usize, lhs: IrOperand, rhs: IrOperand) -> InstructionKind {
        InstructionKind::Binary {
            dst: VirtualRegId(dst),
            op,
            lhs,
            rhs,
        }
    }

    // Spill-everything leaf frame: register n lives at [rbp - 8(n+1)].

    #[test]
    fn add_function_golden() {
        let c = case(&[Int32, Int32, Int32], &[0, 1], |_| {
            (
                vec![binary(BinaryOp::Add, 2, v(0), v(1))],
                Terminator::Return(Some(v(2))),
            )
        });
        assert_eq!(
            lines(&c),
            [
                "push rbp",
                "mov rbp, rsp",
                "sub rsp, 32",
                "mov dword ptr [rbp - 8], edi",
                "mov dword ptr [rbp - 16], esi",
                "mov eax, dword ptr [rbp - 8]",
                "add eax, dword ptr [rbp - 16]",
                "mov dword ptr [rbp - 24], eax",
                "mov eax, dword ptr [rbp - 24]",
                "leave",
                "ret",
            ]
        );
    }

    #[test]
    fn arithmetic_rows() {
        let sub = case(&[Int32, Int32], &[], |_| {
            (
                vec![binary(BinaryOp::Sub, 1, v(0), IrOperand::Int(5))],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&sub),
            [
                "mov eax, dword ptr [rbp - 8]",
                "sub eax, 5",
                "mov dword ptr [rbp - 16], eax"
            ]
        );
        let mul_reg = case(&[Int32, Int32, Int32], &[], |_| {
            (
                vec![binary(BinaryOp::Mul, 2, v(0), v(1))],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&mul_reg),
            [
                "mov eax, dword ptr [rbp - 8]",
                "imul eax, dword ptr [rbp - 16]",
                "mov dword ptr [rbp - 24], eax"
            ]
        );
        let mul_imm = case(&[Int32, Int32], &[], |_| {
            (
                vec![binary(BinaryOp::Mul, 1, v(0), IrOperand::Int(12))],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&mul_imm),
            [
                "mov eax, dword ptr [rbp - 8]",
                "imul eax, eax, 12",
                "mov dword ptr [rbp - 16], eax"
            ]
        );
    }

    #[test]
    fn division_and_modulo_use_idiv_through_r10() {
        let div = case(&[Int32, Int32, Int32], &[], |_| {
            (
                vec![binary(BinaryOp::Div, 2, v(0), v(1))],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&div),
            [
                "mov eax, dword ptr [rbp - 8]",
                "mov r10d, dword ptr [rbp - 16]",
                "cdq",
                "idiv r10d",
                "mov dword ptr [rbp - 24], eax"
            ]
        );
        let rem = case(&[Int32, Int32], &[], |_| {
            (
                vec![binary(BinaryOp::Mod, 1, v(0), IrOperand::Int(7))],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&rem),
            [
                "mov eax, dword ptr [rbp - 8]",
                "mov r10d, 7",
                "cdq",
                "idiv r10d",
                "mov dword ptr [rbp - 16], edx"
            ]
        );
    }

    #[test]
    fn comparisons_use_the_operand_width() {
        let lt = case(&[Int32, Bool], &[], |_| {
            (
                vec![binary(BinaryOp::Lt, 1, v(0), IrOperand::Int(3))],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&lt),
            [
                "mov eax, dword ptr [rbp - 8]",
                "cmp eax, 3",
                "setl al",
                "movzx eax, al",
                "mov dword ptr [rbp - 16], eax"
            ]
        );
        let ref_eq_null = case(&[Ref, Bool], &[], |_| {
            (
                vec![binary(BinaryOp::Eq, 1, v(0), IrOperand::Null)],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&ref_eq_null),
            [
                "mov rax, qword ptr [rbp - 8]",
                "cmp rax, 0",
                "sete al",
                "movzx eax, al",
                "mov dword ptr [rbp - 16], eax"
            ]
        );
        let gt = case(&[Int32, Int32, Bool], &[], |_| {
            (
                vec![binary(BinaryOp::Gt, 2, v(0), v(1))],
                Terminator::Return(None),
            )
        });
        assert!(body(&gt).contains(&"setg al".to_string()));
    }

    #[test]
    fn unary_rows() {
        let neg = case(&[Int32, Int32], &[], |_| {
            (
                vec![InstructionKind::Unary {
                    dst: VirtualRegId(1),
                    op: UnaryOp::Neg,
                    src: v(0),
                }],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&neg),
            [
                "mov eax, dword ptr [rbp - 8]",
                "neg eax",
                "mov dword ptr [rbp - 16], eax"
            ]
        );
        let not = case(&[Bool, Bool], &[], |_| {
            (
                vec![InstructionKind::Unary {
                    dst: VirtualRegId(1),
                    op: UnaryOp::Not,
                    src: v(0),
                }],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&not),
            [
                "mov eax, dword ptr [rbp - 8]",
                "xor eax, 1",
                "mov dword ptr [rbp - 16], eax"
            ]
        );
    }

    #[test]
    fn copy_goes_through_r11_and_materializes_symbols() {
        let copy = case(&[Int32, Int32], &[], |_| {
            (
                vec![InstructionKind::Copy {
                    dst: VirtualRegId(1),
                    src: v(0),
                }],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&copy),
            [
                "mov r11d, dword ptr [rbp - 8]",
                "mov dword ptr [rbp - 16], r11d"
            ]
        );
        let imm = case(&[Int32], &[], |_| {
            (
                vec![InstructionKind::Copy {
                    dst: VirtualRegId(0),
                    src: IrOperand::Int(-1),
                }],
                Terminator::Return(None),
            )
        });
        assert_eq!(body(&imm), ["mov dword ptr [rbp - 8], -1"]);
        let symbol = case(&[Ref], &[], |p| {
            let s = p
                .declare_symbol("LO_EMPTY_STRING", crate::ir::SymbolKind::StaticRef)
                .unwrap();
            (
                vec![InstructionKind::Copy {
                    dst: VirtualRegId(0),
                    src: IrOperand::Symbol(s),
                }],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&symbol),
            [
                "lea r11, [rip + LO_EMPTY_STRING]",
                "mov qword ptr [rbp - 8], r11"
            ]
        );
    }

    #[test]
    fn load_and_store_use_the_type_width() {
        let load = case(&[Ref, Int32, Ref], &[], |_| {
            (
                vec![
                    InstructionKind::Load {
                        dst: VirtualRegId(1),
                        base: v(0),
                        offset: 16,
                    },
                    InstructionKind::Load {
                        dst: VirtualRegId(2),
                        base: v(0),
                        offset: 24,
                    },
                ],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&load),
            [
                "mov r10, qword ptr [rbp - 8]",
                "mov eax, dword ptr [r10 + 16]",
                "mov dword ptr [rbp - 16], eax",
                "mov r10, qword ptr [rbp - 8]",
                "mov rax, qword ptr [r10 + 24]",
                "mov qword ptr [rbp - 24], rax",
            ]
        );
        let store = case(&[Ref, Int32], &[], |_| {
            (
                vec![
                    InstructionKind::Store {
                        base: v(0),
                        offset: 16,
                        value: v(1),
                        ty: Int32,
                    },
                    InstructionKind::Store {
                        base: v(0),
                        offset: 24,
                        value: IrOperand::Null,
                        ty: Ref,
                    },
                ],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&store),
            [
                "mov r10, qword ptr [rbp - 8]",
                "mov eax, dword ptr [rbp - 16]",
                "mov dword ptr [r10 + 16], eax",
                "mov r10, qword ptr [rbp - 8]",
                "mov qword ptr [r10 + 24], 0",
            ]
        );
    }

    #[test]
    fn store_to_a_static_symbol_base() {
        // lo_bindings-style: a Ref stored into static data addressed by symbol.
        let c = case(&[Ref], &[], |p| {
            let s = p
                .declare_symbol("lo_bindings", crate::ir::SymbolKind::Data)
                .unwrap();
            (
                vec![InstructionKind::Store {
                    base: IrOperand::Symbol(s),
                    offset: 24,
                    value: v(0),
                    ty: Ref,
                }],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body(&c),
            [
                "lea r10, [rip + lo_bindings]",
                "mov rax, qword ptr [rbp - 8]",
                "mov qword ptr [r10 + 24], rax",
            ]
        );
    }

    #[test]
    fn branch_layouts() {
        // blocks: 0 -> branch(1, 2); 1; 2. then is next: je else only.
        let build = |then_block: usize, else_block: usize| {
            let mut c = case(&[Bool], &[], |_| {
                (
                    vec![],
                    Terminator::Branch {
                        condition: v(0),
                        then_block: BlockId(then_block),
                        else_block: BlockId(else_block),
                    },
                )
            });
            for _ in 0..2 {
                c.function.blocks.push(BasicBlock {
                    instructions: vec![],
                    terminator: Terminator::Return(None),
                });
            }
            lines(&c)
        };
        let then_next = build(1, 2);
        assert!(then_next.contains(&"cmp dword ptr [rbp - 8], 0".to_string()));
        assert!(then_next.contains(&"je .Lf_2".to_string()));
        assert!(!then_next.iter().any(|l| l.starts_with("jmp")));
        let else_next = build(2, 1);
        assert!(else_next.contains(&"jne .Lf_2".to_string()));
        assert!(!else_next.iter().any(|l| l.starts_with("jmp")));
        let neither = build(2, 2);
        assert!(neither.contains(&"jne .Lf_2".to_string()));
        assert!(neither.contains(&"jmp .Lf_2".to_string()));
    }

    #[test]
    fn constant_branch_condition_is_materialized() {
        let c = case(&[], &[], |_| {
            (
                vec![],
                Terminator::Branch {
                    condition: IrOperand::Bool(true),
                    then_block: BlockId(0),
                    else_block: BlockId(0),
                },
            )
        });
        let l = lines(&c);
        assert!(l.contains(&"mov eax, 1".to_string()));
        assert!(l.contains(&"cmp eax, 0".to_string()));
    }

    #[test]
    fn jump_to_the_next_block_is_omitted() {
        let mut c = case(&[], &[], |_| (vec![], Terminator::Jump(BlockId(1))));
        c.function.blocks.push(BasicBlock {
            instructions: vec![],
            terminator: Terminator::Return(None),
        });
        assert!(!lines(&c).iter().any(|l| l.starts_with("jmp")));
        let mut back = case(&[], &[], |_| (vec![], Terminator::Return(None)));
        back.function.blocks[0].terminator = Terminator::Jump(BlockId(0));
        assert!(lines(&back).contains(&"jmp .Lf_0".to_string()));
    }

    fn declare_ext(
        p: &mut ProgramIr,
        name: &str,
        params: Vec<IrType>,
        result: Option<IrType>,
    ) -> SymbolId {
        p.declare_callable(name, params, result).unwrap()
    }

    #[test]
    fn direct_call_with_register_arguments() {
        let c = case(&[Int32, Ref, Int32], &[], |p| {
            let target = declare_ext(p, "g", vec![Int32, Ref], Some(Int32));
            (
                vec![InstructionKind::Call {
                    dst: Some(VirtualRegId(2)),
                    target: CallTarget::Direct(target),
                    args: vec![v(0), v(1)],
                }],
                Terminator::Return(None),
            )
        });
        // A function with a call has a 16-byte shadow frame below rbp; slots follow.
        assert_eq!(
            body_from(&c, "mov edi, dword ptr [rbp - 24]"),
            [
                "mov edi, dword ptr [rbp - 24]",
                "mov rsi, qword ptr [rbp - 32]",
                "call g",
                "mov dword ptr [rbp - 40], eax",
            ]
        );
    }

    fn many_args(n: usize) -> Case {
        case(&vec![Int32; n], &[], move |p| {
            let target = declare_ext(p, "g", vec![Int32; n], None);
            (
                vec![InstructionKind::Call {
                    dst: None,
                    target: CallTarget::Direct(target),
                    args: (0..n).map(v).collect(),
                }],
                Terminator::Return(None),
            )
        })
    }

    #[test]
    fn stack_arguments_are_pushed_right_to_left_and_cleaned_up() {
        // Shadow frame (16) first, so register n is at [rbp - (24 + 8n)].
        let register_moves = [
            "mov edi, dword ptr [rbp - 24]",
            "mov esi, dword ptr [rbp - 32]",
            "mov edx, dword ptr [rbp - 40]",
            "mov ecx, dword ptr [rbp - 48]",
            "mov r8d, dword ptr [rbp - 56]",
            "mov r9d, dword ptr [rbp - 64]",
        ];
        // Eight arguments: two pushes, eighth first, no padding.
        let mut expected = vec![
            "mov eax, dword ptr [rbp - 80]",
            "push rax",
            "mov eax, dword ptr [rbp - 72]",
            "push rax",
        ];
        expected.extend(register_moves);
        expected.extend(["call g", "add rsp, 16"]);
        assert_eq!(
            body_from(&many_args(8), "mov eax, dword ptr [rbp - 80]"),
            expected
        );

        // Seven arguments: an odd push count needs 8 bytes of padding first.
        let mut expected = vec!["sub rsp, 8", "mov eax, dword ptr [rbp - 72]", "push rax"];
        expected.extend(register_moves);
        expected.extend(["call g", "add rsp, 16"]);
        assert_eq!(body_from(&many_args(7), "sub rsp, 8"), expected);
    }

    #[test]
    fn bool_results_are_zero_extended() {
        let c = case(&[Ref, Bool], &[], |p| {
            let target = declare_ext(p, "lo_instanceof", vec![Ref, Ptr], Some(Bool));
            let descriptor = p
                .declare_symbol("lo_class_4_Main", crate::ir::SymbolKind::Data)
                .unwrap();
            (
                vec![InstructionKind::Call {
                    dst: Some(VirtualRegId(1)),
                    target: CallTarget::Direct(target),
                    args: vec![v(0), IrOperand::Symbol(descriptor)],
                }],
                Terminator::Return(None),
            )
        });
        let b = body(&c);
        assert!(b.contains(&"lea rsi, [rip + lo_class_4_Main]".to_string()));
        let call = b.iter().position(|l| l == "call lo_instanceof").unwrap();
        assert_eq!(b[call + 1], "movzx eax, al");
        assert_eq!(b[call + 2], "mov dword ptr [rbp - 32], eax");
    }

    #[test]
    fn indirect_call_goes_through_r11() {
        use crate::ir::{Signature, SignatureId};
        // "f" interned signature 0, so the next distinct signature is 1.
        let code = IrType::CodePtr(SignatureId(1));
        let c = case(&[Ref, Int32, code], &[], |p| {
            let sig = p.intern_signature(Signature {
                params: vec![Ref],
                result: Some(Int32),
            });
            assert_eq!(sig, SignatureId(1));
            (
                vec![
                    InstructionKind::Load {
                        dst: VirtualRegId(2),
                        base: v(0),
                        offset: 0,
                    },
                    InstructionKind::Call {
                        dst: Some(VirtualRegId(1)),
                        target: CallTarget::Indirect(v(2)),
                        args: vec![v(0)],
                    },
                ],
                Terminator::Return(None),
            )
        });
        assert_eq!(
            body_from(&c, "mov rdi, qword ptr [rbp - 24]"),
            [
                "mov rdi, qword ptr [rbp - 24]",
                "mov r11, qword ptr [rbp - 40]",
                "call r11",
                "mov dword ptr [rbp - 32], eax",
            ]
        );
    }

    #[test]
    fn root_slots_are_addressed_through_the_frame() {
        // Two root slots, so the shadow frame is 32 bytes: slot 0 at [rbp-16],
        // slot 1 at [rbp-8]; spill slots follow at [rbp-40] and below.
        let mut c = case(&[Ref, Ref], &[], |p| {
            let target = declare_ext(p, "g", vec![], None);
            (
                vec![
                    InstructionKind::RootStore {
                        slot: 1,
                        value: v(0),
                    },
                    InstructionKind::Call {
                        dst: None,
                        target: CallTarget::Direct(target),
                        args: vec![],
                    },
                    InstructionKind::RootLoad {
                        dst: VirtualRegId(1),
                        slot: 0,
                    },
                ],
                Terminator::Return(None),
            )
        });
        c.function.root_slots = 2;
        assert_eq!(
            body_from(&c, "mov rax, qword ptr [rbp - 40]"),
            [
                "mov rax, qword ptr [rbp - 40]",
                "mov qword ptr [rbp - 8], rax",
                "call g",
                "mov rax, qword ptr [rbp - 16]",
                "mov qword ptr [rbp - 48], rax",
            ]
        );
    }

    #[test]
    fn call_operands_in_argument_registers_need_a_parallel_move() {
        use crate::ir::register_allocator::{PhysicalLocation, PhysicalRegId};
        let c = case(&[Int32], &[], |p| {
            let target = declare_ext(p, "g", vec![Int32], None);
            (
                vec![InstructionKind::Call {
                    dst: None,
                    target: CallTarget::Direct(target),
                    args: vec![v(0)],
                }],
                Terminator::Return(None),
            )
        });
        let mut allocation = spill_everything(&c.function);
        allocation.register_locations.insert(
            VirtualRegId(0),
            PhysicalLocation::Register(PhysicalRegId::Rdi),
        );
        let err = select_function(&c.program, &c.function, &allocation, false).unwrap_err();
        assert!(err.contains("parallel move"), "{err}");
    }

    #[test]
    fn abort_is_a_call_with_no_cleanup() {
        let c = case(&[], &[], |p| {
            let target = declare_ext(p, "lo_abort_null_receiver", vec![Ptr, Int32], None);
            let bytes = p
                .declare_symbol("lo_bytes_0", crate::ir::SymbolKind::Data)
                .unwrap();
            (
                vec![],
                Terminator::Abort {
                    target,
                    args: vec![IrOperand::Symbol(bytes), IrOperand::Int(4)],
                },
            )
        });
        assert_eq!(
            body(&c).iter().map(String::as_str).collect::<Vec<_>>(),
            [
                "lea rdi, [rip + lo_bytes_0]",
                "mov esi, 4",
                "call lo_abort_null_receiver"
            ]
        );
    }
}
