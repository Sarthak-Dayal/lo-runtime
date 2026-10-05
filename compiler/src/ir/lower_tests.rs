use super::*;
use crate::ast::Type;
use crate::type_checker::TypedMethodDecl;

fn program(method: &TypedMethodDecl) -> Result<CheckedIr, String> {
    let ty = |ty: &Type| match ty {
        Type::Int => IrType::Int32,
        Type::Bool => IrType::Bool,
        Type::Class(_) | Type::String => IrType::Ref,
        Type::Void => panic!("void parameter"),
    };
    let mut params = vec![IrType::Ref];
    params.extend(method.formals.iter().map(|(_, t)| ty(t)));
    ProgramIr {
        data: vec![],
        startup: None,
        symbols: vec![Symbol {
            name: method.method_name.clone(),
            kind: SymbolKind::Function(SignatureId(0)),
        }],
        signatures: vec![Signature {
            params,
            result: if method.return_type == Type::Void {
                None
            } else {
                Some(ty(&method.return_type))
            },
        }],
        functions: vec![lower::lower_method(SymbolId(0), method)?],
    }
    .verify()
}

// Execute just the scalar CFG subset to check behavior, not only IR shape.
fn execute(function: &FunctionIr, args: &[i32]) -> Option<i32> {
    execute_with_trace(function, args).0
}

fn execute_with_trace(function: &FunctionIr, args: &[i32]) -> (Option<i32>, usize) {
    let mut divisions = 0;
    let mut values = vec![0; function.value_types.len()];
    for (&id, &value) in function.params.iter().zip(args) {
        values[id.0] = value;
    }
    let read = |op: Operand, values: &[i32]| match op {
        Operand::Int(n) => n,
        Operand::Bool(b) => i32::from(b),
        Operand::Null => 0,
        Operand::Value(id) => values[id.0],
        _ => panic!("unexpected symbol"),
    };
    let mut block = function.entry;
    for _ in 0..1000 {
        let body = &function.blocks[block.0];
        for instruction in &body.instructions {
            let (dst, value) = match instruction.kind {
                InstructionKind::Copy { dst, src } => (dst, read(src, &values)),
                InstructionKind::Unary { dst, op, src } => (
                    dst,
                    match op {
                        UnaryOp::Neg => read(src, &values).wrapping_neg(),
                        UnaryOp::Not => i32::from(read(src, &values) == 0),
                    },
                ),
                InstructionKind::Binary { dst, op, lhs, rhs } => {
                    let (a, b) = (read(lhs, &values), read(rhs, &values));
                    (
                        dst,
                        match op {
                            BinaryOp::Add => a.wrapping_add(b),
                            BinaryOp::Sub => a.wrapping_sub(b),
                            BinaryOp::Mul => a.wrapping_mul(b),
                            BinaryOp::Div => {
                                divisions += 1;
                                assert!(b != 0 && b != -1, "unguarded division reached execution");
                                a / b
                            }
                            BinaryOp::Mod => {
                                divisions += 1;
                                assert!(b != 0 && b != -1, "unguarded remainder reached execution");
                                a % b
                            }
                            BinaryOp::Eq => i32::from(a == b),
                            BinaryOp::Lt => i32::from(a < b),
                            BinaryOp::Gt => i32::from(a > b),
                        },
                    )
                }
                _ => panic!("unexpected instruction"),
            };
            values[dst.0] = value;
        }
        block = match body.terminator {
            Terminator::Jump(next) => next,
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => {
                if read(condition, &values) != 0 {
                    then_block
                } else {
                    else_block
                }
            }
            Terminator::Return(value) => return (value.map(|v| read(v, &values)), divisions),
            _ => panic!("unexpected abort"),
        };
    }
    panic!("CFG did not terminate");
}

fn compare(body: &str, expected: i32) -> usize {
    let source = format!("class Main () {{ int main() {{ {body} }} }}");
    let tokens = crate::lexer::tokenize(&source).unwrap();
    let ast = crate::parser::parse_program(&tokens).unwrap();
    let (typed, table) = crate::type_checker::check_program(ast).unwrap();
    let method = &typed
        .classes
        .iter()
        .find(|c| c.class_name == "Main")
        .unwrap()
        .methods[0];
    let checked = program(method).ok().unwrap();
    let (result, divisions) = execute_with_trace(&checked.program().functions[0], &[1]);
    assert_eq!(result, Some(expected));
    let io = crate::interpreter::Io::new(
        Box::new(std::io::Cursor::new(Vec::<u8>::new())),
        Box::new(std::io::sink()),
        Box::new(std::io::sink()),
    );
    assert_eq!(
        crate::interpreter::interpret(&typed, &table, crate::interpreter::DEFAULT_HEAP_LIMIT, io),
        crate::interpreter::Outcome::Exit(expected)
    );
    divisions
}

#[test]
fn scalar_lowering_matches_interpreter() {
    compare("int x; bool b; if (b) { return 99; } else { x = ((7 * 3) + (8 % 3)); } return (x - (9 / 3));", 20);
    compare("return (true ? (false ? 1 : 2) : 3);", 2);
    compare("if ((! false)) { return (~ 7); } else { return 9; }", -7);
    compare(
        "Main x; if ((x = null)) { return 1; } else { return 0; }",
        1,
    );
}

#[test]
fn loops_and_break_use_innermost_exit() {
    compare("int i, total; while ((i < 3)) { i = (i + 1); while (true) { total = (total + 2); break; total = 99; } } return total;", 6);
    compare(
        "int i; while ((i < 2)) { i = (i + 1); if ((i = 2)) { return i; } else { ; } } return 0;",
        2,
    );
}

#[test]
fn short_circuit_skips_rhs() {
    assert_eq!(
        compare(
            "if ((false & ((4 / 2) = 0))) { return 1; } else { return 2; }",
            2
        ),
        0
    );
    assert_eq!(
        compare(
            "if ((true | ((4 / 2) = 0))) { return 3; } else { return 4; }",
            3
        ),
        0
    );
    assert_eq!(compare("return (true ? 5 : (4 / 2));", 5), 0);
    assert_eq!(
        compare(
            "if ((true & ((4 / 2) = 2))) { return 1; } else { return 0; }",
            1
        ),
        1
    );
    assert_eq!(
        compare(
            "if ((false | ((4 / 2) = 2))) { return 1; } else { return 0; }",
            1
        ),
        1
    );
}

#[test]
fn integer_boundary_cases_follow_lo_reference() {
    compare("return (7 / 0);", -1);
    compare("return (7 % 0);", 7);
    compare("return ((2147483647 + 1) / (~ 1));", i32::MIN);
    compare("return ((2147483647 + 1) % (~ 1));", 0);
    compare("return (2147483647 + 1);", i32::MIN);
}

#[test]
fn unsupported_runtime_operations_are_errors() {
    for body in [
        "out.println(); return 0;",
        "String s; return 0;",
        "Main x; x = new Main(); return 0;",
    ] {
        let source = format!("class Main () {{ int main() {{ {body} }} }}");
        let tokens = crate::lexer::tokenize(&source).unwrap();
        let ast = crate::parser::parse_program(&tokens).unwrap();
        let (typed, _) = crate::type_checker::check_program(ast).unwrap();
        let method = &typed
            .classes
            .iter()
            .find(|c| c.class_name == "Main")
            .unwrap()
            .methods[0];
        assert!(program(method).err().unwrap().contains("not implemented"));
    }
}

#[test]
fn receiver_formals_and_void_fallthrough() {
    let tokens = crate::lexer::tokenize("class Main () { int main() { return 0; } int f(int x) { x = (x + 1); return x; } void g() { ; } }").unwrap();
    let ast = crate::parser::parse_program(&tokens).unwrap();
    let (typed, _) = crate::type_checker::check_program(ast).unwrap();
    let methods = &typed
        .classes
        .iter()
        .find(|c| c.class_name == "Main")
        .unwrap()
        .methods;
    let f = program(&methods[1]).ok().unwrap();
    assert_eq!(f.program().functions[0].params.len(), 2);
    assert_eq!(execute(&f.program().functions[0], &[1, 41]), Some(42));
    let g = program(&methods[2]).ok().unwrap();
    assert_eq!(execute(&g.program().functions[0], &[1]), None);
}

#[test]
fn variable_division_guards_all_boundary_cases() {
    for operator in ["/", "%"] {
        let source = format!("class Main () {{ int main() {{ return 0; }} int calculate(int a, int b) {{ return (a {operator} b); }} }}");
        let tokens = crate::lexer::tokenize(&source).unwrap();
        let ast = crate::parser::parse_program(&tokens).unwrap();
        let (typed, _) = crate::type_checker::check_program(ast).unwrap();
        let method = &typed
            .classes
            .iter()
            .find(|c| c.class_name == "Main")
            .unwrap()
            .methods[1];
        let checked = program(method).ok().unwrap();
        let function = &checked.program().functions[0];
        for a in [i32::MIN, -7, 0, 7, i32::MAX] {
            for b in [-3, -1, 0, 1, 3] {
                let expected = match (operator, b) {
                    ("/", 0) => -1,
                    ("%", 0) => a,
                    ("/", _) => a.wrapping_div(b),
                    _ => a.wrapping_rem(b),
                };
                let (actual, divisions) = execute_with_trace(function, &[1, a, b]);
                assert_eq!(actual, Some(expected), "{a} {operator} {b}");
                assert_eq!(divisions, usize::from(b != 0 && b != -1));
            }
        }
        let dump = checked.program().dump();
        assert!(dump.contains("this:Ref, a:Int32, b:Int32"));
        assert!(dump.contains(" == 0"));
        assert!(dump.contains(" == -1"));
    }
}

#[test]
fn names_and_nested_division_survive_cfg_lowering() {
    compare("int x, b; x = 7; b = 0; return ((x / b) + (x % b));", 6);
    compare("int b; b = (~ 1); return ((2147483647 + 1) / b);", i32::MIN);
    compare(
        "int i, sum; while ((i < 3)) { sum = (sum + (6 / (i - 1))); i = (i + 1); } return sum;",
        -1,
    );
    let tokens = crate::lexer::tokenize(
        "class Main () { int main() { int count; count = 2; return count; } }",
    )
    .unwrap();
    let ast = crate::parser::parse_program(&tokens).unwrap();
    let (typed, _) = crate::type_checker::check_program(ast).unwrap();
    let method = &typed
        .classes
        .iter()
        .find(|c| c.class_name == "Main")
        .unwrap()
        .methods[0];
    let checked = program(method).ok().unwrap();
    let dump = checked.program().dump();
    assert!(dump.contains("count:Int32 = 0"));
    assert!(dump.contains("count:Int32 = 2"));
    assert!(dump.contains(" = count"));
    assert_eq!(checked.program().functions[0].root_slots, 0);
}
