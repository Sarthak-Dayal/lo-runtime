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
            signature: Some(SignatureId(0)),
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
    p.verify().unwrap();
    assert_eq!(p.dump(), "@0 = \"example\"\nsig0 [] -> Some(Int32)\n\nfunction @0() entry b0 {\n  v0: Int32\nb0:\n  branch true, b1, b2\nb1:\n  v0 = copy 1 ; line 1\n  jump b3\nb2:\n  v0 = copy 2 ; line 1\n  jump b3\nb3:\n  return v0\n}\n");
}

#[test]
fn rejects_uninitialized_merge_and_loop() {
    let mut p = example();
    p.functions[0].blocks[2].instructions.clear();
    assert!(p.verify().unwrap_err().contains("used before assignment"));
    // A definition on a back edge cannot initialize the first iteration.
    p.functions[0].blocks[2].terminator = Terminator::Jump(BlockId(0));
    p.functions[0].blocks[0].instructions.push(Instruction {
        kind: InstructionKind::Copy {
            dst: ValueId(0),
            src: Operand::Value(ValueId(0)),
        },
        line: 1,
    });
    assert!(p.verify().unwrap_err().contains("used before assignment"));
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
    p.verify().unwrap();
}

#[test]
fn rejects_invalid_targets_ids_and_types() {
    let mut p = example();
    p.functions[0].blocks[0].terminator = Terminator::Jump(BlockId(99));
    assert!(p.verify().unwrap_err().contains("invalid target"));
    p.functions[0].blocks[0].terminator = Terminator::Return(Some(Operand::Value(ValueId(99))));
    assert!(p.verify().unwrap_err().contains("unknown value"));
    p.functions[0].blocks[0].terminator = Terminator::Return(Some(Operand::Bool(true)));
    assert!(p.verify().unwrap_err().contains("return type"));
}

#[test]
fn indirect_calls_check_signature_and_initialization() {
    let mut p = example();
    p.symbols.push(Symbol {
        name: "callee".into(),
        signature: Some(SignatureId(1)),
    });
    p.signatures.push(Signature {
        params: vec![IrType::Ref],
        result: Some(IrType::Int32),
    });
    p.functions[0].value_types.push(IrType::CodePtr);
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
    p.verify().unwrap();
    assert!(p.dump().contains("call_indirect v1 sig1(null)"));
    if let InstructionKind::Call { args, .. } = &mut p.functions[0].blocks[0].instructions[1].kind {
        args.clear();
    }
    assert!(p.verify().unwrap_err().contains("argument count"));
}

#[test]
fn reference_store_requires_barrier_operation() {
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
    assert!(p.verify().unwrap_err().contains("StoreRef"));
    p.functions[0].blocks[0].instructions[0].kind = InstructionKind::StoreRef {
        object: Operand::Null,
        offset: 0,
        value: Operand::Null,
    };
    p.verify().unwrap();
}
