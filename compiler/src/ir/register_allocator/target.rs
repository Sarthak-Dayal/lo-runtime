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
            // Caller-saved registers come first: a leaf function can use them
            // without saving anything. Functions that call use only callee-saved
            // registers, so those fall through to the second half.
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

#[cfg(test)]
mod tests {
    use super::PhysicalRegId::*;
    use super::*;

    #[test]
    fn system_v_register_roles() {
        let t = TargetConstraints::system_v();
        assert_eq!(t.argument_registers, [Rdi, Rsi, Rdx, Rcx, R8, R9]);
        assert_eq!(t.return_register, Rax);
        assert_eq!(t.stack_alignment_bytes, 16);
        assert_eq!(t.shadow_space_bytes, 0);
        for callee_saved in [Rbx, Rbp, R12, R13, R14, R15] {
            assert!(t.callee_saved_registers.contains(&callee_saved));
            assert!(!t.caller_saved_registers.contains(&callee_saved));
        }
        // Rsi and Rdi are caller-saved on System V (callee-saved on Microsoft x64).
        for caller_saved in [Rax, Rcx, Rdx, Rsi, Rdi, R8, R9, R10, R11] {
            assert!(t.caller_saved_registers.contains(&caller_saved));
        }
    }

    #[test]
    fn scratch_and_frame_registers_are_never_allocatable() {
        let t = TargetConstraints::system_v();
        // The code generator uses these as scratch or for the frame (codegen/select.rs).
        for scratch in [Rsp, Rbp, Rax, Rcx, Rdx, R10, R11] {
            assert!(t.reserved_registers.contains(&scratch));
            assert!(!t.allocatable_registers.contains(&scratch));
        }
        debug_check(&t);
    }

    #[test]
    fn calling_functions_keep_five_registers() {
        let t = TargetConstraints::system_v();
        let usable_across_calls: Vec<_> = t
            .allocatable_registers
            .iter()
            .filter(|r| t.callee_saved_registers.contains(r))
            .collect();
        assert_eq!(usable_across_calls, [&Rbx, &R12, &R13, &R14, &R15]);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "duplicate")]
    fn duplicate_pool_is_caught_in_debug_builds() {
        let mut t = TargetConstraints::system_v();
        t.allocatable_registers = vec![Rbx, Rbx];
        debug_check(&t);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "both allocatable and reserved")]
    fn reserved_register_in_pool_is_caught_in_debug_builds() {
        let mut t = TargetConstraints::system_v();
        t.allocatable_registers = vec![Rsp];
        debug_check(&t);
    }
}
