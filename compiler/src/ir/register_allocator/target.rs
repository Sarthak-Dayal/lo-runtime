use super::RegisterAllocationError;
use std::collections::HashSet;

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

// Integer/pointer register constraints for the Microsoft x64 (Windows) ABI
// Source: https://learn.microsoft.com/en-us/cpp/build/x64-calling-convention
// RSP/RBP are reserved for stack/frame management. RAX/RCX/RDX/R10/R11 are
// reserved by this backend for fixed-register instructions and call setup.
// Frame layout must provide 32 bytes of call shadow space and 16-byte alignment.
// Floating-point, vector, aggregate, and variadic ABI rules are not modeled.
pub struct TargetConstraints {
    pub allocatable_registers: Vec<PhysicalRegId>,
    pub reserved_registers: Vec<PhysicalRegId>,
    pub caller_saved_registers: Vec<PhysicalRegId>,
    pub callee_saved_registers: Vec<PhysicalRegId>,
    // Further arguments are passed on the stack
    pub argument_registers: [PhysicalRegId; 4],
    // Scalar integer/pointer return values use RAX
    pub return_register: PhysicalRegId,
    pub stack_alignment_bytes: usize,
    pub shadow_space_bytes: usize,
}

impl TargetConstraints {
    pub fn microsoft_x64() -> Self {
        use PhysicalRegId::*;

        Self {
            allocatable_registers: vec![Rbx, Rsi, Rdi, R8, R9, R12, R13, R14, R15],
            reserved_registers: vec![Rsp, Rbp, Rax, Rcx, Rdx, R10, R11],
            caller_saved_registers: vec![Rax, Rcx, Rdx, R8, R9, R10, R11],
            // RSP is preserved by restoring the stack pointer, not by allocating it
            // RBP remains callee-saved even though it is reserved from allocation
            callee_saved_registers: vec![Rbx, Rsp, Rbp, Rsi, Rdi, R12, R13, R14, R15],
            argument_registers: [Rcx, Rdx, R8, R9],
            return_register: Rax,
            stack_alignment_bytes: 16,
            shadow_space_bytes: 32,
        }
    }
}

pub(super) fn validate_target(
    target_constraints: &TargetConstraints,
) -> Result<(), RegisterAllocationError> {
    let microsoft_x64 = TargetConstraints::microsoft_x64();
    for (name, registers) in [
        ("allocatable", &target_constraints.allocatable_registers),
        ("reserved", &target_constraints.reserved_registers),
        ("caller-saved", &target_constraints.caller_saved_registers),
        ("callee-saved", &target_constraints.callee_saved_registers),
    ] {
        let mut seen = HashSet::new();
        for register in registers {
            if !seen.insert(register) {
                return Err(RegisterAllocationError::InvalidTarget(format!(
                    "duplicate {name} register: {register:?}"
                )));
            }
        }
    }

    // The ABI is fixed; callers may change the available pool and reserve extra
    // registers, but may not reclassify the hardware registers or call locations.
    let register_set =
        |registers: &[PhysicalRegId]| registers.iter().copied().collect::<HashSet<_>>();
    if register_set(&target_constraints.caller_saved_registers)
        != register_set(&microsoft_x64.caller_saved_registers)
        || register_set(&target_constraints.callee_saved_registers)
            != register_set(&microsoft_x64.callee_saved_registers)
        || target_constraints.argument_registers != microsoft_x64.argument_registers
        || target_constraints.return_register != microsoft_x64.return_register
        || target_constraints.stack_alignment_bytes != microsoft_x64.stack_alignment_bytes
        || target_constraints.shadow_space_bytes != microsoft_x64.shadow_space_bytes
    {
        return Err(RegisterAllocationError::InvalidTarget(
            "register roles and stack requirements must match the Microsoft x64 ABI".into(),
        ));
    }
    for register in &microsoft_x64.reserved_registers {
        if !target_constraints.reserved_registers.contains(register) {
            return Err(RegisterAllocationError::InvalidTarget(format!(
                "backend requires {register:?} to be reserved"
            )));
        }
    }
    for register in &target_constraints.allocatable_registers {
        if target_constraints.reserved_registers.contains(register) {
            return Err(RegisterAllocationError::InvalidTarget(format!(
                "register {register:?} is both allocatable and reserved"
            )));
        }
    }
    Ok(())
}
