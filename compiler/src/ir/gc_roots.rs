//! Roots each `Ref` live across a call: save before, reload after, null the
//! pass's other slots (`runtime-abi.md` §3.3). Slots below `root_slots` are lowering's.

use std::collections::{BTreeMap, HashMap};

use super::liveness::live_across_calls;
use super::*;

pub fn insert_gc_roots(ir: CheckedIr) -> ProgramIr {
    let mut program = ir.into_program();
    for function in &mut program.functions {
        insert_function_roots(function);
    }
    program
}

fn insert_function_roots(function: &mut FunctionIr) {
    let saved_at = refs_live_across_calls(function);
    let first_slot = function.root_slots;
    let mut slots: HashMap<VirtualRegId, u32> = HashMap::new();
    for saved in saved_at.values() {
        for &register in saved {
            let next = first_slot + slots.len() as u32;
            slots.entry(register).or_insert(next);
        }
    }
    let owned = first_slot..first_slot + slots.len() as u32;

    for (block_index, block) in function.blocks.iter_mut().enumerate() {
        let mut instructions = Vec::with_capacity(block.instructions.len());
        for (index, instruction) in std::mem::take(&mut block.instructions)
            .into_iter()
            .enumerate()
        {
            let Some(saved) = saved_at.get(&(block_index, index)) else {
                instructions.push(instruction);
                continue;
            };
            let line = instruction.line;
            let root = |kind| Instruction { kind, line };
            let saved_slots: Vec<u32> = saved.iter().map(|register| slots[register]).collect();
            for (&register, &slot) in saved.iter().zip(&saved_slots) {
                instructions.push(root(InstructionKind::RootStore {
                    slot,
                    value: Operand::Value(register),
                }));
            }
            for slot in owned.clone().filter(|slot| !saved_slots.contains(slot)) {
                instructions.push(root(InstructionKind::RootStore {
                    slot,
                    value: Operand::Null,
                }));
            }
            instructions.push(instruction);
            for (&register, &slot) in saved.iter().zip(&saved_slots) {
                instructions.push(root(InstructionKind::RootLoad {
                    dst: register,
                    slot,
                }));
            }
        }
        block.instructions = instructions;
    }
    function.root_slots = owned.end;
}

/// The `Ref`s whose values must survive each call. Keys sort in program order.
fn refs_live_across_calls(function: &FunctionIr) -> BTreeMap<(usize, usize), Vec<VirtualRegId>> {
    let mut across = live_across_calls(function);
    for values in across.values_mut() {
        values.retain(|register| function.register_types[register.0] == IrType::Ref);
    }
    across
}
