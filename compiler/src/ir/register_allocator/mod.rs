//! Poletto-Sarkar linear-scan register allocation for the System V AMD64 ABI.
//!
//! Input: the rooted, checked IR and `TargetConstraints::system_v()`. Output: one
//! location (register or spill slot) per virtual register, per function.
//!
//! 1. liveness.rs: liveness across the control-flow graph, and which values are
//!    live across each call.
//! 2. live_intervals.rs: one conservative interval per virtual register.
//! 3. allocation.rs: scan intervals by start. Expire finished ones; give the
//!    current one the first free register it may use, in pool order; otherwise
//!    spill whichever of it and the active intervals ends last. The calling
//!    convention rule: a value live across a call may use only callee-saved
//!    registers; any other value takes caller-saved ones first.
//! 4. target.rs: the register pool and the callee-saved set.
//!
//! Code generation (`codegen::frame`) turns the result into spill addresses and
//! saves and restores the callee-saved registers the allocation used.

mod allocation;
mod live_intervals;
mod liveness;
pub mod spill_all;
mod target;

use crate::ir::{CheckedIr, SymbolId, VirtualRegId};
use std::collections::HashMap;

pub use liveness::live_across_calls;
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
