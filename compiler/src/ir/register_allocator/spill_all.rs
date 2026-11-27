//! Spill-everything "allocator": every virtual register lives in its own stack slot.
//!
//! It produces the same `ProgramAllocation` as linear scan, so code generation does
//! not know which producer ran. It is the reference for debugging: a native
//! divergence that disappears under spill-everything points at the allocator, not
//! at instruction selection.

use std::collections::HashMap;

use crate::ir::{CheckedIr, DebugMode, FunctionIr, VirtualRegId};

use super::{FunctionAllocation, PhysicalLocation, ProgramAllocation, SpillSlotId};

/// Virtual register `n` gets spill slot `n`. No registers are used, so no
/// callee-saved register needs preserving. Under `-g`, named `Ref`s live in
/// their root slots instead (`FunctionIr::named_refs_in_root_slots`), as in linear scan.
pub fn spill_everything(function: &FunctionIr, debug: DebugMode) -> FunctionAllocation {
    let count = function.register_types.len();
    let mut register_locations: HashMap<_, _> = (0..count)
        .map(|n| (VirtualRegId(n), PhysicalLocation::Spill(SpillSlotId(n))))
        .collect();
    if debug == DebugMode::On {
        for (slot, register) in function.named_refs_in_root_slots().into_iter().enumerate() {
            register_locations.insert(register, PhysicalLocation::Root(slot as u32));
        }
    }
    FunctionAllocation {
        register_locations,
        spill_slot_count: count,
        used_callee_saved_registers: vec![],
    }
}

pub fn spill_everything_program(ir: &CheckedIr, debug: DebugMode) -> ProgramAllocation {
    let function_allocations: HashMap<_, _> = ir
        .program()
        .functions
        .iter()
        .map(|function| (function.symbol, spill_everything(function, debug)))
        .collect();
    ProgramAllocation {
        function_allocations,
    }
}
