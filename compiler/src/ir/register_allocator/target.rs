//! The System V register file as the allocator sees it: which registers it may
//! hand out, and which of those survive a call.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PhysicalRegId {
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

/// The System V AMD64 register file as the allocator sees it (System V ABI,
/// AMD64 supplement, section 3.2). The argument registers live with the code
/// that uses them, in `codegen::frame`. Reserved registers, and why:
/// - rsp and rbp manage the stack frame;
/// - rax is the selector's accumulator and the return-value register;
/// - rdx is overwritten by `cdq` and `idiv`;
/// - r10 and r11 are the selector's scratch registers.
pub struct TargetConstraints {
    pub allocatable_registers: Vec<PhysicalRegId>,
    pub reserved_registers: Vec<PhysicalRegId>,
    pub callee_saved_registers: Vec<PhysicalRegId>,
}

impl TargetConstraints {
    pub fn system_v() -> Self {
        use PhysicalRegId::*;

        Self {
            // Caller-saved registers come first: a value that does not survive
            // a call can use them without any save or restore. Values that
            // survive a call may use only the callee-saved second half.
            allocatable_registers: vec![Rsi, Rdi, R8, R9, Rcx, Rbx, R12, R13, R14, R15],
            reserved_registers: vec![Rsp, Rbp, Rax, Rdx, R10, R11],
            // rbp and rsp are saved by the frame itself, not by the allocator.
            callee_saved_registers: vec![Rbx, R12, R13, R14, R15],
        }
    }
}

/// Debug-only guard that the hand-written pool has no duplicates and no reserved register.
pub(super) fn debug_check(target: &TargetConstraints) {
    debug_assert!(
        target
            .allocatable_registers
            .iter()
            .enumerate()
            .all(|(i, register)| !target.allocatable_registers[..i].contains(register)),
        "allocatable registers contain a duplicate: {:?}",
        target.allocatable_registers
    );
    debug_assert!(
        target
            .allocatable_registers
            .iter()
            .all(|register| !target.reserved_registers.contains(register)),
        "a register is both allocatable and reserved: {:?}",
        target.allocatable_registers
    );
}
