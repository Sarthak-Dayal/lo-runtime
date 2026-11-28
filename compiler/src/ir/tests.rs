use super::*;

fn copy(value: i32) -> Instruction {
    Instruction {
        kind: InstructionKind::Copy {
            dst: VirtualRegId(0),
            src: Operand::Int(value),
        },
        line: 1,
    }
}
fn example() -> ProgramIr {
    let mut program = ProgramIr {
        data: vec![],
        startup: None,
        symbols: vec![Symbol {
            name: "example".into(),
            kind: SymbolKind::Function(SignatureId(0)),
        }],
        signatures: vec![Signature {
            params: vec![],
            result: Some(IrType::Int32),
        }],
        functions: vec![FunctionIr {
            symbol: SymbolId(0),
            params: vec![],
            register_types: vec![],
            register_names: vec![],
            root_slots: 0,
            entry: BlockId(0),
            blocks: vec![
                BasicBlock {
                    instructions: vec![],
                    terminator: Terminator::Branch {
                        condition: Operand::Bool(true),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    },
                    terminator_line: None,
                },
                BasicBlock {
                    instructions: vec![copy(1)],
                    terminator: Terminator::Jump(BlockId(3)),
                    terminator_line: None,
                },
                BasicBlock {
                    instructions: vec![copy(2)],
                    terminator: Terminator::Jump(BlockId(3)),
                    terminator_line: None,
                },
                BasicBlock {
                    instructions: vec![],
                    terminator: Terminator::Return(Some(Operand::Value(VirtualRegId(0)))),
                    terminator_line: None,
                },
            ],
        }],
    };
    program.functions[0].new_register(IrType::Int32, Some("result".into()));
    program
}

#[test]
fn new_register_keeps_types_and_names_aligned() {
    let mut program = example();
    let function = &mut program.functions[0];
    let object = function.new_register(IrType::Ref, Some("object".into()));
    let pointer = function.new_register(IrType::Ptr, None);
    assert_eq!(object, VirtualRegId(1));
    assert_eq!(pointer, VirtualRegId(2));
    assert_eq!(
        function.register_types,
        vec![IrType::Int32, IrType::Ref, IrType::Ptr]
    );
    assert_eq!(
        function.register_names,
        vec![Some("result".into()), Some("object".into()), None]
    );
    program.validate().unwrap();
}

#[test]
fn use_def_helpers_preserve_order_and_duplicate_reads() {
    let register = VirtualRegId(0);
    let binary = InstructionKind::Binary {
        dst: register,
        op: BinaryOp::Add,
        lhs: Operand::Value(register),
        rhs: Operand::Value(register),
    };
    assert_eq!(binary.destination(), Some(register));
    assert_eq!(binary.used_registers(), vec![register, register]);

    let pointer = VirtualRegId(1);
    let call = InstructionKind::Call {
        dst: Some(VirtualRegId(2)),
        target: CallTarget::Indirect(Operand::Value(pointer)),
        args: vec![
            Operand::Value(register),
            Operand::Int(7),
            Operand::Value(register),
        ],
    };
    assert_eq!(call.used_registers(), vec![register, register, pointer]);
    let abort = Terminator::Abort {
        target: SymbolId(0),
        args: vec![
            Operand::Value(register),
            Operand::Null,
            Operand::Value(register),
        ],
    };
    assert_eq!(abort.used_registers(), vec![register, register]);
}

#[test]
fn dump_preserves_external_data_and_function_layout() {
    let mut program = example();
    let receiver = program.functions[0].new_register(IrType::Ref, Some("receiver".into()));
    let condition = program.functions[0].new_register(IrType::Bool, None);
    program.functions[0].params = vec![receiver, condition];
    program.signatures[0].params = vec![IrType::Ref, IrType::Bool];
    program.functions[0].root_slots = 2;
    program.startup = Some(SymbolId(0));
    let runtime_signature = program.intern_signature(Signature {
        params: vec![IrType::Int32, IrType::Ptr],
        result: None,
    });
    program.symbols.extend([
        Symbol {
            name: "runtime".into(),
            kind: SymbolKind::Function(runtime_signature),
        },
        Symbol {
            name: "external_data".into(),
            kind: SymbolKind::Data,
        },
        Symbol {
            name: "empty_ref".into(),
            kind: SymbolKind::StaticRef,
        },
        Symbol {
            name: "constants".into(),
            kind: SymbolKind::Data,
        },
        Symbol {
            name: "globals".into(),
            kind: SymbolKind::Data,
        },
    ]);
    program.data = vec![
        DataDef {
            symbol: SymbolId(4),
            section: Section::ReadOnly,
            align: 4,
            items: vec![
                DataItem::Bytes(vec![0, 15, 255]),
                DataItem::Bytes(vec![]),
                DataItem::Addr(SymbolId(0)),
            ],
        },
        DataDef {
            symbol: SymbolId(5),
            section: Section::Writable,
            align: 8,
            items: vec![DataItem::U32(42), DataItem::Zero(8)],
        },
    ];
    program.validate().unwrap();
    assert_eq!(
        program.dump(),
        concat!(
            "extern func @runtime(Int32, Ptr) -> Void\n",
            "extern data @external_data\n",
            "extern ref @empty_ref\n",
            ".rodata @constants align 4 {\n",
            "  bytes \"\\x00\\x0f\\xff\"\n",
            "  bytes \"\"\n",
            "  addr @example\n",
            "}\n",
            ".data @globals align 8 {\n",
            "  u32 42\n",
            "  zero 8\n",
            "}\n",
            "\nfunc @example(receiver:Ref, t2:Bool) -> Int32 entry .L0 roots 2 startup {\n",
            ".L0:\n",
            "  cbr true, .L1, .L2\n",
            ".L1:\n",
            "  result:Int32 = 1 ; line 1\n",
            "  br .L3\n",
            ".L2:\n",
            "  result:Int32 = 2 ; line 1\n",
            "  br .L3\n",
            ".L3:\n",
            "  ret result\n",
            "}\n",
        )
    );
}

#[test]
fn dump_preserves_duplicate_names_and_malformed_register_fallbacks() {
    let mut program = example();
    program.functions[0].register_names[0] = Some("shared".into());
    program.functions[0].new_register(IrType::Int32, Some("shared".into()));
    program.functions[0].new_register(IrType::Int32, Some("t7".into()));
    program.functions[0].new_register(IrType::Int32, Some("t+8".into()));
    program.functions[0].new_register(IrType::Int32, Some("ordinary".into()));
    program.functions[0].params = (0..5).map(VirtualRegId).collect();
    assert!(program.dump().contains(
        "func @example(shared.v0:Int32, shared.v1:Int32, t7.v2:Int32, t+8.v3:Int32, ordinary:Int32)"
    ));

    // Names beyond the type table still affect duplicate-name detection.
    program.functions[0].register_types.truncate(1);
    assert!(program
        .dump()
        .contains("shared.v0:Int32, shared.v1:<invalid type>"));
    program.functions[0].register_names.clear();
    program.functions[0].params = vec![VirtualRegId(99)];
    assert!(program.dump().contains("func @example(t99:<invalid type>)"));
    assert!(program.dump().contains("t0:Int32 = 1"));
}

#[test]
fn dump_preserves_invalid_function_and_signature_markers() {
    let mut program = example();
    program.symbols[0].kind = SymbolKind::Function(SignatureId(99));
    assert!(program
        .dump()
        .contains("func @example() -> <invalid signature>"));
    program.functions[0].symbol = SymbolId(99);
    let dump = program.dump();
    assert!(dump.contains("extern func @example<invalid signature:99>"));
    assert!(dump.contains("func @<invalid:99>() -> <invalid function>"));

    program.functions[0].symbol = SymbolId(0);
    program.symbols[0].kind = SymbolKind::Data;
    assert!(program
        .dump()
        .contains("func @example() -> <invalid function>"));
    program.symbols[0].kind = SymbolKind::Function(SignatureId(0));
    program.signatures[0] = Signature {
        params: vec![IrType::CodePtr(SignatureId(0))],
        result: Some(IrType::CodePtr(SignatureId(0))),
    };
    program.functions[0].new_register(IrType::CodePtr(SignatureId(0)), None);
    program.functions[0].params = vec![VirtualRegId(1)];
    assert!(program
        .dump()
        .contains("t1:CodePtr(<invalid nested CodePtr>) -> <invalid nested CodePtr>"));
}

#[test]
fn mutable_merge_and_readable_dump() {
    let p = example();
    p.validate().unwrap();
    assert_eq!(p.dump(), "\nfunc @example() -> Int32 entry .L0 roots 0 {\n.L0:\n  cbr true, .L1, .L2\n.L1:\n  result:Int32 = 1 ; line 1\n  br .L3\n.L2:\n  result:Int32 = 2 ; line 1\n  br .L3\n.L3:\n  ret result\n}\n");
}

#[test]
fn rejects_uninitialized_merge_and_loop() {
    let mut p = example();
    p.functions[0].blocks[2].instructions.clear();
    assert!(p.validate().unwrap_err().contains("used before assignment"));
    // A definition on a back edge cannot initialize the first iteration.
    p.functions[0].blocks[2].terminator = Terminator::Jump(BlockId(0));
    p.functions[0].blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Copy {
            dst: VirtualRegId(0),
            src: Operand::Value(VirtualRegId(0)),
        },
        line: 1,
    });
    assert!(p.validate().unwrap_err().contains("used before assignment"));
}

#[test]
fn accepts_initialized_loop() {
    let mut p = example();
    p.functions[0].blocks[0].instructions.push(copy(0));
    p.functions[0].blocks[1].instructions = vec![Instruction {
        kind: InstructionKind::Binary {
            dst: VirtualRegId(0),
            op: BinaryOp::Add,
            lhs: Operand::Value(VirtualRegId(0)),
            rhs: Operand::Int(1),
        },
        line: 2,
    }];
    p.functions[0].blocks[1].terminator = Terminator::Branch {
        condition: Operand::Bool(false),
        then_block: BlockId(1),
        else_block: BlockId(3),
    };
    liveness::remove_dead_copies(&mut p.functions[0], DebugMode::Off);
    assert_eq!(p.functions[0].blocks[0].instructions.len(), 1);
    p.validate().unwrap();
}

#[test]
fn assignment_analysis_handles_reverse_block_order_and_unreachable_predecessors() {
    let mut program = example();
    program.functions[0].entry = BlockId(3);
    program.functions[0].blocks = vec![
        BasicBlock {
            instructions: vec![],
            terminator: Terminator::Return(Some(Operand::Value(VirtualRegId(0)))),
            terminator_line: None,
        },
        // This unreachable path does not initialize v0 and must not affect b0.
        BasicBlock {
            instructions: vec![],
            terminator: Terminator::Jump(BlockId(0)),
            terminator_line: None,
        },
        BasicBlock {
            instructions: vec![copy(7)],
            terminator: Terminator::Jump(BlockId(0)),
            terminator_line: None,
        },
        BasicBlock {
            instructions: vec![],
            terminator: Terminator::Jump(BlockId(2)),
            terminator_line: None,
        },
    ];
    program.validate().unwrap();

    // Assignment information must propagate through multiple iterations even
    // when blocks are visited in the reverse of execution order.
    program.functions[0].blocks[2].instructions.clear();
    assert_eq!(
        program.validate().unwrap_err(),
        "example: b0: v0 used before assignment"
    );

    // Unreachable blocks still require valid IDs before assignment analysis.
    program.functions[0].blocks[1].terminator = Terminator::Jump(BlockId(99));
    assert_eq!(
        program.validate().unwrap_err(),
        "example: b1: invalid target b99"
    );
}

#[test]
fn invalid_entry_parameter_and_destination_ids_return_errors() {
    let mut program = example();
    program.functions[0].blocks.clear();
    assert_eq!(
        program.validate().unwrap_err(),
        "example: invalid entry block"
    );

    let mut program = example();
    program.signatures[0].params = vec![IrType::Int32];
    program.functions[0].params = vec![VirtualRegId(99)];
    assert_eq!(
        program.validate().unwrap_err(),
        "example: unknown value v99"
    );

    let mut program = example();
    program.functions[0].blocks[0]
        .instructions
        .push(Instruction {
            kind: InstructionKind::Copy {
                dst: VirtualRegId(99),
                src: Operand::Int(1),
            },
            line: 42,
        });
    assert_eq!(
        program.validate().unwrap_err(),
        "example: b0: line 42: unknown value v99"
    );
}

#[test]
fn verifier_errors_include_locations_and_expected_types() {
    let mut program = example();
    if let Terminator::Branch { condition, .. } = &mut program.functions[0].blocks[0].terminator {
        *condition = Operand::Int(1);
    }
    assert_eq!(
        program.validate().unwrap_err(),
        "example: b0: expected Bool, got Int32"
    );

    program.functions[0].blocks[0].terminator = Terminator::Return(None);
    assert_eq!(
        program.validate().unwrap_err(),
        "example: b0: return type mismatch: expected Some(Int32), got None"
    );

    let mut program = example();
    program.functions[0].blocks[0]
        .instructions
        .push(Instruction {
            kind: InstructionKind::Copy {
                dst: VirtualRegId(0),
                src: Operand::Value(VirtualRegId(0)),
            },
            line: 42,
        });
    assert_eq!(
        program.validate().unwrap_err(),
        "example: b0: line 42: v0 used before assignment"
    );
}

#[test]
fn call_and_abort_errors_identify_argument_types_and_counts() {
    let mut program = example();
    program.signatures.push(Signature {
        params: vec![IrType::Ref, IrType::Int32],
        result: None,
    });
    program.symbols.push(Symbol {
        name: "helper".into(),
        kind: SymbolKind::Function(SignatureId(1)),
    });
    program.functions[0].blocks[0]
        .instructions
        .push(Instruction {
            kind: InstructionKind::Call {
                dst: None,
                target: CallTarget::Direct(SymbolId(1)),
                args: vec![Operand::Null, Operand::Bool(true)],
            },
            line: 42,
        });
    assert_eq!(
        program.validate().unwrap_err(),
        "example: b0: line 42: call argument 2: expected Int32, got Bool"
    );
    if let InstructionKind::Call { args, .. } =
        &mut program.functions[0].blocks[0].instructions[0].kind
    {
        args.pop();
    }
    assert_eq!(
        program.validate().unwrap_err(),
        "example: b0: line 42: call argument count mismatch: expected 2, got 1"
    );

    program.functions[0].blocks[0].instructions.clear();
    program.functions[0].blocks[0].terminator = Terminator::Abort {
        target: SymbolId(1),
        args: vec![Operand::Null, Operand::Bool(true)],
    };
    assert_eq!(
        program.validate().unwrap_err(),
        "example: b0: abort argument 2: expected Int32, got Bool"
    );
    if let Terminator::Abort { args, .. } = &mut program.functions[0].blocks[0].terminator {
        args.pop();
    }
    assert_eq!(
        program.validate().unwrap_err(),
        "example: b0: abort argument count mismatch: expected 2, got 1"
    );
}

#[test]
fn rejects_invalid_targets_ids_and_types() {
    let mut p = example();
    p.functions[0].blocks[0].terminator = Terminator::Jump(BlockId(99));
    assert!(p.validate().unwrap_err().contains("invalid target"));
    p.functions[0].blocks[0].terminator =
        Terminator::Return(Some(Operand::Value(VirtualRegId(99))));
    assert!(p.validate().unwrap_err().contains("unknown value"));
    p.functions[0].blocks[0].terminator = Terminator::Return(Some(Operand::Bool(true)));
    assert!(p.validate().unwrap_err().contains("return type"));
}

#[test]
fn indirect_calls_check_signature_and_initialization() {
    let mut p = example();
    p.symbols.push(Symbol {
        name: "callee".into(),
        kind: SymbolKind::Function(SignatureId(1)),
    });
    p.signatures.push(Signature {
        params: vec![IrType::Ref],
        result: Some(IrType::Int32),
    });
    p.functions[0].new_register(IrType::CodePtr(SignatureId(1)), None);
    p.functions[0].blocks[0].instructions = vec![
        Instruction {
            kind: InstructionKind::Copy {
                dst: VirtualRegId(1),
                src: Operand::Symbol(SymbolId(1)),
            },
            line: 1,
        },
        Instruction {
            kind: InstructionKind::Call {
                dst: Some(VirtualRegId(0)),
                target: CallTarget::Indirect(Operand::Value(VirtualRegId(1))),

                args: vec![Operand::Null],
            },
            line: 2,
        },
    ];
    p.validate().unwrap();
    assert!(p.dump().contains("call_indirect t1(null)"));
    if let InstructionKind::Call { args, .. } = &mut p.functions[0].blocks[0].instructions[1].kind {
        args.clear();
    }
    assert!(p.validate().unwrap_err().contains("argument count"));
}

#[test]
fn reference_store_requires_barrier_call() {
    let mut p = example();
    p.functions[0].blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Store {
            base: Operand::Null,
            offset: 0,
            value: Operand::Null,
            ty: IrType::Ref,
        },
        line: 1,
    });
    assert!(p.validate().unwrap_err().contains("write-barrier call"));
    p.symbols.push(Symbol {
        name: "lo_gc_write_barrier".into(),
        kind: SymbolKind::Function(SignatureId(1)),
    });
    p.signatures.push(Signature {
        params: vec![IrType::Ref, IrType::Int32, IrType::Ref],
        result: None,
    });
    p.functions[0].blocks[0].instructions[0].kind = InstructionKind::Call {
        dst: None,
        target: CallTarget::Direct(SymbolId(1)),

        args: vec![Operand::Null, Operand::Int(0), Operand::Null],
    };
    p.validate().unwrap();
}

#[test]
fn static_objects_are_references_not_raw_data() {
    let mut p = example();
    p.symbols.push(Symbol {
        name: "LO_EMPTY_STRING".into(),
        kind: SymbolKind::StaticRef,
    });
    p.functions[0].new_register(IrType::Ref, None);
    p.functions[0].params.push(VirtualRegId(1));
    p.signatures[0].params.push(IrType::Ref);
    p.functions[0].blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Store {
            base: Operand::Value(VirtualRegId(1)),
            offset: 0,
            value: Operand::Symbol(SymbolId(1)),
            ty: IrType::Ref,
        },
        line: 1,
    });
    p.symbols.push(Symbol {
        name: "consume_string".into(),
        kind: SymbolKind::Function(SignatureId(1)),
    });
    p.signatures.push(Signature {
        params: vec![IrType::Ref],
        result: None,
    });
    p.functions[0].blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Call {
            dst: None,
            target: CallTarget::Direct(SymbolId(2)),

            args: vec![Operand::Symbol(SymbolId(1))],
        },
        line: 1,
    });
    p.validate().unwrap();
    p.symbols[1].kind = SymbolKind::Data;
    assert!(p.validate().is_err());
    p.functions[0].blocks[0].instructions.remove(0);
    assert!(p.validate().unwrap_err().contains("expected Ref, got Ptr"));
}

#[test]
fn abort_is_a_checked_terminal_call() {
    let mut p = example();
    p.symbols.push(Symbol {
        name: "lo_abort_null_receiver".into(),
        kind: SymbolKind::Function(SignatureId(1)),
    });
    p.symbols.push(Symbol {
        name: "method_name".into(),
        kind: SymbolKind::Data,
    });
    p.signatures.push(Signature {
        params: vec![IrType::Ptr, IrType::Int32],
        result: None,
    });
    p.functions[0].blocks[0].terminator = Terminator::Abort {
        target: SymbolId(1),
        args: vec![Operand::Symbol(SymbolId(2)), Operand::Int(4)],
    };
    p.validate().unwrap();
    assert!(p.functions[0].blocks[0].terminator.successors().is_empty());
    assert!(p
        .dump()
        .contains("abort @lo_abort_null_receiver(@method_name, 4)"));
    if let Terminator::Abort { args, .. } = &mut p.functions[0].blocks[0].terminator {
        args[1] = Operand::Value(VirtualRegId(0));
    }
    assert!(p.validate().unwrap_err().contains("used before assignment"));
    p.functions[0].blocks[0].instructions.push(copy(4));
    p.validate().unwrap();
    p.signatures[1].result = Some(IrType::Int32);
    assert!(p.validate().unwrap_err().contains("must not return"));
    p.signatures[1].result = None;
    if let Terminator::Abort { args, .. } = &mut p.functions[0].blocks[0].terminator {
        args.pop();
    }
    assert!(p.validate().unwrap_err().contains("argument count"));
}

#[test]
fn checked_ir_must_be_unwrapped_before_editing() {
    let checked = example().verify().ok().unwrap();
    assert!(checked.program().dump().contains("func @example"));
    let mut program = checked.into_program();
    program.functions[0].entry = BlockId(99);
    assert!(program.verify().err().unwrap().contains("invalid entry"));
}

fn indirect_example() -> ProgramIr {
    let mut p = example();
    p.functions[0].new_register(IrType::CodePtr(SignatureId(0)), None);
    p.functions[0].blocks[0].instructions = vec![
        Instruction {
            kind: InstructionKind::Copy {
                dst: VirtualRegId(1),
                src: Operand::Symbol(SymbolId(0)),
            },
            line: 1,
        },
        Instruction {
            kind: InstructionKind::Call {
                dst: Some(VirtualRegId(0)),
                target: CallTarget::Indirect(Operand::Value(VirtualRegId(1))),
                args: vec![],
            },
            line: 2,
        },
    ];
    p
}

#[test]
fn signatures_are_interned_and_calls_use_pointer_types() {
    let mut p = indirect_example();
    assert_eq!(p.intern_signature(p.signatures[0].clone()), SignatureId(0));
    assert_eq!(p.signatures.len(), 1);
    p.validate().unwrap();
    if let InstructionKind::Call { args, .. } = &mut p.functions[0].blocks[0].instructions[1].kind {
        args.push(Operand::Null);
    }
    assert!(p.validate().unwrap_err().contains("argument count"));
    let other = p.intern_signature(Signature {
        params: vec![IrType::Ref],
        result: Some(IrType::Int32),
    });
    p.functions[0].register_types[1] = IrType::CodePtr(other);
    assert!(p.validate().unwrap_err().contains("line 1"));
}

#[test]
fn vtable_data_and_load_preserve_declared_signature() {
    let mut p = indirect_example();
    p.symbols.push(Symbol {
        name: "example_vtable".into(),
        kind: SymbolKind::Data,
    });
    p.data.push(DataDef {
        symbol: SymbolId(1),
        section: Section::ReadOnly,
        align: 8,
        items: vec![DataItem::Addr(SymbolId(0))],
    });
    p.functions[0].blocks[0].instructions[0].kind = InstructionKind::Load {
        dst: VirtualRegId(1),
        base: Operand::Symbol(SymbolId(1)),
        offset: 0,
    };
    p.validate().unwrap();
    let dump = p.dump();
    assert!(dump.contains(".rodata @example_vtable align 8"));
    assert!(dump.contains("addr @example"));
    assert!(dump.contains("t1:CodePtr() -> Int32 = load [@example_vtable + 0]"));
    assert!(dump.contains("result:Int32 = call_indirect t1()"));
    let other = p.intern_signature(Signature {
        params: vec![],
        result: Some(IrType::Bool),
    });
    p.functions[0].register_types[1] = IrType::CodePtr(other);
    assert!(p.validate().unwrap_err().contains("result type"));
}

#[test]
fn invalid_and_nested_signatures_are_rejected() {
    let mut p = indirect_example();
    p.functions[0].register_types[1] = IrType::CodePtr(SignatureId(99));
    assert!(p.validate().unwrap_err().contains("unknown signature"));
    p.functions[0].register_types[1] = IrType::CodePtr(SignatureId(0));
    p.signatures.push(p.signatures[0].clone());
    assert!(p.validate().unwrap_err().contains("duplicate signature"));
    p.signatures[1].result = Some(IrType::CodePtr(SignatureId(1)));
    assert!(p.validate().unwrap_err().contains("cannot take or return"));
    assert!(p.dump().contains("func @example"));
}

#[test]
fn root_instructions_check_slots_and_types() {
    let mut p = example();
    p.functions[0].root_slots = 1;
    let object = p.functions[0].new_register(IrType::Ref, Some("object".into()));
    p.functions[0].blocks[0].instructions = vec![
        Instruction {
            kind: InstructionKind::RootStore {
                slot: 0,
                value: Operand::Null,
            },
            line: 1,
        },
        Instruction {
            kind: InstructionKind::RootLoad {
                dst: object,
                slot: 0,
            },
            line: 2,
        },
    ];
    p.validate().unwrap();
    assert!(p.dump().contains("object:Ref = root_load root0"));
    p.functions[0].root_slots = 0;
    assert!(p.validate().unwrap_err().contains("root slot"));
    p.functions[0].root_slots = 1;
    p.functions[0].blocks[0].instructions[0].kind = InstructionKind::RootStore {
        slot: 0,
        value: Operand::Int(1),
    };
    assert!(p.validate().unwrap_err().contains("expected Ref"));
    p.functions[0].blocks[0].instructions.remove(0);
    p.functions[0].register_types[object.0] = IrType::Int32;
    assert!(p.validate().unwrap_err().contains("expected Ref"));
}

#[test]
fn data_definitions_and_relocations_are_checked() {
    let mut p = example();
    p.symbols.push(Symbol {
        name: "globals".into(),
        kind: SymbolKind::Data,
    });
    p.data.push(DataDef {
        symbol: SymbolId(1),
        section: Section::Writable,
        align: 8,
        items: vec![
            DataItem::U32(1),
            DataItem::Bytes(vec![0x61, 0]),
            DataItem::Zero(4),
            DataItem::Addr(SymbolId(0)),
        ],
    });
    p.validate().unwrap();
    let dump = p.dump();
    assert!(dump.contains(".data @globals align 8"));
    assert!(dump.contains("u32 1\n  bytes \"a\\x00\"\n  zero 4\n  addr @example"));
    p.data[0].align = 3;
    assert!(p.validate().unwrap_err().contains("alignment"));
    p.data[0].align = 8;
    p.data[0].items.push(DataItem::Addr(SymbolId(99)));
    assert!(p.validate().unwrap_err().contains("unknown symbol"));
    p.data[0].items.pop();
    p.data.push(DataDef {
        symbol: SymbolId(1),
        section: Section::Writable,
        align: 8,
        items: vec![],
    });
    assert!(p.validate().unwrap_err().contains("duplicate data"));
}

#[test]
fn division_literals_require_special_case_lowering() {
    for op in [BinaryOp::Div, BinaryOp::Mod] {
        for divisor in [0, -1, 2] {
            let mut p = example();
            p.functions[0].blocks[0].instructions.push(Instruction {
                kind: InstructionKind::Binary {
                    dst: VirtualRegId(0),
                    op,
                    lhs: Operand::Int(i32::MIN),
                    rhs: Operand::Int(divisor),
                },
                line: 1,
            });
            assert_eq!(p.validate().is_ok(), divisor == 2);
        }
    }
}

#[test]
fn public_use_def_helpers_include_calls_and_terminators() {
    let call = InstructionKind::Call {
        dst: Some(VirtualRegId(0)),
        target: CallTarget::Indirect(Operand::Value(VirtualRegId(1))),
        args: vec![Operand::Value(VirtualRegId(2))],
    };
    assert_eq!(call.destination(), Some(VirtualRegId(0)));
    assert_eq!(call.operands().len(), 2);
    assert!(matches!(
        call.operands()[1],
        Operand::Value(VirtualRegId(1))
    ));
    assert_eq!(
        call.used_registers(),
        vec![VirtualRegId(2), VirtualRegId(1)]
    );
    let abort = Terminator::Abort {
        target: SymbolId(0),
        args: vec![Operand::Value(VirtualRegId(2))],
    };
    assert!(matches!(
        abort.operands()[0],
        Operand::Value(VirtualRegId(2))
    ));
    assert_eq!(abort.used_registers(), vec![VirtualRegId(2)]);
    assert!(InstructionKind::Copy {
        dst: VirtualRegId(0),
        src: Operand::Int(42),
    }
    .used_registers()
    .is_empty());
    assert!(Terminator::Return(Some(Operand::Int(42)))
        .used_registers()
        .is_empty());
    assert!(Terminator::Jump(BlockId(0)).operands().is_empty());
    assert!(matches!(
        Terminator::Return(Some(Operand::Value(VirtualRegId(0)))).operands()[0],
        Operand::Value(VirtualRegId(0))
    ));
    assert!(InstructionKind::RootLoad {
        dst: VirtualRegId(0),
        slot: 0
    }
    .operands()
    .is_empty());
}

#[test]
fn dump_uses_named_typed_calls_and_handles_invalid_ids() {
    let mut p = example();
    let signature = p.intern_signature(Signature {
        params: vec![IrType::Ptr],
        result: Some(IrType::Ref),
    });
    p.symbols.push(Symbol {
        name: "lo_alloc".into(),
        kind: SymbolKind::Function(signature),
    });
    p.symbols.push(Symbol {
        name: "lo_class_6_circle".into(),
        kind: SymbolKind::Data,
    });
    p.functions[0].new_register(IrType::Ref, None);
    p.functions[0].blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Call {
            dst: Some(VirtualRegId(1)),
            target: CallTarget::Direct(SymbolId(1)),
            args: vec![Operand::Symbol(SymbolId(2))],
        },
        line: 1,
    });
    p.validate().unwrap();
    let dump = p.dump();
    assert!(dump.contains("t1:Ref = call @lo_alloc(@lo_class_6_circle)"));
    assert!(!dump.contains("may_gc"));
    assert!(!dump.contains("SignatureId"));
    p.functions[0].blocks[0].terminator = Terminator::Return(Some(Operand::Symbol(SymbolId(99))));
    assert!(p.validate().is_err());
    assert!(p.dump().contains("@<invalid:99>"));
}

#[test]
fn null_check_is_a_branch_to_an_abort_block() {
    let mut p = example();
    let abort_sig = p.intern_signature(Signature {
        params: vec![IrType::Ptr, IrType::Int32],
        result: None,
    });
    p.symbols.push(Symbol {
        name: "lo_abort_null_receiver".into(),
        kind: SymbolKind::Function(abort_sig),
    });
    p.symbols.push(Symbol {
        name: "method_name".into(),
        kind: SymbolKind::Data,
    });
    p.data.push(DataDef {
        symbol: SymbolId(2),
        section: Section::ReadOnly,
        align: 1,
        items: vec![DataItem::Bytes(b"draw".to_vec())],
    });
    p.signatures[0].params.push(IrType::Ref);
    p.functions[0].params.push(VirtualRegId(1));
    p.functions[0].new_register(IrType::Ref, Some("receiver".into()));
    p.functions[0].new_register(IrType::Bool, None);
    p.functions[0].blocks = vec![
        BasicBlock {
            instructions: vec![Instruction {
                kind: InstructionKind::Binary {
                    dst: VirtualRegId(2),
                    op: BinaryOp::Eq,
                    lhs: Operand::Value(VirtualRegId(1)),
                    rhs: Operand::Null,
                },
                line: 1,
            }],
            terminator: Terminator::Branch {
                condition: Operand::Value(VirtualRegId(2)),
                then_block: BlockId(1),
                else_block: BlockId(2),
            },
            terminator_line: None,
        },
        BasicBlock {
            instructions: vec![],
            terminator: Terminator::Abort {
                target: SymbolId(1),
                args: vec![Operand::Symbol(SymbolId(2)), Operand::Int(4)],
            },
            terminator_line: None,
        },
        BasicBlock {
            instructions: vec![],
            terminator: Terminator::Return(Some(Operand::Int(0))),
            terminator_line: None,
        },
    ];
    p.validate().unwrap();
    let dump = p.dump();
    assert!(dump.contains("t2:Bool = receiver == null"));
    assert!(dump.contains("cbr t2, .L1, .L2"));
    assert!(dump.contains("abort @lo_abort_null_receiver(@method_name, 4)"));
}

#[test]
fn names_are_unambiguous_and_malformed_ir_can_be_dumped() {
    let mut p = example();
    p.functions[0].register_names[0] = Some("t1".into());
    p.functions[0].new_register(IrType::Int32, None);
    p.functions[0].blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Copy {
            dst: VirtualRegId(1),
            src: Operand::Int(7),
        },
        line: 1,
    });
    p.validate().unwrap();
    assert!(p.dump().contains("t1.v0:Int32 = 1"));
    assert!(p.dump().contains("t1:Int32 = 7"));
    p.functions[0].register_names.clear();
    assert!(p.validate().unwrap_err().contains("register_names"));
    assert!(p.dump().contains("t0:Int32 = 1"));
    p.signatures.push(Signature {
        params: vec![],
        result: Some(IrType::CodePtr(SignatureId(1))),
    });
    p.symbols.push(Symbol {
        name: "invalid".into(),
        kind: SymbolKind::Function(SignatureId(1)),
    });
    assert!(p.dump().contains("<invalid nested CodePtr>"));
}

#[test]
fn root_stores_read_initialized_values() {
    let mut p = example();
    p.functions[0].root_slots = 1;
    p.functions[0].new_register(IrType::Ref, None);
    p.functions[0].blocks[0].instructions.push(Instruction {
        kind: InstructionKind::RootStore {
            slot: 0,
            value: Operand::Value(VirtualRegId(1)),
        },
        line: 1,
    });
    assert!(p.validate().unwrap_err().contains("used before assignment"));
    p.functions[0].blocks[0].instructions.insert(
        0,
        Instruction {
            kind: InstructionKind::Copy {
                dst: VirtualRegId(1),
                src: Operand::Null,
            },
            line: 1,
        },
    );
    p.validate().unwrap();
}

#[test]
fn dead_defaults_are_removed_but_defaults_read_on_a_branch_survive() {
    let mut program = example();
    let function = &mut program.functions[0];
    function.blocks[0].instructions.push(copy(0));
    liveness::remove_dead_copies(function, DebugMode::Off);
    assert!(function.blocks[0].instructions.is_empty());
    assert_eq!(function.blocks[1].instructions.len(), 1);
    assert_eq!(function.blocks[2].instructions.len(), 1);
    function.blocks[0].instructions.push(copy(0));
    function.blocks[2].instructions.clear();
    liveness::remove_dead_copies(function, DebugMode::Off);
    assert_eq!(function.blocks[0].instructions.len(), 1);
    program.validate().unwrap();
}

#[test]
fn dead_copy_removal_preserves_calls_with_unused_results() {
    let mut program = example();
    let function = &mut program.functions[0];
    function.blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Call {
            dst: Some(VirtualRegId(0)),
            target: CallTarget::Direct(SymbolId(0)),
            args: vec![],
        },
        line: 1,
    });
    liveness::remove_dead_copies(function, DebugMode::Off);
    assert!(matches!(function.blocks[0].instructions[0].kind,
        InstructionKind::Call { .. }));
}

fn lowered(source: &str, debug: DebugMode) -> ProgramIr {
    let tokens = crate::lexer::tokenize(source).expect("lexes");
    let program = crate::parser::parse_program(&tokens).expect("parses");
    let (typed, classes) = crate::type_checker::check_program(program).expect("type checks");
    lower::lower_program(&typed, &classes, crate::layout::NATIVE, debug)
        .expect("lowers")
        .into_program()
}

fn function<'p>(program: &'p ProgramIr, symbol: &str) -> &'p FunctionIr {
    program
        .functions
        .iter()
        .find(|f| program.symbols[f.symbol.0].name == symbol)
        .expect(symbol)
}

const TERMINATOR_LINES_SOURCE: &str = "class Main () {
    int main() { return 0; }
}
class T () {
    int f(bool a, bool b) {
        int r;
        r = 0;
        if (a) {
            r = 1;
        } else {
            r = 2;
        }
        while ((a | b)) {
            break;
        }
        return 1;
    }
    void g() {
        int k;
        k = 3;
    }
}
";

#[test]
fn terminators_carry_their_statement_lines_and_connecting_jumps_carry_none() {
    let program = lowered(TERMINATOR_LINES_SOURCE, DebugMode::Off);
    let f = function(&program, "lo_method_1_T_1_f");
    let ends: Vec<(&str, Option<u32>)> = f
        .blocks
        .iter()
        .map(|block| {
            let kind = match block.terminator {
                Terminator::Jump(_) => "jump",
                Terminator::Branch { .. } => "branch",
                Terminator::Return(_) => "return",
                Terminator::Abort { .. } => "abort",
            };
            (kind, block.terminator_line)
        })
        .collect();
    // if (a): line 8; the `|` and the while test: line 13; break: 14; return 1: 16.
    assert!(ends.contains(&("branch", Some(8))), "{ends:?}");
    assert!(ends.contains(&("branch", Some(13))), "{ends:?}");
    assert!(ends.contains(&("jump", Some(14))), "{ends:?}");
    assert!(ends.contains(&("return", Some(16))), "{ends:?}");
    // Every other jump only connects blocks: the if join, the jump into the loop
    // test, the `|`'s join and the loop's back edge.
    let jumps_with_lines = ends
        .iter()
        .filter(|(kind, line)| *kind == "jump" && line.is_some())
        .count();
    assert_eq!(
        jumps_with_lines, 1,
        "only `break` is a jump with a line: {ends:?}"
    );

    // The implicit return closing a void body takes the last statement's line.
    let g = function(&program, "lo_method_1_T_1_g");
    let last = g.blocks.last().expect("g has blocks");
    assert!(matches!(last.terminator, Terminator::Return(None)));
    assert_eq!(last.terminator_line, Some(20));
}

#[test]
fn debug_build_moves_local_zero_init_to_line_0() {
    let init_line = |debug| {
        let program = lowered(TERMINATOR_LINES_SOURCE, debug);
        let f = function(&program, "lo_method_1_T_1_f");
        let first = &f.blocks[f.entry.0].instructions[0];
        assert!(
            matches!(first.kind, InstructionKind::Copy { .. }),
            "zero-init of r"
        );
        first.line
    };
    assert_eq!(
        init_line(DebugMode::Off),
        5,
        "default build: the method's line"
    );
    assert_eq!(init_line(DebugMode::On), GENERATED_LINE);
}

#[test]
fn debug_build_keeps_copies_into_named_values() {
    let source = "class Main () {
    int main() {
        int x;
        int y;
        x = 5;
        y = 1;
        return y;
    }
}
";
    // Copies into x after cleanup: its zero-init and `x = 5`, both never read.
    let copies_into_x = |debug| {
        let mut program = lowered(source, debug);
        let main = program
            .functions
            .iter_mut()
            .find(|f| f.register_names.iter().any(|n| n.as_deref() == Some("x")))
            .expect("main");
        liveness::remove_dead_copies(main, debug);
        let x = main
            .register_names
            .iter()
            .position(|n| n.as_deref() == Some("x"))
            .expect("x");
        main.blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| matches!(i.kind, InstructionKind::Copy { dst, .. } if dst.0 == x))
            .count()
    };
    assert_eq!(copies_into_x(DebugMode::Off), 0, "default build drops both");
    assert_eq!(
        copies_into_x(DebugMode::On),
        2,
        "-g keeps the zero-init and x = 5"
    );
}

#[test]
fn debug_build_gives_named_values_their_own_stack_slots() {
    use register_allocator::{
        allocate_registers, PhysicalLocation, SpillSlotId, TargetConstraints,
    };
    let source = "class Main () {
    int main() { return 0; }
}
class Calc () {
    int f(int a, int b) {
        int x;
        int y;
        x = (a + b);
        y = (x * a);
        return (x + y);
    }
}
";
    // Where each register of Calc.f ends up, the way the driver builds it.
    let allocate = |debug| {
        let tokens = crate::lexer::tokenize(source).expect("lexes");
        let program = crate::parser::parse_program(&tokens).expect("parses");
        let (typed, classes) = crate::type_checker::check_program(program).expect("type checks");
        let ir =
            lower::lower_program(&typed, &classes, crate::layout::NATIVE, debug).expect("lowers");
        let mut program = gc_roots::insert_gc_roots(ir, debug);
        for function in &mut program.functions {
            liveness::remove_dead_copies(function, debug);
        }
        let checked = program.verify().expect("verifies");
        let allocation = allocate_registers(&checked, &TargetConstraints::system_v(), debug);
        let f = function(checked.program(), "lo_method_4_Calc_1_f");
        let names = f.register_names.clone();
        let locations = allocation.function_allocations[&f.symbol]
            .register_locations
            .clone();
        (names, locations)
    };

    let (names, debug) = allocate(DebugMode::On);
    let named: Vec<_> = (0..names.len()).filter(|&n| names[n].is_some()).collect();
    assert_eq!(named.len(), 5, "this, a, b, x, y");
    for (slot, &register) in named.iter().enumerate() {
        assert_eq!(
            debug[&VirtualRegId(register)],
            PhysicalLocation::Spill(SpillSlotId(slot)),
            "{:?} has its own stack slot under -g",
            names[register]
        );
    }

    let (_, default) = allocate(DebugMode::Off);
    assert!(
        named.iter().any(|&register| matches!(
            default.get(&VirtualRegId(register)),
            Some(PhysicalLocation::Register(_))
        )),
        "the default build still keeps some named values in registers"
    );
}

/// Lowered, rooted, cleaned and verified IR, as the driver builds it.
fn checked_ir(source: &str, debug: DebugMode) -> CheckedIr {
    let tokens = crate::lexer::tokenize(source).expect("lexes");
    let program = crate::parser::parse_program(&tokens).expect("parses");
    let (typed, classes) = crate::type_checker::check_program(program).expect("type checks");
    let ir = lower::lower_program(&typed, &classes, crate::layout::NATIVE, debug).expect("lowers");
    let mut program = gc_roots::insert_gc_roots(ir, debug);
    for function in &mut program.functions {
        liveness::remove_dead_copies(function, debug);
    }
    program.verify().expect("verifies")
}

/// `c` is read only as the receiver of `c.run(7)`, so it is never live across a
/// call: the default build never roots it, and a collection during `c.run`
/// would leave its home slot pointing at the old object.
const NAMED_REFS_SOURCE: &str = "class Main () {
    int main() {
        Calc c;
        int n;
        c = new Calc();
        n = c.run(7);
        return n;
    }
}
class Calc () {
    int run(int x) { return x; }
}
";

#[test]
fn debug_build_saves_and_reloads_every_named_ref_around_every_call() {
    // Around `c.run(7)`, the last call that sees `c`: whether `c` is saved into
    // a root slot just before it, whether it is reloaded just after it, and
    // whether any root slot holding `c` is ever set to null.
    let around_last_call = |debug| {
        let ir = checked_ir(NAMED_REFS_SOURCE, debug);
        let main = function(ir.program(), "lo_method_4_Main_4_main");
        let c = main
            .register_names
            .iter()
            .position(|name| name.as_deref() == Some("c"))
            .map(VirtualRegId)
            .expect("c");
        let block = main
            .blocks
            .iter()
            .find(|block| {
                block.instructions.iter().any(|i| {
                    matches!(
                        i.kind,
                        InstructionKind::Call {
                            target: CallTarget::Indirect(_),
                            ..
                        }
                    )
                })
            })
            .expect("the block calling c.run");
        let call = block
            .instructions
            .iter()
            .position(|i| matches!(i.kind, InstructionKind::Call { .. }))
            .expect("c.run");
        let saved_before = block.instructions[..call].iter().any(|i| {
            matches!(i.kind, InstructionKind::RootStore { value: Operand::Value(v), .. } if v == c)
        });
        let reloaded_after = block.instructions[call + 1..]
            .iter()
            .any(|i| matches!(i.kind, InstructionKind::RootLoad { dst, .. } if dst == c));
        let instructions = || main.blocks.iter().flat_map(|b| &b.instructions);
        let slots_of_c: Vec<u32> = instructions()
            .filter_map(|i| match i.kind {
                InstructionKind::RootStore {
                    slot,
                    value: Operand::Value(v),
                } if v == c => Some(slot),
                _ => None,
            })
            .collect();
        let c_slot_nulled = instructions().any(|i| {
            matches!(i.kind, InstructionKind::RootStore { slot, value: Operand::Null }
                if slots_of_c.contains(&slot))
        });
        (saved_before, reloaded_after, c_slot_nulled)
    };

    // The default build stops tracking `c` once the program is done with it.
    assert_eq!(around_last_call(DebugMode::Off), (false, false, false));
    // -g keeps `c` in its root slot across every call and never clears it.
    assert_eq!(around_last_call(DebugMode::On), (true, true, false));
}

#[test]
fn debug_build_gives_every_function_its_frame_info() {
    use register_allocator::{allocate_registers, TargetConstraints};
    let assembly = |debug| {
        let ir = checked_ir(NAMED_REFS_SOURCE, debug);
        let allocation = allocate_registers(&ir, &TargetConstraints::system_v(), debug);
        crate::codegen::emit_program(ir.program(), &allocation, debug).expect("emits")
    };

    assert!(!assembly(DebugMode::Off).contains(".cfi"));

    let asm = assembly(DebugMode::On);
    let lines: Vec<&str> = asm.lines().map(str::trim).collect();
    let starts: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i] == ".cfi_startproc")
        .collect();
    // Every function the program defines, plus the C `main` wrapper.
    let functions = asm
        .lines()
        .filter(|line| line.starts_with(".globl "))
        .count();
    assert_eq!(starts.len(), functions);
    assert_eq!(
        lines.iter().filter(|line| **line == ".cfi_endproc").count(),
        functions
    );
    for &start in &starts {
        assert_eq!(
            lines[start..start + 6],
            [
                ".cfi_startproc",
                "push rbp",
                ".cfi_def_cfa_offset 16",
                ".cfi_offset 6, -16",
                "mov rbp, rsp",
                ".cfi_def_cfa_register 6",
            ],
            "the three frame-info states follow {}",
            lines[start - 1]
        );
    }
}
