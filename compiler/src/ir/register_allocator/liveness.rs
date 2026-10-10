use crate::ir::{BasicBlock, CheckedIr, FunctionIr, SymbolId, VirtualRegId};
use std::collections::{HashMap, HashSet};

pub struct LivenessInfo {
    // For each block, the set of registers that are live at the end of the block.
    pub live_out: Vec<HashSet<VirtualRegId>>,
    // For each block, the set of registers that are live at the start of the block.
    pub live_in: Vec<HashSet<VirtualRegId>>,
}

// Computes liveness for each defined function, keyed by its symbol ID.
pub fn compute_liveness(ir: &CheckedIr) -> HashMap<SymbolId, LivenessInfo> {
    let mut functions = HashMap::new();
    for function in &ir.program().functions {
        functions.insert(function.symbol, compute_function_liveness(function));
    }
    functions
}

// Computes the liveness information for each block in one function.
pub fn compute_function_liveness(function: &FunctionIr) -> LivenessInfo {
    let mut live_out: Vec<HashSet<VirtualRegId>> = vec![HashSet::new(); function.blocks.len()];
    let mut live_in: Vec<HashSet<VirtualRegId>> = vec![HashSet::new(); function.blocks.len()];

    loop {
        let mut changed = false;

        for (block_id, block) in function.blocks.iter().enumerate().rev() {
            let mut new_out = HashSet::new();
            let mut new_in = get_used(block);
            let def = get_def(block);

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

// Registers read before their first definition in this block (the use set)
pub(super) fn get_used(block: &BasicBlock) -> HashSet<VirtualRegId> {
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

// all registers written by instructions in this block (the def set)
pub(super) fn get_def(block: &BasicBlock) -> HashSet<VirtualRegId> {
    block
        .instructions
        .iter()
        .filter_map(|instruction| instruction.kind.destination())
        .collect()
}
