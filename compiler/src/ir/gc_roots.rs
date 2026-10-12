//! Makes every call GC-safe. Any call may run the moving collector, which only
//! sees references stored in shadow-frame root slots, and may move the objects
//! they point to. So for each `Ref` still needed after a call, this pass stores
//! it to its own root slot before the call and reloads it from that slot after
//! the call (the collector updated the slot if the object moved). Every other
//! slot is set to null, so an object the function no longer needs is not kept
//! alive (`runtime-abi.md` §3.3). Each `Ref` gets one slot for the whole function.

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
    let live_refs_at_call = refs_live_across_calls(function);
    let slots = assign_root_slots(&live_refs_at_call);
    let all_slots = 0..slots.len() as u32;

    for (block_index, block) in function.blocks.iter_mut().enumerate() {
        let mut instructions = Vec::with_capacity(block.instructions.len());
        for (index, instruction) in std::mem::take(&mut block.instructions)
            .into_iter()
            .enumerate()
        {
            let Some(saved) = live_refs_at_call.get(&(block_index, index)) else {
                instructions.push(instruction);
                continue;
            };
            let line = instruction.line;
            let at_line = |kind| Instruction { kind, line };
            let saved_slots: Vec<u32> = saved.iter().map(|register| slots[register]).collect();
            for (&register, &slot) in saved.iter().zip(&saved_slots) {
                instructions.push(at_line(InstructionKind::RootStore {
                    slot,
                    value: Operand::Value(register),
                }));
            }
            for slot in all_slots.clone().filter(|slot| !saved_slots.contains(slot)) {
                instructions.push(at_line(InstructionKind::RootStore {
                    slot,
                    value: Operand::Null,
                }));
            }
            instructions.push(instruction);
            for (&register, &slot) in saved.iter().zip(&saved_slots) {
                instructions.push(at_line(InstructionKind::RootLoad {
                    dst: register,
                    slot,
                }));
            }
        }
        block.instructions = instructions;
    }
    function.root_slots = all_slots.end;
}

/// One slot per `Ref` that is live across some call, numbered in the order the
/// calls appear.
fn assign_root_slots(
    live_refs_at_call: &BTreeMap<(usize, usize), Vec<VirtualRegId>>,
) -> HashMap<VirtualRegId, u32> {
    let mut slots = HashMap::new();
    for saved in live_refs_at_call.values() {
        for &register in saved {
            let next = slots.len() as u32;
            slots.entry(register).or_insert(next);
        }
    }
    slots
}

/// The `Ref`s whose values must survive each call. Keys sort in program order.
fn refs_live_across_calls(function: &FunctionIr) -> BTreeMap<(usize, usize), Vec<VirtualRegId>> {
    let mut across = live_across_calls(function);
    for values in across.values_mut() {
        values.retain(|register| function.register_types[register.0] == IrType::Ref);
    }
    across
}
