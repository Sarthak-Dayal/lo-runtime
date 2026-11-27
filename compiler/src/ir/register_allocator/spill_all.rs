//! Spill-everything "allocator": every virtual register lives in its own stack slot.
//!
//! It produces the same `ProgramAllocation` as linear scan, so code generation does
//! not know which producer ran. It is the reference for debugging: a native
//! divergence that disappears under spill-everything points at the allocator, not
//! at instruction selection.

use std::collections::HashMap;

use crate::ir::{CheckedIr, FunctionIr, VirtualRegId};

use super::{FunctionAllocation, PhysicalLocation, ProgramAllocation, SpillSlotId};

/// Virtual register `n` gets spill slot `n`. No registers are used, so no
/// callee-saved register needs preserving.
pub fn spill_everything(function: &FunctionIr) -> FunctionAllocation {
    let count = function.register_types.len();
    FunctionAllocation {
        register_locations: (0..count)
            .map(|n| (VirtualRegId(n), PhysicalLocation::Spill(SpillSlotId(n))))
            .collect(),
        spill_slot_count: count,
        used_callee_saved_registers: vec![],
    }
}

pub fn spill_everything_program(ir: &CheckedIr) -> ProgramAllocation {
    let function_allocations: HashMap<_, _> = ir
        .program()
        .functions
        .iter()
        .map(|function| (function.symbol, spill_everything(function)))
        .collect();
    ProgramAllocation {
        function_allocations,
    }
}
