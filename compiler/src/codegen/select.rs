//! Instruction selection: IR instructions to `x86::Inst`, one emitter per operator.
//!
//! The discipline: every arithmetic, compare and load result is computed in
//! `rax` (`eax` for Int32 and Bool; `idiv` leaves a remainder in `edx`) and then
//! moved to the destination's location. A second operand is used where it
//! lives, or is fetched into `r10` when x86 cannot take it there (an address
//! base, a divisor, a symbol address). `r11` carries memory-to-memory copies and
//! indirect call targets. Calls follow System V: stack arguments are pushed,
//! then register arguments are written as one parallel move.
//!
//! `src` gives an operand's location (register, memory or immediate); `fetch` is
//! `src` but loads memory into the scratch register given. The allocator never
//! hands out rax (accumulator and return value), rdx (`cdq`/`idiv` overwrite
//! it), or r10 and r11 (scratch). Widths come from IR types: Int32 and Bool are
//! 32-bit, everything else 64-bit.
//!
//! Under `-g`, `emit` also writes the line table (`.loc`): see `LineMarks`.

use std::collections::HashSet;

use crate::ir::register_allocator::{FunctionAllocation, PhysicalLocation};
use crate::ir::{
    BinaryOp, BlockId, CallTarget, DebugMode, FunctionIr, InstructionKind, IrType,
    Operand as IrOperand, ProgramIr, SymbolId, Terminator, UnaryOp, VirtualRegId, GENERATED_LINE,
};

use super::frame::{width_of, Frame, Param, ARGUMENT_REGISTERS};
use super::moves::{parallel_move, Move};
use super::x86::{
    check_instructions, Alu, Cond, Inst, Item, Mem, Operand, Reg, Reg8, Unary, Width,
};

const RAX: Reg = Reg::Rax;
const R10: Reg = Reg::R10;
const R11: Reg = Reg::R11;
/// Bytes one `push` moves `rsp` by.
const STACK_SLOT: i64 = 8;

/// Emits the assembly for one function: label, prologue, blocks.
pub fn select_function(
    program: &ProgramIr,
    function: &FunctionIr,
    allocation: &FunctionAllocation,
    debug: DebugMode,
) -> Result<Vec<Item>, String> {
    let name = program.symbols[function.symbol.0].name.clone();
    let frame = Frame::new(function, allocation)?;
    let mut selector = Selector {
        program,
        function,
        allocation,
        frame,
        debug,
        name: name.clone(),
        next_block: None,
        out: vec![],
        lines: LineMarks::default(),
    };
    selector
        .function_body()
        .map_err(|error| format!("{name}: {error}"))?;
    check_instructions(&name, &selector.out)?;
    Ok(selector.out)
}

struct Selector<'a> {
    program: &'a ProgramIr,
    function: &'a FunctionIr,
    allocation: &'a FunctionAllocation,
    frame: Frame,
    debug: DebugMode,
    name: String,
    /// The block emitted next, so jumps to it can be omitted.
    next_block: Option<BlockId>,
    out: Vec<Item>,
    lines: LineMarks,
}

/// The `-g` line table for one function, written as `.loc` marks. Each IR
/// instruction and block terminator sets `pending` to its source line; `emit`
/// writes the mark just before the first machine instruction that follows, so
/// an IR instruction that emits nothing leaves no mark behind.
///
/// - The first mark for a line in this function is where the debugger stops
///   for that line (`is_stmt 1`). Blocks are laid out in creation order, and a
///   statement's own block is created before any block its expression adds,
///   so the first mark is where the line's code begins. Later marks for the
///   same line (code laid out further down) get `is_stmt 0`: they map
///   addresses to the line but are never a stopping point.
/// - The first mark in the body also carries `prologue_end`, so a breakpoint
///   on the function stops after its setup code.
/// - Line 0 (setup code, compiler-written functions) gets no mark.
#[derive(Default)]
struct LineMarks {
    pending: Option<u32>,
    /// The line of the last mark written, reset at every block label.
    last: Option<u32>,
    seen: HashSet<u32>,
    prologue_end_written: bool,
}

impl LineMarks {
    /// The mark to write before an instruction on `line`, if one is needed.
    fn mark(&mut self, line: u32) -> Option<Item> {
        let continues_last_mark = self.last == Some(line) && self.prologue_end_written;
        if line == GENERATED_LINE || continues_last_mark {
            return None;
        }
        self.last = Some(line);
        let is_stmt = self.seen.insert(line);
        let prologue_end = !std::mem::replace(&mut self.prologue_end_written, true);
        Some(loc(line, is_stmt, prologue_end))
    }

    /// The mark at a function's first instruction: its declaration line.
    fn entry(&mut self, line: u32) -> Item {
        self.last = Some(line);
        self.seen.insert(line);
        loc(line, true, false)
    }
}

/// `.loc 1 LINE 0 is_stmt N [prologue_end]`: file 1, no column.
fn loc(line: u32, is_stmt: bool, prologue_end: bool) -> Item {
    let prologue_end = if prologue_end { " prologue_end" } else { "" };
    Item::Directive(format!(
        "    .loc 1 {line} 0 is_stmt {}{prologue_end}",
        u8::from(is_stmt)
    ))
}

impl Selector<'_> {
    fn emit(&mut self, inst: Inst) {
        if let Some(line) = self.lines.pending.take() {
            self.out.extend(self.lines.mark(line));
        }
        self.out.push(Item::Inst(inst));
    }

    /// Under `-g`, the line `emit` marks the next machine instruction with.
    fn set_line(&mut self, line: Option<u32>) {
        if self.debug == DebugMode::On {
            self.lines.pending = line;
        }
    }

    /// The first source line in `block`, for a mark that points into it.
    fn first_line(&self, block: BlockId) -> u32 {
        let block = &self.function.blocks[block.0];
        block
            .instructions
            .iter()
            .map(|instruction| instruction.line)
            .find(|&line| line != GENERATED_LINE)
            .or(block.terminator_line)
            .unwrap_or(self.function.decl_line)
    }

    fn label(&self, block: BlockId) -> String {
        format!(".L{}_{}", self.name, block.0)
    }

    fn symbol_name(&self, id: SymbolId) -> String {
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

    /// Where a virtual register lives (a register or a spill slot), as an operand
    /// of its type's width.
    fn location_operand(&self, v: VirtualRegId) -> Result<Operand, String> {
        Ok(self
            .frame
            .location(self.location(v)?, width_of(self.register_type(v))))
    }

    fn function_body(&mut self) -> Result<(), String> {
        self.out.push(Item::Blank);
        self.out
            .push(Item::Directive(format!(".globl {}", self.name)));
        self.out.push(Item::Label(self.name.clone()));
        if self.debug == DebugMode::On && self.function.decl_line != GENERATED_LINE {
            let entry = self.lines.entry(self.function.decl_line);
            self.out.push(entry);
        }

        let params = self
            .function
            .params
            .iter()
            .map(|&p| {
                let ty = self.register_type(p);
                Ok(Param {
                    ty,
                    dest: self.location_operand(p)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        self.out.extend(self.frame.prologue(&params, self.debug)?);
        // The prologue falls through into block 0, so jump if the entry block is elsewhere.
        if self.function.entry != BlockId(0) {
            self.emit(Inst::Jmp(self.label(self.function.entry)));
        }

        for (index, block) in self.function.blocks.iter().enumerate() {
            let id = BlockId(index);
            self.next_block = (index + 1 < self.function.blocks.len()).then(|| BlockId(index + 1));
            self.out.push(Item::Label(self.label(id)));
            if index > 0 {
                // Blocks are not laid out in source order: each states its line.
                self.lines.last = None;
            }
            for instruction in &block.instructions {
                self.set_line(Some(instruction.line));
                self.instruction(&instruction.kind)
                    .map_err(|e| format!("line {}: {e}", instruction.line))?;
            }
            self.set_line(block.terminator_line);
            self.terminator(&block.terminator)?;
        }
        Ok(())
    }

    // ---- operand helpers ----

    /// Where an operand is: register, memory or immediate. A symbol address is
    /// materialized into `scratch` (a 64-bit register operand).
    fn src(&mut self, operand: IrOperand, scratch: Reg) -> Result<Operand, String> {
        Ok(match operand {
            IrOperand::Value(v) => self.location_operand(v)?,
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
            InstructionKind::RootStore { slot, value } => self.root_store(*slot, *value),
            InstructionKind::RootLoad { dst, slot } => self.root_load(*dst, *slot),
            InstructionKind::Call { dst, target, args } => {
                self.call(*dst, target, args, /* returns_here */ true)
            }
        }
    }

    /// `dst = src` through r11 when memory-to-memory.
    fn copy(&mut self, dst: VirtualRegId, src: IrOperand) -> Result<(), String> {
        let value = self.fetch(src, R11)?;
        let dst = self.location_operand(dst)?;
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
        if matches!(op, BinaryOp::Eq | BinaryOp::Lt | BinaryOp::Gt) {
            return self.compare(dst, op, lhs, rhs);
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
            BinaryOp::Eq | BinaryOp::Lt | BinaryOp::Gt => unreachable!("compares return above"),
        }
        let result = if matches!(op, BinaryOp::Mod) {
            Operand::r32(Reg::Rdx)
        } else {
            acc
        };
        let dst = self.location_operand(dst)?;
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
            BinaryOp::Gt => Cond::G,
            _ => unreachable!("compare called with {op:?}"),
        };
        self.emit(Inst::SetCc(cond, Reg8::Al));
        self.emit(Inst::Movzx(RAX, Width::W32, Reg8::Al));
        let dst = self.location_operand(dst)?;
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
        let dst = self.location_operand(dst)?;
        self.emit(Inst::Mov(dst, acc));
        Ok(())
    }

    fn load(&mut self, dst: VirtualRegId, base: IrOperand, offset: i32) -> Result<(), String> {
        let width = width_of(self.register_type(dst));
        let base = self.address(base)?;
        let acc = Self::rax(width);
        self.emit(Inst::Mov(acc.clone(), Operand::mem(base, offset, width)));
        let dst = self.location_operand(dst)?;
        self.mov(dst, acc);
        Ok(())
    }

    fn root_store(&mut self, slot: u32, value: IrOperand) -> Result<(), String> {
        let value = self.fetch(value, RAX)?;
        let slot = Operand::Mem(self.frame.root(slot), Width::W64);
        self.emit(Inst::Mov(slot, value));
        Ok(())
    }

    fn root_load(&mut self, dst: VirtualRegId, slot: u32) -> Result<(), String> {
        // Through rax: the destination may itself be a spill slot, and x86 has
        // no memory-to-memory mov.
        let slot = Operand::Mem(self.frame.root(slot), Width::W64);
        self.mov(Self::rax(Width::W64), slot);
        let dst = self.location_operand(dst)?;
        self.mov(dst, Self::rax(Width::W64));
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
    /// left, `rsp` 16-aligned at the `call`. `returns_here` is false for `Abort`,
    /// whose callee never returns, so the stack is not cleaned up after it.
    fn call(
        &mut self,
        dst: Option<VirtualRegId>,
        target: &CallTarget,
        args: &[IrOperand],
        returns_here: bool,
    ) -> Result<(), String> {
        // The pointer may sit in an argument register, so take it out before
        // argument setup overwrites those.
        if let CallTarget::Indirect(pointer) = target {
            let value = self.fetch(*pointer, R11)?;
            self.mov(Operand::r64(R11), value);
        }
        let register_count = args.len().min(ARGUMENT_REGISTERS.len());
        let (register_args, stack_args) = args.split_at(register_count);

        // Between instructions rsp is 16-aligned (`Frame` rounds the frame size up
        // to 16). Each push moves it by 8, so an odd number of stack arguments
        // needs one 8-byte pad first. Pushes only read registers (through rax), so
        // they come before any argument register is written.
        let padding = if stack_args.len() % 2 == 1 {
            STACK_SLOT
        } else {
            0
        };
        if padding > 0 {
            self.emit(Inst::Alu(
                Alu::Sub,
                Operand::r64(Reg::Rsp),
                Operand::imm(padding),
            ));
        }
        for &arg in stack_args.iter().rev() {
            // push takes only 64-bit operands; the callee reads just the low 32
            // bits of an int or bool argument, so the upper half does not matter.
            let value = match self.fetch(arg, RAX)? {
                Operand::Reg(register, _) => Operand::r64(register),
                other => other,
            };
            self.emit(Inst::Push(value));
        }
        self.move_register_arguments(register_args)?;

        match target {
            CallTarget::Direct(id) => {
                let name = self.symbol_name(*id);
                self.emit(Inst::Call(name));
            }
            CallTarget::Indirect(_) => self.emit(Inst::CallIndirect(Operand::r64(R11))),
        }

        if returns_here {
            let cleanup = padding + STACK_SLOT * stack_args.len() as i64;
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
        Ok(())
    }

    /// Writes the register arguments. A value may already sit in another
    /// argument register (`f(b, a)` with `a` in rdi and `b` in rsi), so the
    /// register-to-register copies are one parallel move. Memory, immediate and
    /// symbol arguments are written after it, since their sources cannot be
    /// overwritten but their destinations may still be read by the parallel move.
    fn move_register_arguments(&mut self, args: &[IrOperand]) -> Result<(), String> {
        let mut register_moves = vec![];
        let mut other_args = vec![];
        for (&arg, &destination) in args.iter().zip(&ARGUMENT_REGISTERS) {
            match arg {
                IrOperand::Value(v) => match self.location_operand(v)? {
                    Operand::Reg(src, width) => register_moves.push(Move {
                        dst: destination,
                        src,
                        width,
                    }),
                    _ => other_args.push((arg, destination)),
                },
                _ => other_args.push((arg, destination)),
            }
        }
        for inst in parallel_move(&register_moves)? {
            self.emit(inst);
        }
        for (arg, destination) in other_args {
            if let IrOperand::Symbol(id) = arg {
                let name = self.symbol_name(id);
                self.emit(Inst::Lea(destination, Mem::Rip(name)));
            } else {
                let width = self.operand_width(arg)?;
                let value = self.src(arg, RAX)?;
                self.emit(Inst::Mov(Operand::Reg(destination, width), value));
            }
        }
        Ok(())
    }

    /// Copy a call's result out of rax. SysV leaves the upper bits of a `bool`
    /// return undefined, so Bool results are zero-extended from `al`.
    fn take_result(&mut self, dst: VirtualRegId) -> Result<(), String> {
        let ty = self.register_type(dst);
        let dst = self.location_operand(dst)?;
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
                let falls_through = self.next_block == Some(*target);
                let debug = self.debug == DebugMode::On;
                if debug
                    && !self.lines.prologue_end_written
                    && self.function.decl_line != GENERATED_LINE
                {
                    // The body's first code is the jump into a loop's test, so
                    // the test would carry `prologue_end` and a breakpoint on the
                    // function would stop on every pass. Mark this jump instead,
                    // which runs once; the test keeps its own stopping point.
                    let line = self.first_line(*target);
                    self.out.push(loc(line, false, true));
                    self.lines.prologue_end_written = true;
                    self.lines.pending = None;
                    self.emit(Inst::Jmp(self.label(*target)));
                } else if !falls_through || (debug && self.lines.pending.is_some()) {
                    // Under -g a `break` keeps its jump even when it falls
                    // through, so its line has code to stop on.
                    self.emit(Inst::Jmp(self.label(*target)));
                }
            }
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => {
                // cmp cannot take an immediate as its first operand, so a constant
                // condition goes through eax.
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
                self.call(
                    None,
                    &CallTarget::Direct(*target),
                    args,
                    /* returns_here */ false,
                )?;
            }
        }
        Ok(())
    }
}
