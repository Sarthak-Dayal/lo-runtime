//! One live interval per virtual register. Positions number instructions in
//! block order, and each terminator gets its own position. A value live into or
//! out of a block covers that block's first or last position, so an interval
//! may be longer than strictly needed (for example across a loop) but never
//! shorter.

use crate::ir::liveness::LivenessInfo;
use crate::ir::{FunctionIr, VirtualRegId};
use std::collections::HashMap;

/// Positions `start..=end`, both inclusive.
pub struct LiveInterval {
    pub register: VirtualRegId,
    pub start: usize,
    pub end: usize,
}

/// Sorted by start, then register number.
pub(super) fn build_function_live_intervals(
    function: &FunctionIr,
    liveness: &LivenessInfo,
) -> Vec<LiveInterval> {
    let mut intervals: HashMap<VirtualRegId, LiveInterval> = HashMap::new();
    let mut position = 0;
    let entry = function.entry.0;

    for (block_id, block) in function.blocks.iter().enumerate() {
        for &incoming_register in &liveness.live_in[block_id] {
            include_position(&mut intervals, incoming_register, position);
        }

        if block_id == entry {
            // Parameters arrive at function entry, so they are live from there.
            for &parameter in &function.params {
                include_position(&mut intervals, parameter, position);
            }
        }

        for instruction in &block.instructions {
            for used_register in instruction.kind.used_registers() {
                include_position(&mut intervals, used_register, position);
            }
            if let Some(destination) = instruction.kind.destination() {
                include_position(&mut intervals, destination, position);
            }
            position += 1;
        }

        for terminator_register in block.terminator.used_registers() {
            include_position(&mut intervals, terminator_register, position);
        }

        for &outgoing_register in &liveness.live_out[block_id] {
            include_position(&mut intervals, outgoing_register, position);
        }

        position += 1;
    }

    let mut intervals: Vec<LiveInterval> = intervals.into_values().collect();

    intervals.sort_by_key(|interval| (interval.start, interval.register.0));

    intervals
}

fn include_position(
    intervals: &mut HashMap<VirtualRegId, LiveInterval>,
    register_id: VirtualRegId,
    position: usize,
) {
    let interval = intervals.entry(register_id).or_insert(LiveInterval {
        register: register_id,
        start: position,
        end: position,
    });

    interval.start = interval.start.min(position);
    interval.end = interval.end.max(position);
}
