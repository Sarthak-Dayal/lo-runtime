use super::liveness::LivenessInfo;
use crate::ir::{CheckedIr, FunctionIr, SymbolId, VirtualRegId};
use std::collections::HashMap;

/// One conservative, inclusive interval in instruction/terminator positions.
pub struct LiveInterval {
    pub register: VirtualRegId,
    pub start: usize,
    pub end: usize,
}

/// Requires liveness computed from the same checked program.
pub fn build_live_intervals(
    ir: &CheckedIr,
    liveness: &HashMap<SymbolId, LivenessInfo>,
) -> HashMap<SymbolId, Vec<LiveInterval>> {
    let mut intervals = HashMap::new();
    for function in &ir.program().functions {
        let liveness_info = liveness
            .get(&function.symbol)
            .expect("liveness must contain every defined function");
        let function_intervals = build_function_live_intervals(function, liveness_info);
        intervals.insert(function.symbol, function_intervals);
    }
    intervals
}

pub(super) fn build_function_live_intervals(
    function: &FunctionIr,
    liveness: &LivenessInfo,
) -> Vec<LiveInterval> {
    let mut intervals: HashMap<VirtualRegId, LiveInterval> = HashMap::new();
    let mut position = 0;

    for (block_id, block) in function.blocks.iter().enumerate() {
        for &incoming_register in &liveness.live_in[block_id] {
            include_position(&mut intervals, incoming_register, position);
        }

        let func_entry_block = function.entry.0;
        if block_id == func_entry_block {
            // parameters need to be live at the start of the function, so include them in the live intervals
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
