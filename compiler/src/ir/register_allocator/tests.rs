use super::liveness::{compute_function_liveness, compute_liveness, get_def, get_used};
use crate::ir::{BasicBlock, FunctionIr, VirtualRegId};
use crate::ir::{
    BinaryOp, BlockId, CallTarget, Instruction, InstructionKind, IrType, Operand, SymbolId,
    Terminator,
};
use std::collections::HashSet;

fn block(instructions: Vec<InstructionKind>, terminator: Terminator) -> BasicBlock {
    BasicBlock {
        instructions: instructions
            .into_iter()
            .map(|kind| Instruction { kind, line: 1 })
            .collect(),
        terminator,
    }
}

#[test]
fn self_assignment_reads_the_incoming_value() {
    let register = VirtualRegId(0);
    let block = block(
        vec![InstructionKind::Binary {
            dst: register,
            op: BinaryOp::Add,
            lhs: Operand::Value(register),
            rhs: Operand::Int(1),
        }],
        Terminator::Return(Some(Operand::Value(register))),
    );
    assert_eq!(get_used(&block), HashSet::from([register]));
    assert_eq!(get_def(&block), HashSet::from([register]));
}

#[test]
fn reads_after_a_definition_do_not_require_an_incoming_value() {
    let block = block(
        vec![
            InstructionKind::Copy {
                dst: VirtualRegId(0),
                src: Operand::Int(10),
            },
            InstructionKind::Copy {
                dst: VirtualRegId(1),
                src: Operand::Value(VirtualRegId(0)),
            },
            InstructionKind::Copy {
                dst: VirtualRegId(0),
                src: Operand::Int(20),
            },
        ],
        Terminator::Return(Some(Operand::Value(VirtualRegId(1)))),
    );
    assert!(get_used(&block).is_empty());
    assert_eq!(
        get_def(&block),
        HashSet::from([VirtualRegId(0), VirtualRegId(1)])
    );
}

#[test]
fn terminators_and_indirect_calls_contribute_reads() {
    let call = InstructionKind::Call {
        dst: Some(VirtualRegId(0)),
        target: CallTarget::Indirect(Operand::Value(VirtualRegId(1))),
        args: vec![
            Operand::Value(VirtualRegId(0)),
            Operand::Value(VirtualRegId(0)),
        ],
    };
    let block = block(
        vec![call],
        Terminator::Abort {
            target: SymbolId(0),
            args: vec![
                Operand::Value(VirtualRegId(0)),
                Operand::Value(VirtualRegId(2)),
                Operand::Int(7),
            ],
        },
    );
    assert_eq!(
        get_used(&block),
        HashSet::from([VirtualRegId(0), VirtualRegId(1), VirtualRegId(2)])
    );
    assert_eq!(get_def(&block), HashSet::from([VirtualRegId(0)]));
}

#[test]
fn loop_liveness_propagates_across_the_back_edge() {
    let function = FunctionIr {
        symbol: SymbolId(0),
        params: vec![],
        register_types: vec![IrType::Bool],
        register_names: vec![None],
        root_slots: 0,
        entry: BlockId(0),
        blocks: vec![
            block(
                vec![InstructionKind::Copy {
                    dst: VirtualRegId(0),
                    src: Operand::Bool(true),
                }],
                Terminator::Jump(BlockId(1)),
            ),
            block(
                vec![],
                Terminator::Branch {
                    condition: Operand::Value(VirtualRegId(0)),
                    then_block: BlockId(1),
                    else_block: BlockId(2),
                },
            ),
            block(vec![], Terminator::Return(None)),
        ],
    };
    let result = compute_function_liveness(&function);
    assert_eq!(
        result.live_in,
        vec![
            HashSet::new(),
            HashSet::from([VirtualRegId(0)]),
            HashSet::new()
        ]
    );
    assert_eq!(
        result.live_out,
        vec![
            HashSet::from([VirtualRegId(0)]),
            HashSet::from([VirtualRegId(0)]),
            HashSet::new()
        ]
    );
}

#[test]
fn program_liveness_keeps_function_local_registers_separate() {
    use crate::ir::{ProgramIr, Signature, SignatureId, Symbol, SymbolKind};

    let parameter = FunctionIr {
        symbol: SymbolId(0),
        params: vec![VirtualRegId(0)],
        register_types: vec![IrType::Int32],
        register_names: vec![None],
        root_slots: 0,
        entry: BlockId(0),
        blocks: vec![block(
            vec![],
            Terminator::Return(Some(Operand::Value(VirtualRegId(0)))),
        )],
    };
    let constant = FunctionIr {
        symbol: SymbolId(2),
        params: vec![],
        register_types: vec![IrType::Int32],
        register_names: vec![None],
        root_slots: 0,
        entry: BlockId(0),
        blocks: vec![block(
            vec![InstructionKind::Copy {
                dst: VirtualRegId(0),
                src: Operand::Int(10),
            }],
            Terminator::Return(Some(Operand::Value(VirtualRegId(0)))),
        )],
    };
    let program = ProgramIr {
        symbols: vec![
            Symbol {
                name: "parameter".into(),
                kind: SymbolKind::Function(SignatureId(0)),
            },
            Symbol {
                name: "external".into(),
                kind: SymbolKind::Function(SignatureId(1)),
            },
            Symbol {
                name: "constant".into(),
                kind: SymbolKind::Function(SignatureId(1)),
            },
        ],
        signatures: vec![
            Signature {
                params: vec![IrType::Int32],
                result: Some(IrType::Int32),
            },
            Signature {
                params: vec![],
                result: Some(IrType::Int32),
            },
        ],
        // Function order differs from symbol order: results must use symbol IDs.
        functions: vec![constant, parameter],
        data: vec![],
        startup: None,
    }
    .verify()
    .unwrap();

    let result = compute_liveness(&program);
    assert_eq!(result.len(), 2);
    assert_eq!(
        result[&SymbolId(0)].live_in[0],
        HashSet::from([VirtualRegId(0)])
    );
    assert!(result[&SymbolId(2)].live_in[0].is_empty());
    assert!(result[&SymbolId(0)].live_out[0].is_empty());
    assert!(result[&SymbolId(2)].live_out[0].is_empty());
    assert!(!result.contains_key(&SymbolId(1)));
}

mod live_intervals {
    use super::super::live_intervals::{build_function_live_intervals, build_live_intervals};
    use super::super::liveness::{compute_function_liveness, compute_liveness};
    use crate::ir::{
        BasicBlock, BlockId, Instruction, InstructionKind, IrType, Operand, ProgramIr, Signature,
        SignatureId, Symbol, SymbolKind, Terminator,
    };
    use crate::ir::{FunctionIr, SymbolId, VirtualRegId};

    fn block(instructions: Vec<InstructionKind>, terminator: Terminator) -> BasicBlock {
        BasicBlock {
            instructions: instructions
                .into_iter()
                .map(|kind| Instruction { kind, line: 99 })
                .collect(),
            terminator,
        }
    }

    fn copy(destination: usize, source: Operand) -> InstructionKind {
        InstructionKind::Copy {
            dst: VirtualRegId(destination),
            src: source,
        }
    }

    fn function(blocks: Vec<BasicBlock>, register_count: usize) -> FunctionIr {
        FunctionIr {
            symbol: SymbolId(0),
            params: vec![],
            register_types: vec![IrType::Int32; register_count],
            register_names: vec![None; register_count],
            root_slots: 0,
            entry: BlockId(0),
            blocks,
        }
    }

    fn bounds(function: &FunctionIr) -> Vec<(usize, usize, usize)> {
        build_function_live_intervals(function, &compute_function_liveness(function))
            .iter()
            .map(|interval| (interval.register.0, interval.start, interval.end))
            .collect()
    }

    #[test]
    fn local_reads_and_dead_writes_extend_the_same_interval() {
        let function = function(
            vec![block(
                vec![
                    copy(0, Operand::Int(10)),
                    copy(1, Operand::Value(VirtualRegId(0))),
                    copy(0, Operand::Int(20)),
                ],
                Terminator::Return(Some(Operand::Value(VirtualRegId(1)))),
            )],
            3,
        );
        // Unused v2 has no interval. The dead write to v0 still needs a location.
        assert_eq!(bounds(&function), vec![(0, 0, 2), (1, 1, 3)]);
    }

    #[test]
    fn empty_forwarding_blocks_preserve_live_values() {
        let function = function(
            vec![
                block(
                    vec![copy(0, Operand::Int(10))],
                    Terminator::Jump(BlockId(1)),
                ),
                block(vec![], Terminator::Jump(BlockId(2))),
                block(
                    vec![],
                    Terminator::Return(Some(Operand::Value(VirtualRegId(0)))),
                ),
            ],
            1,
        );
        assert_eq!(bounds(&function), vec![(0, 0, 3)]);
    }

    #[test]
    fn back_edge_extends_an_interval_past_its_last_textual_read() {
        let mut function = function(
            vec![
                block(
                    vec![copy(0, Operand::Bool(true))],
                    Terminator::Jump(BlockId(1)),
                ),
                block(
                    vec![],
                    Terminator::Branch {
                        condition: Operand::Value(VirtualRegId(0)),
                        then_block: BlockId(2),
                        else_block: BlockId(3),
                    },
                ),
                block(vec![], Terminator::Jump(BlockId(1))),
                block(vec![], Terminator::Return(None)),
            ],
            1,
        );
        function.register_types[0] = IrType::Bool;
        // v0 must survive position 3 to supply the next iteration's branch at 2.
        assert_eq!(bounds(&function), vec![(0, 0, 3)]);
    }

    #[test]
    fn branch_arms_share_one_conservative_interval() {
        let mut function = function(
            vec![
                block(
                    vec![],
                    Terminator::Branch {
                        condition: Operand::Value(VirtualRegId(0)),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    },
                ),
                block(
                    vec![copy(1, Operand::Int(10))],
                    Terminator::Jump(BlockId(3)),
                ),
                block(
                    vec![copy(1, Operand::Int(20))],
                    Terminator::Jump(BlockId(3)),
                ),
                block(
                    vec![],
                    Terminator::Return(Some(Operand::Value(VirtualRegId(1)))),
                ),
            ],
            2,
        );
        function.params = vec![VirtualRegId(0)];
        function.register_types[0] = IrType::Bool;
        assert_eq!(bounds(&function), vec![(0, 0, 0), (1, 1, 5)]);
    }

    #[test]
    fn parameters_start_at_the_actual_entry_and_ties_sort_by_register_id() {
        let mut function = function(
            vec![
                block(vec![], Terminator::Return(None)),
                block(vec![], Terminator::Return(None)),
            ],
            2,
        );
        function.entry = BlockId(1);
        function.params = vec![VirtualRegId(1), VirtualRegId(0)];
        assert_eq!(bounds(&function), vec![(0, 1, 1), (1, 1, 1)]);
    }

    #[test]
    fn program_intervals_keep_function_local_registers_separate() {
        let mut parameter = function(
            vec![block(
                vec![],
                Terminator::Return(Some(Operand::Value(VirtualRegId(0)))),
            )],
            1,
        );
        parameter.params = vec![VirtualRegId(0)];
        let mut constant = function(
            vec![block(
                vec![copy(0, Operand::Int(10))],
                Terminator::Return(Some(Operand::Value(VirtualRegId(0)))),
            )],
            1,
        );
        constant.symbol = SymbolId(2);
        let program = ProgramIr {
            symbols: vec![
                Symbol {
                    name: "parameter".into(),
                    kind: SymbolKind::Function(SignatureId(0)),
                },
                Symbol {
                    name: "external".into(),
                    kind: SymbolKind::Function(SignatureId(1)),
                },
                Symbol {
                    name: "constant".into(),
                    kind: SymbolKind::Function(SignatureId(1)),
                },
            ],
            signatures: vec![
                Signature {
                    params: vec![IrType::Int32],
                    result: Some(IrType::Int32),
                },
                Signature {
                    params: vec![],
                    result: Some(IrType::Int32),
                },
            ],
            functions: vec![constant, parameter],
            data: vec![],
            startup: None,
        }
        .verify()
        .unwrap();
        let intervals = build_live_intervals(&program, &compute_liveness(&program));
        assert_eq!(intervals.len(), 2);
        assert!(!intervals.contains_key(&SymbolId(1)));
        let parameter_interval = &intervals[&SymbolId(0)][0];
        assert_eq!(
            (
                parameter_interval.register,
                parameter_interval.start,
                parameter_interval.end
            ),
            (VirtualRegId(0), 0, 0)
        );
        let constant_interval = &intervals[&SymbolId(2)][0];
        assert_eq!(
            (
                constant_interval.register,
                constant_interval.start,
                constant_interval.end
            ),
            (VirtualRegId(0), 0, 1)
        );
    }
}

mod allocation {
    use super::super::allocation::{allocate, allocate_function};
    use super::super::live_intervals::{build_live_intervals, LiveInterval};
    use super::super::{allocate_registers, liveness::compute_liveness};
    use super::super::{
        FunctionAllocation, PhysicalLocation, PhysicalRegId, RegisterAllocationError, SpillSlotId,
        TargetConstraints,
    };
    use super::block;
    use crate::ir::{
        BinaryOp, BlockId, CallTarget, CheckedIr, FunctionIr, InstructionKind, IrType, Operand,
        ProgramIr, Signature, SignatureId, Symbol, SymbolId, SymbolKind, Terminator, VirtualRegId,
    };
    use std::collections::{HashMap, HashSet};

    fn interval(register: usize, start: usize, end: usize) -> LiveInterval {
        LiveInterval {
            register: VirtualRegId(register),
            start,
            end,
        }
    }

    fn target(registers: &[PhysicalRegId]) -> TargetConstraints {
        let mut target = TargetConstraints::microsoft_x64();
        target.allocatable_registers = registers.to_vec();
        target
    }

    fn location(allocation: &FunctionAllocation, register: usize) -> PhysicalLocation {
        allocation.register_locations[&VirtualRegId(register)]
    }

    #[test]
    fn inclusive_endpoints_overlap_but_expired_registers_are_reused() {
        let intervals = [interval(0, 0, 1), interval(1, 1, 2), interval(2, 3, 3)];
        let allocation = allocate_function(&intervals, &target(&[PhysicalRegId::Rbx]), false);
        assert_eq!(
            location(&allocation, 0),
            PhysicalLocation::Register(PhysicalRegId::Rbx)
        );
        assert_eq!(
            location(&allocation, 1),
            PhysicalLocation::Spill(SpillSlotId(0))
        );
        assert_eq!(
            location(&allocation, 2),
            PhysicalLocation::Register(PhysicalRegId::Rbx)
        );
        assert_eq!(allocation.spill_slot_count, 1);
        assert_eq!(
            allocation.used_callee_saved_registers,
            vec![PhysicalRegId::Rbx]
        );
    }

    #[test]
    fn spilling_the_latest_active_interval_replaces_its_entire_assignment() {
        let intervals = [
            interval(0, 0, 9),
            interval(1, 1, 8),
            interval(2, 2, 3),
            interval(3, 4, 4),
        ];
        let allocation = allocate_function(
            &intervals,
            &target(&[PhysicalRegId::Rbx, PhysicalRegId::Rsi]),
            false,
        );
        assert_eq!(
            location(&allocation, 0),
            PhysicalLocation::Spill(SpillSlotId(0))
        );
        assert_eq!(
            location(&allocation, 1),
            PhysicalLocation::Register(PhysicalRegId::Rsi)
        );
        assert_eq!(
            location(&allocation, 2),
            PhysicalLocation::Register(PhysicalRegId::Rbx)
        );
        assert_eq!(
            location(&allocation, 3),
            PhysicalLocation::Register(PhysicalRegId::Rbx)
        );
        assert_eq!(allocation.spill_slot_count, 1);
    }

    #[test]
    fn later_and_equal_ending_current_intervals_are_spilled() {
        let intervals = [interval(0, 0, 3), interval(1, 1, 4), interval(2, 2, 3)];
        let allocation = allocate_function(&intervals, &target(&[PhysicalRegId::Rbx]), false);
        assert_eq!(
            location(&allocation, 0),
            PhysicalLocation::Register(PhysicalRegId::Rbx)
        );
        assert_eq!(
            location(&allocation, 1),
            PhysicalLocation::Spill(SpillSlotId(0))
        );
        assert_eq!(
            location(&allocation, 2),
            PhysicalLocation::Spill(SpillSlotId(1))
        );
    }

    #[test]
    fn zero_registers_produce_distinct_spill_slots() {
        let intervals = [interval(0, 0, 0), interval(1, 1, 1)];
        let allocation = allocate_function(&intervals, &target(&[]), false);
        assert_eq!(
            location(&allocation, 0),
            PhysicalLocation::Spill(SpillSlotId(0))
        );
        assert_eq!(
            location(&allocation, 1),
            PhysicalLocation::Spill(SpillSlotId(1))
        );
        assert_eq!(allocation.spill_slot_count, 2);
        assert!(allocation.used_callee_saved_registers.is_empty());
    }

    #[test]
    fn unsorted_intervals_and_start_ties_have_deterministic_assignments() {
        let intervals = [interval(2, 2, 2), interval(1, 0, 1), interval(0, 0, 1)];
        let target = target(&[PhysicalRegId::Rbx, PhysicalRegId::Rsi]);
        let allocation = allocate_function(&intervals, &target, false);
        assert_eq!(
            location(&allocation, 0),
            PhysicalLocation::Register(PhysicalRegId::Rbx)
        );
        assert_eq!(
            location(&allocation, 1),
            PhysicalLocation::Register(PhysicalRegId::Rsi)
        );
        assert_eq!(
            location(&allocation, 2),
            PhysicalLocation::Register(PhysicalRegId::Rbx)
        );
        assert_eq!(allocation, allocate_function(&intervals, &target, false));
        assert!(allocate_function(&[], &target, false)
            .register_locations
            .is_empty());
    }

    #[test]
    fn call_policy_excludes_caller_saved_registers() {
        let intervals = [interval(0, 0, 1), interval(1, 0, 1)];
        let target = target(&[PhysicalRegId::R8, PhysicalRegId::Rbx]);
        let allocation = allocate_function(&intervals, &target, true);
        assert_eq!(
            location(&allocation, 0),
            PhysicalLocation::Register(PhysicalRegId::Rbx)
        );
        assert_eq!(
            location(&allocation, 1),
            PhysicalLocation::Spill(SpillSlotId(0))
        );
        let allocation = allocate_function(&intervals, &self::target(&[PhysicalRegId::R8]), true);
        assert_eq!(allocation.spill_slot_count, 2);
        assert!(allocation.used_callee_saved_registers.is_empty());
        let leaf_allocation = allocate_function(&intervals, &target, false);
        assert_eq!(
            location(&leaf_allocation, 0),
            PhysicalLocation::Register(PhysicalRegId::R8)
        );
    }

    // Checked IR with two definitions and an external function; v0 is local to each definition.
    fn program(with_call: bool) -> CheckedIr {
        let first = FunctionIr {
            symbol: SymbolId(0),
            params: vec![],
            register_types: vec![IrType::Int32],
            register_names: vec![None],
            root_slots: 0,
            entry: BlockId(0),
            blocks: vec![block(
                vec![InstructionKind::Copy {
                    dst: VirtualRegId(0),
                    src: Operand::Int(1),
                }],
                Terminator::Return(Some(Operand::Value(VirtualRegId(0)))),
            )],
        };
        let mut instructions = vec![InstructionKind::Copy {
            dst: VirtualRegId(0),
            src: Operand::Int(10),
        }];
        if with_call {
            instructions.push(InstructionKind::Call {
                dst: Some(VirtualRegId(1)),
                target: CallTarget::Direct(SymbolId(2)),
                args: vec![],
            });
            instructions.push(InstructionKind::Binary {
                dst: VirtualRegId(2),
                op: BinaryOp::Add,
                lhs: Operand::Value(VirtualRegId(0)),
                rhs: Operand::Value(VirtualRegId(1)),
            });
        }
        let register_count = if with_call { 3 } else { 1 };
        let second = FunctionIr {
            symbol: SymbolId(1),
            params: vec![],
            register_types: vec![IrType::Int32; register_count],
            register_names: vec![None; register_count],
            root_slots: 0,
            entry: BlockId(0),
            blocks: vec![block(
                instructions,
                Terminator::Return(Some(Operand::Value(VirtualRegId(register_count - 1)))),
            )],
        };
        ProgramIr {
            symbols: ["first", "second", "external"]
                .into_iter()
                .map(|name| Symbol {
                    name: name.into(),
                    kind: SymbolKind::Function(SignatureId(0)),
                })
                .collect(),
            signatures: vec![Signature {
                params: vec![],
                result: Some(IrType::Int32),
            }],
            functions: vec![second, first],
            data: vec![],
            startup: None,
        }
        .verify()
        .unwrap()
    }

    #[test]
    fn public_pipeline_allocates_functions_separately_and_handles_calls() {
        let program = program(true);
        let target = target(&[PhysicalRegId::R8, PhysicalRegId::Rbx, PhysicalRegId::Rsi]);
        let allocation = allocate_registers(&program, &target).unwrap();
        assert_eq!(allocation.function_allocations.len(), 2);
        assert!(!allocation.function_allocations.contains_key(&SymbolId(2)));
        assert_eq!(
            location(&allocation.function_allocations[&SymbolId(0)], 0),
            PhysicalLocation::Register(PhysicalRegId::R8)
        );
        let calling_function = &allocation.function_allocations[&SymbolId(1)];
        assert_eq!(
            location(calling_function, 0),
            PhysicalLocation::Register(PhysicalRegId::Rbx)
        );
        assert_eq!(
            location(calling_function, 1),
            PhysicalLocation::Register(PhysicalRegId::Rsi)
        );
        assert_eq!(
            location(calling_function, 2),
            PhysicalLocation::Spill(SpillSlotId(0))
        );
        assert_eq!(
            calling_function.used_callee_saved_registers,
            vec![PhysicalRegId::Rbx, PhysicalRegId::Rsi]
        );
    }

    #[test]
    fn invalid_target_constraints_return_errors() {
        let program = program(false);
        let mut invalid_targets = Vec::new();
        invalid_targets.push(target(&[PhysicalRegId::Rbx, PhysicalRegId::Rbx]));
        invalid_targets.push(target(&[PhysicalRegId::Rsp]));
        let mut invalid = target(&[PhysicalRegId::Rbx]);
        invalid
            .reserved_registers
            .retain(|register| *register != PhysicalRegId::Rax);
        invalid_targets.push(invalid);
        let mut invalid = target(&[PhysicalRegId::Rbx]);
        invalid.caller_saved_registers.push(PhysicalRegId::Rbx);
        invalid_targets.push(invalid);
        let mut invalid = target(&[PhysicalRegId::Rbx]);
        invalid.argument_registers[0] = PhysicalRegId::Rdi;
        invalid_targets.push(invalid);
        let mut invalid = target(&[PhysicalRegId::Rbx]);
        invalid.stack_alignment_bytes = 8;
        invalid_targets.push(invalid);
        for target in invalid_targets {
            assert!(matches!(
                allocate_registers(&program, &target),
                Err(RegisterAllocationError::InvalidTarget(_))
            ));
        }
        assert!(allocate_registers(&program, &target(&[])).is_ok());
    }

    #[test]
    fn missing_stale_and_malformed_intervals_return_errors() {
        let program = program(false);
        let target = target(&[PhysicalRegId::Rbx]);
        assert_eq!(
            allocate(&program, &HashMap::new(), &target).unwrap_err(),
            RegisterAllocationError::MissingIntervals(SymbolId(1))
        );
        for invalid_intervals in [
            vec![],
            vec![interval(0, 0, 0)], // Last use at position 1 is missing.
            vec![interval(0, 1, 0)],
            vec![interval(0, 0, 2)],
            vec![interval(1, 0, 1)],
            vec![interval(0, 0, 1), interval(0, 0, 1)],
        ] {
            let mut intervals = build_live_intervals(&program, &compute_liveness(&program));
            intervals.insert(SymbolId(1), invalid_intervals);
            assert!(matches!(
                allocate(&program, &intervals, &target),
                Err(RegisterAllocationError::InvalidIntervals {
                    function: SymbolId(1),
                    ..
                })
            ));
        }
    }

    #[test]
    fn exhaustive_small_interval_sets_preserve_allocation_invariants() {
        let ranges = [(0, 0), (0, 1), (0, 2), (1, 1), (1, 2), (2, 2)];
        let available = [PhysicalRegId::Rbx, PhysicalRegId::Rsi, PhysicalRegId::R8];
        // Enumerate all 6^4 combinations of four intervals, with 0..=3 registers.
        for combination in 0..ranges.len().pow(4) {
            let mut remaining = combination;
            let mut intervals = Vec::new();
            for register in 0..4 {
                let (start, end) = ranges[remaining % ranges.len()];
                intervals.push(interval(register, start, end));
                remaining /= ranges.len();
            }
            for register_count in 0..=available.len() {
                let target = target(&available[..register_count]);
                for has_calls in [false, true] {
                    let allocation = allocate_function(&intervals, &target, has_calls);
                    assert_eq!(allocation.register_locations.len(), intervals.len());
                    let mut spill_slots = HashSet::new();
                    for (index, interval) in intervals.iter().enumerate() {
                        match location(&allocation, interval.register.0) {
                            PhysicalLocation::Register(register) => {
                                assert!(target.allocatable_registers.contains(&register));
                                assert!(!target.reserved_registers.contains(&register));
                                assert!(
                                    !has_calls || target.callee_saved_registers.contains(&register)
                                );
                                for other in &intervals[index + 1..] {
                                    let overlap =
                                        interval.start <= other.end && other.start <= interval.end;
                                    if overlap {
                                        assert_ne!(
                                            location(&allocation, other.register.0),
                                            PhysicalLocation::Register(register)
                                        );
                                    }
                                }
                            }
                            PhysicalLocation::Spill(slot) => {
                                assert!(slot.0 < allocation.spill_slot_count);
                                assert!(spill_slots.insert(slot.0));
                            }
                        }
                    }
                    assert_eq!(spill_slots.len(), allocation.spill_slot_count);
                    for &register in &target.callee_saved_registers {
                        let used = allocation
                            .register_locations
                            .values()
                            .any(|location| *location == PhysicalLocation::Register(register));
                        assert_eq!(
                            allocation.used_callee_saved_registers.contains(&register),
                            used
                        );
                    }
                }
            }
        }
    }
}
