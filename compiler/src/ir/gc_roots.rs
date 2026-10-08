//! Shadow-stack roots around calls (`runtime-abi.md` §3.3). The collector moves
//! objects and only sees root slots, so every `Ref` live across a call is saved
//! to a slot before it and reloaded after it. Every other slot this pass owns
//! is nulled before the call, so the collector never follows a dead pointer.
//! Slots below the function's existing `root_slots` (the startup function's
//! `in`/`out`/`err`) belong to lowering and are left alone.

use std::collections::{BTreeMap, HashMap};

use super::register_allocator::compute_function_liveness;
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

/// For each call, keyed by (block, instruction index): the `Ref` registers live
/// after it, excluding the call's own result, in register order. Iterating the
/// map visits calls in program order.
fn refs_live_across_calls(function: &FunctionIr) -> BTreeMap<(usize, usize), Vec<VirtualRegId>> {
    let liveness = compute_function_liveness(function);
    let mut saved_at = BTreeMap::new();
    for (block_index, block) in function.blocks.iter().enumerate() {
        let mut live = liveness.live_out[block_index].clone();
        live.extend(block.terminator.used_registers());
        for (index, instruction) in block.instructions.iter().enumerate().rev() {
            let destination = instruction.kind.destination();
            if matches!(instruction.kind, InstructionKind::Call { .. }) {
                let mut saved: Vec<_> = live
                    .iter()
                    .copied()
                    .filter(|&register| Some(register) != destination)
                    .filter(|register| function.register_types[register.0] == IrType::Ref)
                    .collect();
                saved.sort_by_key(|register| register.0);
                saved_at.insert((block_index, index), saved);
            }
            if let Some(destination) = destination {
                live.remove(&destination);
            }
            live.extend(instruction.kind.used_registers());
        }
    }
    saved_at
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAKE: SymbolId = SymbolId(0); // () -> Ref
    const USE: SymbolId = SymbolId(1); // (Ref) -> Int32
    const TICK: SymbolId = SymbolId(2); // () -> Int32
    const MAIN: SymbolId = SymbolId(3);

    fn program(function: FunctionIr, main_signature: Signature) -> CheckedIr {
        let signature = |params, result| Signature { params, result };
        let function_symbol = |name: &str, id| Symbol {
            name: name.into(),
            kind: SymbolKind::Function(SignatureId(id)),
        };
        ProgramIr {
            symbols: vec![
                function_symbol("make", 0),
                function_symbol("use", 1),
                function_symbol("tick", 2),
                function_symbol("main", 3),
            ],
            signatures: vec![
                signature(vec![], Some(IrType::Ref)),
                signature(vec![IrType::Ref], Some(IrType::Int32)),
                signature(vec![], Some(IrType::Int32)),
                main_signature,
            ],
            functions: vec![function],
            data: vec![],
            startup: None,
        }
        .verify()
        .unwrap()
    }

    fn call(dst: usize, target: SymbolId, args: Vec<Operand>) -> Instruction {
        Instruction {
            kind: InstructionKind::Call {
                dst: Some(VirtualRegId(dst)),
                target: CallTarget::Direct(target),
                args,
            },
            line: 1,
        }
    }

    fn rendered(program: &ProgramIr) -> Vec<String> {
        program.functions[0].blocks[0]
            .instructions
            .iter()
            .map(|instruction| format!("{:?}", instruction.kind))
            .collect()
    }

    #[test]
    fn ref_live_across_a_call_is_saved_and_reloaded() {
        // t1 = make(); t2 = use(t1); ret t1
        let function = FunctionIr {
            symbol: MAIN,
            params: vec![VirtualRegId(0)],
            register_types: vec![IrType::Ref, IrType::Ref, IrType::Int32],
            register_names: vec![None; 3],
            root_slots: 0,
            blocks: vec![BasicBlock {
                instructions: vec![
                    call(1, MAKE, vec![]),
                    call(2, USE, vec![Operand::Value(VirtualRegId(1))]),
                ],
                terminator: Terminator::Return(Some(Operand::Value(VirtualRegId(1)))),
            }],
            entry: BlockId(0),
        };
        let main = Signature {
            params: vec![IrType::Ref],
            result: Some(IrType::Ref),
        };
        let out = insert_gc_roots(program(function, main));
        assert_eq!(out.functions[0].root_slots, 1);
        let t1 = VirtualRegId(1);
        assert_eq!(
            rendered(&out),
            [
                // make(): t1 is its result and `this` is dead, so nothing is saved.
                format!(
                    "{:?}",
                    InstructionKind::RootStore {
                        slot: 0,
                        value: Operand::Null
                    }
                ),
                format!("{:?}", call(1, MAKE, vec![]).kind),
                format!(
                    "{:?}",
                    InstructionKind::RootStore {
                        slot: 0,
                        value: Operand::Value(t1)
                    }
                ),
                format!("{:?}", call(2, USE, vec![Operand::Value(t1)]).kind),
                format!("{:?}", InstructionKind::RootLoad { dst: t1, slot: 0 }),
            ]
        );
        out.validate().unwrap();
    }

    #[test]
    fn loop_carried_ref_is_saved_and_lowering_slots_are_untouched() {
        // .L0: t2 = make(); br .L1
        // .L1: t3 = tick(); cbr t1, .L1, .L2
        // .L2: ret t2
        let function = FunctionIr {
            symbol: MAIN,
            params: vec![VirtualRegId(0), VirtualRegId(1)],
            register_types: vec![IrType::Ref, IrType::Bool, IrType::Ref, IrType::Int32],
            register_names: vec![None; 4],
            root_slots: 2,
            blocks: vec![
                BasicBlock {
                    instructions: vec![call(2, MAKE, vec![])],
                    terminator: Terminator::Jump(BlockId(1)),
                },
                BasicBlock {
                    instructions: vec![call(3, TICK, vec![])],
                    terminator: Terminator::Branch {
                        condition: Operand::Value(VirtualRegId(1)),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    },
                },
                BasicBlock {
                    instructions: vec![],
                    terminator: Terminator::Return(Some(Operand::Value(VirtualRegId(2)))),
                },
            ],
            entry: BlockId(0),
        };
        let main = Signature {
            params: vec![IrType::Ref, IrType::Bool],
            result: Some(IrType::Ref),
        };
        let out = insert_gc_roots(program(function, main));
        let function = &out.functions[0];
        assert_eq!(function.root_slots, 3);
        let kinds: Vec<_> = function.blocks[1]
            .instructions
            .iter()
            .map(|instruction| format!("{:?}", instruction.kind))
            .collect();
        let t2 = VirtualRegId(2);
        assert_eq!(
            kinds,
            [
                format!(
                    "{:?}",
                    InstructionKind::RootStore {
                        slot: 2,
                        value: Operand::Value(t2)
                    }
                ),
                format!("{:?}", call(3, TICK, vec![]).kind),
                format!("{:?}", InstructionKind::RootLoad { dst: t2, slot: 2 }),
            ]
        );
        out.validate().unwrap();
    }
}
