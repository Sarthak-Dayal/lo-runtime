//! Poletto-Sarkar linear-scan register allocation (one pass over live intervals
//! sorted by start; Poletto and Sarkar, 1999) for the System V AMD64 ABI.
//!
//! Input: the rooted, checked IR and `TargetConstraints::system_v()`. Output: one
//! location (register or spill slot) per virtual register, per function.
//!
//! 1. `ir::liveness`: liveness across the control-flow graph, and which values
//!    are live across each call.
//! 2. live_intervals.rs: one conservative interval per virtual register.
//! 3. allocation.rs: scan intervals by start. Expire finished ones; give the
//!    current one the first free register it may use, in pool order; otherwise
//!    spill whichever of it and the active intervals ends last. The calling
//!    convention rule: a value live across a call may use only callee-saved
//!    registers; any other value takes caller-saved ones first.
//! 4. target.rs: the register pool and the callee-saved set.
//!
//! Under `-g`, named values (`this`, formals, locals) are given their slot first
//! and left out of the scan, so each stays at one stack address for the whole
//! function, which is what the debug info describes. A named `Ref` in a function
//! with calls lives in a root slot; every other named value gets a home slot.
//!
//! Code generation (`codegen::frame`) turns the result into spill addresses and
//! saves and restores the callee-saved registers the allocation used.

mod allocation;
mod live_intervals;
pub mod spill_all;
mod target;

use crate::ir::liveness::compute_function_liveness;
use crate::ir::{CheckedIr, DebugMode, FunctionIr, SymbolId, VirtualRegId};
use std::collections::HashMap;

pub use target::{PhysicalRegId, TargetConstraints};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpillSlotId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalLocation {
    Register(PhysicalRegId),
    Spill(SpillSlotId),
    /// Shadow-frame root slot `n` (only named `Ref`s under `-g`).
    Root(u32),
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

pub fn allocate_registers(
    ir: &CheckedIr,
    target: &TargetConstraints,
    debug: DebugMode,
) -> ProgramAllocation {
    target::debug_check(target);
    let function_allocations = ir
        .program()
        .functions
        .iter()
        .map(|function| {
            let liveness = compute_function_liveness(function);
            let named_slots = named_value_slots(function, debug);
            let intervals: Vec<_> =
                live_intervals::build_function_live_intervals(function, &liveness)
                    .into_iter()
                    .filter(|interval| !named_slots.contains_key(&interval.register))
                    .collect();
            let allocation =
                allocation::allocate_function(function, &intervals, target, named_slots);
            (function.symbol, allocation)
        })
        .collect();
    ProgramAllocation {
        function_allocations,
    }
}

/// Under `-g`, where each named value lives: root slot `i` for the `i`th of
/// `FunctionIr::named_refs_in_root_slots`, and home slots `0..K` (spill slots)
/// for the other K named values, in register order. Without `-g`, none.
fn named_value_slots(
    function: &FunctionIr,
    debug: DebugMode,
) -> HashMap<VirtualRegId, PhysicalLocation> {
    if debug == DebugMode::Off {
        return HashMap::new();
    }
    let in_root_slots = function.named_refs_in_root_slots();
    let root_slots = in_root_slots
        .iter()
        .enumerate()
        .map(|(slot, &register)| (register, PhysicalLocation::Root(slot as u32)));
    let gets_home_slot = |register: &VirtualRegId| {
        function.register_names[register.0].is_some() && !in_root_slots.contains(register)
    };
    let home_slots = (0..function.register_names.len())
        .map(VirtualRegId)
        .filter(gets_home_slot)
        .enumerate()
        .map(|(slot, register)| (register, PhysicalLocation::Spill(SpillSlotId(slot))));
    root_slots.chain(home_slots).collect()
}
