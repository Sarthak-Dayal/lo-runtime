use crate::ast::{ClassDecl, Formal, IoOp, MethodBody, MethodDecl, Program, Type};

const SYNTHETIC_LINE: u32 = 0;

/// The classes this module injects, in injection order. This is the single
/// source of truth for "which classes are synthetic preamble" — callers
/// (e.g. the type checker's `ClassKind` assignment) should check membership
/// here rather than reverse-engineering it from `line == 0` or any other
/// structural signal.
pub const CLASS_NAMES: [&str; 2] = ["Input", "Output"];

/// Prepends the `Input`/`Output` preamble classes to `program`. This must be
/// called exactly once, from `type_checker::check_program`, before any other
/// pass runs — calling it twice would silently duplicate both classes.
pub fn add_io_classes(mut program: Program) -> Program {
    debug_assert!(
        !program.classes.iter().any(|c| CLASS_NAMES.contains(&c.class_name.as_str())),
        "add_io_classes called on a program that already has the preamble classes"
    );
    let mut classes = vec![input_class(), output_class()];
    classes.append(&mut program.classes);
    program.classes = classes;
    program
}

fn input_class() -> ClassDecl {
    ClassDecl {
        class_name: "Input".to_string(),
        extends: None,
        fields: vec![],
        constructors: vec![],
        methods: vec![
            io_method(Type::Int, "read_int", IoOp::ReadInt, vec![]),
            io_method(Type::Bool, "read_bool", IoOp::ReadBool, vec![]),
            io_method(Type::String, "read_string", IoOp::ReadString, vec![]),
            io_method(Type::Bool, "eof", IoOp::Eof, vec![]),
        ],
        line: SYNTHETIC_LINE,
    }
}

fn output_class() -> ClassDecl {
    ClassDecl {
        class_name: "Output".to_string(),
        extends: None,
        fields: vec![],
        constructors: vec![],
        methods: vec![
            io_method(
                Type::Void,
                "print_int",
                IoOp::PrintInt,
                vec![io_formal(Type::Int, "n")],
            ),
            io_method(
                Type::Void,
                "print_bool",
                IoOp::PrintBool,
                vec![io_formal(Type::Bool, "b")],
            ),
            io_method(
                Type::Void,
                "print_string",
                IoOp::PrintString,
                vec![io_formal(Type::String, "s")],
            ),
            io_method(Type::Void, "println", IoOp::Println, vec![]),
        ],
        line: SYNTHETIC_LINE,
    }
}

fn io_method(return_type: Type, method_name: &str, op: IoOp, formals: Vec<Formal>) -> MethodDecl {
    MethodDecl {
        return_type,
        method_name: method_name.to_string(),
        formals,
        body: MethodBody::Io(op),
        line: SYNTHETIC_LINE,
    }
}

fn io_formal(declared_type: Type, identifier: &str) -> Formal {
    Formal {
        declared_type,
        identifier: identifier.to_string(),
        line: SYNTHETIC_LINE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn method<'p>(class: &'p ClassDecl, method_name: &str) -> &'p MethodDecl {
        class
            .methods
            .iter()
            .find(|m| m.method_name == method_name)
            .unwrap_or_else(|| panic!("{} has no method named {method_name}", class.class_name))
    }

    fn io_op(m: &MethodDecl) -> &IoOp {
        match &m.body {
            MethodBody::Io(op) => op,
            MethodBody::UserDefined(_) => panic!("{} body is not Io", m.method_name),
        }
    }

    #[test]
    fn prepends_input_and_output_before_user_classes() {
        let user_class = ClassDecl {
            class_name: "Main".into(),
            extends: None,
            fields: vec![],
            constructors: vec![],
            methods: vec![],
            line: 1,
        };
        let program = add_io_classes(Program {
            classes: vec![user_class],
        });
        let names: Vec<&str> = program
            .classes
            .iter()
            .map(|c| c.class_name.as_str())
            .collect();
        let mut expected = CLASS_NAMES.to_vec();
        expected.push("Main");
        assert_eq!(names, expected);
    }

    #[test]
    fn input_and_output_are_root_classes_with_no_declared_constructor() {
        let program = add_io_classes(Program { classes: vec![] });
        for class in &program.classes {
            assert_eq!(class.extends, None);
            assert!(class.fields.is_empty());
            assert!(class.constructors.is_empty());
        }
    }

    #[test]
    fn input_methods_match_the_language_reference_signatures() {
        let program = add_io_classes(Program { classes: vec![] });
        let input = &program.classes[0];
        assert_eq!(input.class_name, "Input");

        let read_int = method(input, "read_int");
        assert_eq!(read_int.return_type, Type::Int);
        assert!(read_int.formals.is_empty());
        assert_eq!(*io_op(read_int), IoOp::ReadInt);

        let read_bool = method(input, "read_bool");
        assert_eq!(read_bool.return_type, Type::Bool);
        assert_eq!(*io_op(read_bool), IoOp::ReadBool);

        let read_string = method(input, "read_string");
        assert_eq!(read_string.return_type, Type::String);
        assert_eq!(*io_op(read_string), IoOp::ReadString);

        let eof = method(input, "eof");
        assert_eq!(eof.return_type, Type::Bool);
        assert_eq!(*io_op(eof), IoOp::Eof);
    }

    #[test]
    fn output_methods_match_the_language_reference_signatures() {
        let program = add_io_classes(Program { classes: vec![] });
        let output = &program.classes[1];
        assert_eq!(output.class_name, "Output");

        let print_int = method(output, "print_int");
        assert_eq!(print_int.return_type, Type::Void);
        assert_eq!(print_int.formals, vec![io_formal(Type::Int, "n")]);
        assert_eq!(*io_op(print_int), IoOp::PrintInt);

        let print_bool = method(output, "print_bool");
        assert_eq!(print_bool.formals, vec![io_formal(Type::Bool, "b")]);
        assert_eq!(*io_op(print_bool), IoOp::PrintBool);

        let print_string = method(output, "print_string");
        assert_eq!(print_string.formals, vec![io_formal(Type::String, "s")]);
        assert_eq!(*io_op(print_string), IoOp::PrintString);

        let println = method(output, "println");
        assert_eq!(println.return_type, Type::Void);
        assert!(println.formals.is_empty());
        assert_eq!(*io_op(println), IoOp::Println);
    }
}
