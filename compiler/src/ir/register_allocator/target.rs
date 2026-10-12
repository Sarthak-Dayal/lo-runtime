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

// Integer/pointer register constraints for the System V AMD64 ABI (Linux).
// Source: System V Application Binary Interface, AMD64 Architecture Processor
// Supplement, section 3.2. RSP/RBP are reserved for stack/frame management.
// RAX/RCX/RDX/R10/R11 are reserved by this backend as scratch for fixed-register
// instructions (idiv, shifts), address computation and call setup.
// Floating-point, vector, aggregate and variadic ABI rules are not modeled.
// Only `allocatable_registers` and `callee_saved_registers` are read by the
// allocator; the rest documents the ABI the code generator relies on.
pub struct TargetConstraints {
    pub allocatable_registers: Vec<PhysicalRegId>,
    pub reserved_registers: Vec<PhysicalRegId>,
    pub caller_saved_registers: Vec<PhysicalRegId>,
    pub callee_saved_registers: Vec<PhysicalRegId>,
    // Further arguments are passed on the stack
    pub argument_registers: [PhysicalRegId; 6],
    // Scalar integer/pointer return values use RAX
    pub return_register: PhysicalRegId,
    pub stack_alignment_bytes: usize,
    // System V has no caller-allocated shadow space (that is Microsoft x64)
    pub shadow_space_bytes: usize,
}

impl TargetConstraints {
    pub fn system_v() -> Self {
        use PhysicalRegId::*;

        Self {
            // Caller-saved registers come first: a value that does not survive
            // a call can use them without any save or restore. Values that
            // survive a call may use only the callee-saved second half.
            allocatable_registers: vec![Rsi, Rdi, R8, R9, Rbx, R12, R13, R14, R15],
            reserved_registers: vec![Rsp, Rbp, Rax, Rcx, Rdx, R10, R11],
            caller_saved_registers: vec![Rax, Rcx, Rdx, Rsi, Rdi, R8, R9, R10, R11],
            // RSP is preserved by restoring the stack pointer, not by allocating it
            // RBP remains callee-saved even though it is reserved from allocation
            callee_saved_registers: vec![Rbx, Rsp, Rbp, R12, R13, R14, R15],
            argument_registers: [Rdi, Rsi, Rdx, Rcx, R8, R9],
            return_register: Rax,
            stack_alignment_bytes: 16,
            shadow_space_bytes: 0,
        }
    }
}

/// Cheap sanity checks for a hand-built register pool. Constraints come from
/// `system_v()` (optionally with a narrowed pool in tests), so this is a guard
/// against slips rather than input validation, and costs nothing in release builds.
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
