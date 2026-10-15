//! Backward dataflow liveness per block, plus the one fact both the GC-root pass
//! and the register allocator need: which values are live after each call.

use crate::ir::{BasicBlock, FunctionIr, InstructionKind, VirtualRegId};
use std::collections::{BTreeMap, HashSet};

pub struct LivenessInfo {
    /// For each block, the registers live at its end.
    pub live_out: Vec<HashSet<VirtualRegId>>,
    /// For each block, the registers live at its start.
    pub live_in: Vec<HashSet<VirtualRegId>>,
}

/// Per block: `live_in` is the use set plus whatever is live out and not
/// defined in the block. Repeats over all blocks until nothing changes.
pub fn compute_function_liveness(function: &FunctionIr) -> LivenessInfo {
    let mut live_out: Vec<HashSet<VirtualRegId>> = vec![HashSet::new(); function.blocks.len()];
    let mut live_in: Vec<HashSet<VirtualRegId>> = vec![HashSet::new(); function.blocks.len()];

    loop {
        let mut changed = false;

        for (block_id, block) in function.blocks.iter().enumerate().rev() {
            let mut new_out = HashSet::new();
            let mut new_in = use_set(block);
            let def = def_set(block);

            for successor in block.terminator.successors() {
                new_out.extend(live_in[successor.0].iter().copied());
            }

            new_in.extend(new_out.difference(&def).copied());

            if new_in != live_in[block_id] || new_out != live_out[block_id] {
                changed = true;
            }

            live_out[block_id] = new_out;
            live_in[block_id] = new_in;
        }

        if !changed {
            break;
        }
    }

    LivenessInfo { live_out, live_in }
}

/// For each `Call`, keyed by (block, instruction index), the values whose current
/// contents are read after the call returns: live after it, minus its result.
/// Sorted by register number; keys sort in program order. The GC-roots pass roots
/// the `Ref`s among them; the allocator keeps all of them out of caller-saved registers.
pub fn live_across_calls(function: &FunctionIr) -> BTreeMap<(usize, usize), Vec<VirtualRegId>> {
    let liveness = compute_function_liveness(function);
    let mut across = BTreeMap::new();
    for (block_index, block) in function.blocks.iter().enumerate() {
        let mut live = liveness.live_out[block_index].clone();
        live.extend(block.terminator.used_registers());
        for (index, instruction) in block.instructions.iter().enumerate().rev() {
            let destination = instruction.kind.destination();
            if matches!(instruction.kind, InstructionKind::Call { .. }) {
                let mut values: Vec<_> = live
                    .iter()
                    .copied()
                    .filter(|&register| Some(register) != destination)
                    .collect();
                values.sort_by_key(|register| register.0);
                across.insert((block_index, index), values);
            }
            if let Some(destination) = destination {
                live.remove(&destination);
            }
            live.extend(instruction.kind.used_registers());
        }
    }
    across
}

/// Registers read before their first definition in this block (the use set).
fn use_set(block: &BasicBlock) -> HashSet<VirtualRegId> {
    let mut used = HashSet::new();
    let mut defined_so_far = HashSet::new();

    for instruction in &block.instructions {
        // use before def, must have been defined in a predecessor block
        for register in instruction.kind.used_registers() {
            if !defined_so_far.contains(&register) {
                used.insert(register);
            }
        }

        // write, so add to defined_so_far for subsequent instructions in this block
        if let Some(destination) = instruction.kind.destination() {
            defined_so_far.insert(destination);
        }
    }

    // if the terminator itself uses any registers, they are also considered used in this block
    for register in block.terminator.used_registers() {
        if !defined_so_far.contains(&register) {
            used.insert(register);
        }
    }
    used
}

/// All registers written by instructions in this block (the def set).
fn def_set(block: &BasicBlock) -> HashSet<VirtualRegId> {
    block
        .instructions
        .iter()
        .filter_map(|instruction| instruction.kind.destination())
        .collect()
}

/// Drop unread copies so dead local defaults do not extend live intervals.
pub(crate) fn remove_dead_copies(function: &mut FunctionIr) {
    loop {
        let liveness = compute_function_liveness(function);
        let mut changed = false;
        for (index, block) in function.blocks.iter_mut().enumerate() {
            let mut live = liveness.live_out[index].clone();
            live.extend(block.terminator.used_registers());
            let mut kept = Vec::with_capacity(block.instructions.len());
            for instruction in block.instructions.drain(..).rev() {
                let destination = instruction.kind.destination();
                if matches!(instruction.kind, InstructionKind::Copy { .. })
                    && destination.is_some_and(|dst| !live.contains(&dst))
                {
                    changed = true;
                    continue;
                }
                if let Some(dst) = destination {
                    live.remove(&dst);
                }
                live.extend(instruction.kind.used_registers());
                kept.push(instruction);
            }
            kept.reverse();
            block.instructions = kept;
        }
        if !changed {
            break;
        }
    }
}
