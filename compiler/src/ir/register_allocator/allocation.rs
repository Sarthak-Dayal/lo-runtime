//! Linear scan over one function's intervals: one location per virtual
//! register, honoring the caller-/callee-saved split.

use super::live_intervals::LiveInterval;
use super::target::TargetConstraints;
use super::{FunctionAllocation, PhysicalLocation, PhysicalRegId, SpillSlotId};
use crate::ir::liveness::live_across_calls;
use crate::ir::{FunctionIr, VirtualRegId};
use std::collections::{HashMap, HashSet};

/// An interval currently occupying a physical register.
struct ActiveInterval<'a> {
    live_interval: &'a LiveInterval,
    physical_register: PhysicalRegId,
}

/// The calling-convention rule: a value read after some call returns must not sit
/// in a caller-saved register, which the callee may overwrite. A function with
/// calls also calls `lo_push_frame` in its prologue, after the parameters reach
/// their locations, so its parameters must survive a call too.
///
/// The allocator runs on rooted IR: `gc_roots` reloads each `Ref` from its root
/// slot right after a call, so a `Ref`'s pre-call register is never read again
/// and only `Int`/`Bool` values end up in this set.
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

/// `live_intervals` must be sorted by start, as `build_function_live_intervals`
/// returns them.
pub(super) fn allocate_function(
    function: &FunctionIr,
    live_intervals: &[LiveInterval],
    target_constraints: &TargetConstraints,
) -> FunctionAllocation {
    let surviving_calls = values_surviving_calls(function);
    let mut function_allocation = FunctionAllocation {
        register_locations: HashMap::new(),
        spill_slot_count: 0,
        used_callee_saved_registers: Vec::new(),
    };
    let mut active_intervals = Vec::new();
    let mut free_physical_registers = target_constraints.allocatable_registers.clone();

    for current_interval in live_intervals {
        expire_finished_intervals(
            current_interval.start,
            &mut active_intervals,
            &mut free_physical_registers,
        );
        let survives_call = surviving_calls.contains(&current_interval.register);
        let may_use = |register: PhysicalRegId| {
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
            .find(|&register| may_use(register) && free_physical_registers.contains(&register));

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
                may_use,
                &mut active_intervals,
                &mut function_allocation,
            ),
        }
    }

    function_allocation.used_callee_saved_registers =
        used_callee_saved(&function_allocation, target_constraints);
    function_allocation
}

/// Computed from the final map: an interval evicted later is spilled for its
/// entire lifetime, so its old register may no longer need saving.
fn used_callee_saved(
    allocation: &FunctionAllocation,
    target_constraints: &TargetConstraints,
) -> Vec<PhysicalRegId> {
    target_constraints
        .callee_saved_registers
        .iter()
        .copied()
        .filter(|&register| {
            allocation
                .register_locations
                .values()
                .any(|location| *location == PhysicalLocation::Register(register))
        })
        .collect()
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

/// No usable register is free: spill whichever of the current interval and the
/// active intervals holding a usable register ends last.
fn spill_at_interval<'a>(
    current_interval: &'a LiveInterval,
    may_use: impl Fn(PhysicalRegId) -> bool,
    active_intervals: &mut Vec<ActiveInterval<'a>>,
    function_allocation: &mut FunctionAllocation,
) {
    // `active_intervals` is sorted by end, so the last candidate ends latest.
    let candidate = active_intervals
        .iter()
        .rposition(|active_interval| may_use(active_interval.physical_register));
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
    // Spill the evicted value for its whole lifetime; no code is emitted until
    // the map is complete, so nothing refers to its old register.
    assign_spill_slot(evicted_interval.live_interval, function_allocation);
    assign_register(
        current_interval,
        evicted_interval.physical_register,
        active_intervals,
        function_allocation,
    );
}
