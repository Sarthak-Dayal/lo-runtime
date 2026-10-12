use super::live_intervals::LiveInterval;
use super::liveness::live_across_calls;
use super::target::debug_check;
use super::{
    FunctionAllocation, PhysicalLocation, PhysicalRegId, ProgramAllocation,
    RegisterAllocationError, SpillSlotId, TargetConstraints,
};
use crate::ir::{CheckedIr, FunctionIr, SymbolId, VirtualRegId};
use std::collections::{HashMap, HashSet};

// An interval currently occupying a physical register.
struct ActiveInterval<'a> {
    live_interval: &'a LiveInterval,
    physical_register: PhysicalRegId,
}

// Allocate one permanent location per virtual register, separately per function.
// Intervals must cover the same checked IR; debug builds check that they do.
pub fn allocate(
    checked_ir: &CheckedIr,
    function_live_intervals: &HashMap<SymbolId, Vec<LiveInterval>>,
    target_constraints: &TargetConstraints,
) -> Result<ProgramAllocation, RegisterAllocationError> {
    debug_check(target_constraints);

    let mut function_allocations = HashMap::new();
    for function in &checked_ir.program().functions {
        let live_intervals = function_live_intervals
            .get(&function.symbol)
            .ok_or(RegisterAllocationError::MissingIntervals(function.symbol))?;
        // The intervals come from `allocate_registers` over this same IR, so
        // re-deriving and comparing them only guards against bugs in this module.
        if cfg!(debug_assertions) {
            validate_intervals(function, live_intervals)?;
        }

        let function_allocation = allocate_function(
            live_intervals,
            target_constraints,
            &values_surviving_calls(function),
        );
        function_allocations.insert(function.symbol, function_allocation);
    }

    Ok(ProgramAllocation {
        function_allocations,
    })
}

// The calling-convention rule: a value read after some call returns must not sit
// in a caller-saved register, which the callee may overwrite. A function with
// calls also calls `lo_push_frame` in its prologue, after the parameters reach
// their locations, so its parameters must survive a call too.
fn values_surviving_calls(function: &FunctionIr) -> HashSet<VirtualRegId> {
    let mut surviving: HashSet<_> = live_across_calls(function)
        .into_values()
        .flatten()
        .collect();
    if function.has_calls() {
        surviving.extend(&function.params);
    }
    surviving
}

pub(super) fn allocate_function(
    live_intervals: &[LiveInterval],
    target_constraints: &TargetConstraints,
    surviving_calls: &HashSet<VirtualRegId>,
) -> FunctionAllocation {
    let mut function_allocation = FunctionAllocation {
        register_locations: HashMap::new(),
        spill_slot_count: 0,
        used_callee_saved_registers: Vec::new(),
    };
    let mut active_intervals = Vec::new();
    let mut free_physical_registers = target_constraints.allocatable_registers.clone();

    let mut sorted_intervals: Vec<_> = live_intervals.iter().collect();
    sorted_intervals.sort_by_key(|interval| (interval.start, interval.register.0));

    for current_interval in sorted_intervals {
        expire_finished_intervals(
            current_interval.start,
            &mut active_intervals,
            &mut free_physical_registers,
        );
        let survives_call = surviving_calls.contains(&current_interval.register);
        let allowed = |register: PhysicalRegId| {
            !survives_call
                || target_constraints
                    .callee_saved_registers
                    .contains(&register)
        };
        // Pool order lists caller-saved registers first, so a value that does not
        // survive a call takes one of those before a callee-saved one, which would
        // cost a save and restore.
        let free_register = target_constraints
            .allocatable_registers
            .iter()
            .copied()
            .find(|&register| allowed(register) && free_physical_registers.contains(&register));

        match free_register {
            Some(physical_register) => {
                free_physical_registers.retain(|&register| register != physical_register);
                assign_register(
                    current_interval,
                    physical_register,
                    &mut active_intervals,
                    &mut function_allocation,
                );
            }
            None => spill_at_interval(
                current_interval,
                allowed,
                &mut active_intervals,
                &mut function_allocation,
            ),
        }
    }

    // Compute this from the final map: an interval evicted later is spilled for
    // its entire lifetime, so its old register may no longer need saving.
    for &physical_register in &target_constraints.callee_saved_registers {
        if function_allocation
            .register_locations
            .values()
            .any(|location| *location == PhysicalLocation::Register(physical_register))
        {
            function_allocation
                .used_callee_saved_registers
                .push(physical_register);
        }
    }

    function_allocation
}

fn expire_finished_intervals(
    current_start_position: usize,
    active_intervals: &mut Vec<ActiveInterval<'_>>,
    free_physical_registers: &mut Vec<PhysicalRegId>,
) {
    active_intervals.retain(|active_interval| {
        // Inclusive endpoints: equality still means the intervals overlap.
        if active_interval.live_interval.end < current_start_position {
            free_physical_registers.push(active_interval.physical_register);
            false
        } else {
            true
        }
    });
}

fn assign_register<'a>(
    live_interval: &'a LiveInterval,
    physical_register: PhysicalRegId,
    active_intervals: &mut Vec<ActiveInterval<'a>>,
    function_allocation: &mut FunctionAllocation,
) {
    function_allocation.register_locations.insert(
        live_interval.register,
        PhysicalLocation::Register(physical_register),
    );
    active_intervals.push(ActiveInterval {
        live_interval,
        physical_register,
    });
    active_intervals.sort_by_key(|active_interval| {
        (
            active_interval.live_interval.end,
            active_interval.live_interval.register.0,
        )
    });
}

fn assign_spill_slot(live_interval: &LiveInterval, function_allocation: &mut FunctionAllocation) {
    let spill_slot = SpillSlotId(function_allocation.spill_slot_count);
    function_allocation.spill_slot_count += 1;
    function_allocation
        .register_locations
        .insert(live_interval.register, PhysicalLocation::Spill(spill_slot));
}

// No allowed register is free: spill whichever of the current interval and the
// active intervals holding an allowed register ends last.
fn spill_at_interval<'a>(
    current_interval: &'a LiveInterval,
    allowed: impl Fn(PhysicalRegId) -> bool,
    active_intervals: &mut Vec<ActiveInterval<'a>>,
    function_allocation: &mut FunctionAllocation,
) {
    // `active_intervals` is sorted by end, so the last candidate ends latest.
    let candidate = active_intervals
        .iter()
        .rposition(|active_interval| allowed(active_interval.physical_register));
    let Some(index) = candidate else {
        assign_spill_slot(current_interval, function_allocation);
        return;
    };

    if active_intervals[index].live_interval.end <= current_interval.end {
        // On equal end positions, keep the existing assignment.
        assign_spill_slot(current_interval, function_allocation);
        return;
    }

    let evicted_interval = active_intervals.remove(index);
    // This replaces the old register assignment for the WHOLE interval.
    // Code generation consumes the completed map; no machine code exists yet.
    assign_spill_slot(evicted_interval.live_interval, function_allocation);
    assign_register(
        current_interval,
        evicted_interval.physical_register,
        active_intervals,
        function_allocation,
    );
}

fn validate_intervals(
    function: &FunctionIr,
    live_intervals: &[LiveInterval],
) -> Result<(), RegisterAllocationError> {
    let invalid = |reason: String| RegisterAllocationError::InvalidIntervals {
        function: function.symbol,
        reason,
    };
    let position_count: usize = function
        .blocks
        .iter()
        .map(|block| block.instructions.len() + 1)
        .sum();
    let mut intervals_by_register = HashMap::new();
    for interval in live_intervals {
        if interval.register.0 >= function.register_types.len()
            || interval.start > interval.end
            || interval.end >= position_count
        {
            return Err(invalid(format!(
                "invalid bounds or register: v{}",
                interval.register.0
            )));
        }
        if intervals_by_register
            .insert(interval.register, interval)
            .is_some()
        {
            return Err(invalid(format!(
                "duplicate interval for v{}",
                interval.register.0
            )));
        }
    }

    // Recompute the required coverage to reject stale or incomplete interval maps.
    // Extra conservative coverage is allowed, but every required position must fit.
    let liveness = super::liveness::compute_function_liveness(function);
    let required_intervals =
        super::live_intervals::build_function_live_intervals(function, &liveness);
    for required in required_intervals {
        let Some(provided) = intervals_by_register.get(&required.register) else {
            return Err(invalid(format!(
                "missing interval for v{}",
                required.register.0
            )));
        };
        if provided.start > required.start || provided.end < required.end {
            return Err(invalid(format!(
                "interval does not cover v{}",
                required.register.0
            )));
        }
    }
    Ok(())
}
