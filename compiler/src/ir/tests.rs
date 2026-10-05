use super::*;

fn copy(value: i32) -> Instruction {
    Instruction {
        kind: InstructionKind::Copy {
            dst: ValueId(0),
            src: Operand::Int(value),
        },
        line: 1,
    }
}
fn example() -> ProgramIr {
    ProgramIr {
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
            value_types: vec![IrType::Int32],
            entry: BlockId(0),
            blocks: vec![
                BasicBlock {
                    instructions: vec![],
                    terminator: Terminator::Branch {
                        condition: Operand::Bool(true),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    },
                },
                BasicBlock {
                    instructions: vec![copy(1)],
                    terminator: Terminator::Jump(BlockId(3)),
                },
                BasicBlock {
                    instructions: vec![copy(2)],
                    terminator: Terminator::Jump(BlockId(3)),
                },
                BasicBlock {
                    instructions: vec![],
                    terminator: Terminator::Return(Some(Operand::Value(ValueId(0)))),
                },
            ],
        }],
    }
}

#[test]
fn mutable_merge_and_readable_dump() {
    let p = example();
    p.validate().unwrap();
    assert_eq!(p.dump(), "@0 = \"example\" Function(SignatureId(0))\nsig0 [] -> Some(Int32)\n\nfunc @0() entry .L0 {\n  t0: Int32\n.L0:\n  cbr true, .L1, .L2\n.L1:\n  t0 = 1 ; line 1\n  br .L3\n.L2:\n  t0 = 2 ; line 1\n  br .L3\n.L3:\n  ret t0\n}\n");
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
            dst: ValueId(0),
            src: Operand::Value(ValueId(0)),
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
            dst: ValueId(0),
            op: BinaryOp::Add,
            lhs: Operand::Value(ValueId(0)),
            rhs: Operand::Int(1),
        },
        line: 2,
    }];
    p.functions[0].blocks[1].terminator = Terminator::Branch {
        condition: Operand::Bool(false),
        then_block: BlockId(1),
        else_block: BlockId(3),
    };
    p.validate().unwrap();
}

#[test]
fn rejects_invalid_targets_ids_and_types() {
    let mut p = example();
    p.functions[0].blocks[0].terminator = Terminator::Jump(BlockId(99));
    assert!(p.validate().unwrap_err().contains("invalid target"));
    p.functions[0].blocks[0].terminator = Terminator::Return(Some(Operand::Value(ValueId(99))));
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
    p.functions[0]
        .value_types
        .push(IrType::CodePtr(SignatureId(1)));
    p.functions[0].blocks[0].instructions = vec![
        Instruction {
            kind: InstructionKind::Copy {
                dst: ValueId(1),
                src: Operand::Symbol(SymbolId(1)),
            },
            line: 1,
        },
        Instruction {
            kind: InstructionKind::Call {
                dst: Some(ValueId(0)),
                target: CallTarget::Indirect(Operand::Value(ValueId(1))),
                signature: SignatureId(1),
                args: vec![Operand::Null],
                effects: CallEffects::default(),
            },
            line: 2,
        },
    ];
    p.validate().unwrap();
    assert!(p.dump().contains("call_indirect t1, null ; sig1"));
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
        signature: SignatureId(1),
        args: vec![Operand::Null, Operand::Int(0), Operand::Null],
        effects: CallEffects::default(),
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
    p.functions[0].value_types.push(IrType::Ref);
    p.functions[0].params.push(ValueId(1));
    p.signatures[0].params.push(IrType::Ref);
    p.functions[0].blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Store {
            base: Operand::Value(ValueId(1)),
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
            signature: SignatureId(1),
            args: vec![Operand::Symbol(SymbolId(1))],
            effects: CallEffects::default(),
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
    assert!(p.dump().contains("abort @1, @2, 4"));
    if let Terminator::Abort { args, .. } = &mut p.functions[0].blocks[0].terminator {
        args[1] = Operand::Value(ValueId(0));
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
    assert!(checked.program().dump().contains("func @0"));
    let mut program = checked.into_program();
    program.functions[0].entry = BlockId(99);
    assert!(program.verify().err().unwrap().contains("invalid entry"));
}

fn indirect_example() -> ProgramIr {
    let mut p = example();
    p.signatures.push(p.signatures[0].clone());
    p.functions[0]
        .value_types
        .push(IrType::CodePtr(SignatureId(1)));
    p.functions[0].blocks[0].instructions = vec![
        Instruction {
            kind: InstructionKind::Copy {
                dst: ValueId(1),
                src: Operand::Symbol(SymbolId(0)),
            },
            line: 1,
        },
        Instruction {
            kind: InstructionKind::Call {
                dst: Some(ValueId(0)),
                target: CallTarget::Indirect(Operand::Value(ValueId(1))),
                signature: SignatureId(0),
                args: vec![],
                effects: CallEffects::default(),
            },
            line: 2,
        },
    ];
    p
}

#[test]
fn callable_signatures_survive_copies_and_indirect_calls() {
    let mut p = indirect_example();
    // The copy and call use equivalent signatures with different IDs.
    p.validate().unwrap();
    p.signatures.push(Signature {
        params: vec![IrType::Ref],
        result: Some(IrType::Int32),
    });
    if let InstructionKind::Call {
        signature, args, ..
    } = &mut p.functions[0].blocks[0].instructions[1].kind
    {
        *signature = SignatureId(2);
        args.push(Operand::Null);
    }
    assert!(p.validate().unwrap_err().contains("CodePtr"));
    // Changing the destination annotation cannot disguise a mismatched copy.
    p.functions[0].value_types[1] = IrType::CodePtr(SignatureId(2));
    assert!(p.validate().unwrap_err().contains("line 1"));
}

#[test]
fn callable_loads_check_declared_signatures() {
    let mut p = indirect_example();
    p.symbols.push(Symbol {
        name: "vtable".into(),
        kind: SymbolKind::Data,
    });
    p.functions[0].blocks[0].instructions[0].kind = InstructionKind::Load {
        dst: ValueId(1),
        base: Operand::Symbol(SymbolId(1)),
        offset: 0,
    };
    p.validate().unwrap();
    p.signatures[1].result = Some(IrType::Bool);
    assert!(p.validate().unwrap_err().contains("CodePtr"));
}

#[test]
fn rejects_unknown_callable_signature_ids() {
    let mut p = example();
    p.functions[0]
        .value_types
        .push(IrType::CodePtr(SignatureId(99)));
    assert!(p.validate().unwrap_err().contains("unknown signature"));
    p.functions[0].value_types.pop();
    p.signatures.push(Signature {
        params: vec![],
        result: Some(IrType::CodePtr(SignatureId(99))),
    });
    assert!(p.validate().unwrap_err().contains("unknown signature"));
}

#[test]
fn recursive_callable_signatures_compare_structurally() {
    let mut p = example();
    p.signatures.extend([
        Signature {
            params: vec![IrType::CodePtr(SignatureId(1))],
            result: None,
        },
        Signature {
            params: vec![IrType::CodePtr(SignatureId(2))],
            result: None,
        },
    ]);
    p.symbols.push(Symbol {
        name: "callback".into(),
        kind: SymbolKind::Function(SignatureId(1)),
    });
    p.functions[0]
        .value_types
        .push(IrType::CodePtr(SignatureId(2)));
    p.functions[0].blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Copy {
            dst: ValueId(1),
            src: Operand::Symbol(SymbolId(1)),
        },
        line: 1,
    });
    p.validate().unwrap();
    p.signatures[2].result = Some(IrType::Int32);
    assert!(p.validate().is_err());
}
