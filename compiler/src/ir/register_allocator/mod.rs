/*
Poletto-Sarkar Linear Scan Register Allocation
Target: Microsoft x64 (Windows) ABI; general-purpose integer/pointer registers.
Construct constraints with TargetConstraints::microsoft_x64().
Inputs: an IR of the program, a target constraints object (e.g. number of registers,
 which are available, reserved registers, call clobbers, etc.)
Output: Result<ProgramAllocation, RegisterAllocationError>

1. liveness.rs: Compute liveness across the control-flow graph.
2. live_intervals.rs: Build live intervals and sort them by start position.
3. allocation.rs: Scan intervals.
   - Expire finished intervals and free their registers.
   - Assign a free register when available.
   - Otherwise, spill whichever interval ends furthest away.
4. target.rs: Describe and validate the Microsoft x64 register constraints.

The result describes locations; the future native code generator must emit ABI
moves, spill loads/stores, callee-save prologs/epilogs, and stack frame layout.
*/

mod allocation;
mod live_intervals;
mod liveness;
mod target;

#[cfg(test)]
mod tests;

use crate::ir::{CheckedIr, SymbolId, VirtualRegId};
use std::collections::HashMap;

pub use target::{PhysicalRegId, TargetConstraints};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpillSlotId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalLocation {
    Register(PhysicalRegId),
    Spill(SpillSlotId),
}

#[derive(Debug, PartialEq, Eq)]
pub struct FunctionAllocation {
    pub register_locations: HashMap<VirtualRegId, PhysicalLocation>,
    /// Each slot holds one 8-byte value; frame layout assigns its stack offset.
    pub spill_slot_count: usize,
    /// Allocated callee-saved registers to preserve on entry and every return.
    /// Frame construction separately preserves the reserved frame pointer RBP.
    pub used_callee_saved_registers: Vec<PhysicalRegId>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ProgramAllocation {
    pub function_allocations: HashMap<SymbolId, FunctionAllocation>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RegisterAllocationError {
    MissingIntervals(SymbolId),
    InvalidTarget(String),
    InvalidIntervals { function: SymbolId, reason: String },
}

pub fn allocate_registers(
    ir: &CheckedIr,
    target_constraints: &TargetConstraints,
) -> Result<ProgramAllocation, RegisterAllocationError> {
    let function_liveness = liveness::compute_liveness(ir);
    let function_live_intervals = live_intervals::build_live_intervals(ir, &function_liveness);

    allocation::allocate(ir, &function_live_intervals, target_constraints)
}
