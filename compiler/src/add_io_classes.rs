use crate::ast::{ClassDecl, IoOp, MethodBody, MethodDecl, Param, Program, Type};

const SYNTHETIC_LINE: u32 = 0;

pub fn add_io_classes(mut program: Program) -> Program {
    let mut classes = vec![input_class(), output_class()];
    classes.append(&mut program.classes);
    program.classes = classes;
    program
}

fn input_class() -> ClassDecl {
    ClassDecl {
        name: "Input".to_string(),
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
        name: "Output".to_string(),
        extends: None,
        fields: vec![],
        constructors: vec![],
        methods: vec![
            io_method(
                Type::Void,
                "print_int",
                IoOp::PrintInt,
                vec![io_param(Type::Int, "n")],
            ),
            io_method(
                Type::Void,
                "print_bool",
                IoOp::PrintBool,
                vec![io_param(Type::Bool, "b")],
            ),
            io_method(
                Type::Void,
                "print_string",
                IoOp::PrintString,
                vec![io_param(Type::String, "s")],
            ),
            io_method(Type::Void, "println", IoOp::Println, vec![]),
        ],
        line: SYNTHETIC_LINE,
    }
}

fn io_method(return_type: Type, name: &str, op: IoOp, params: Vec<Param>) -> MethodDecl {
    MethodDecl {
        return_type,
        name: name.to_string(),
        params,
        body: MethodBody::Io(op),
        line: SYNTHETIC_LINE,
    }
}

fn io_param(declared_type: Type, name: &str) -> Param {
    Param {
        declared_type,
        name: name.to_string(),
        line: SYNTHETIC_LINE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn method<'p>(class: &'p ClassDecl, name: &str) -> &'p MethodDecl {
        class
            .methods
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("{} has no method named {name}", class.name))
    }

    fn io_op(m: &MethodDecl) -> &IoOp {
        match &m.body {
            MethodBody::Io(op) => op,
            MethodBody::UserDefined(_) => panic!("{} body is not Io", m.name),
        }
    }

    #[test]
    fn prepends_input_and_output_before_user_classes() {
        let user_class = ClassDecl {
            name: "Main".into(),
            extends: None,
            fields: vec![],
            constructors: vec![],
            methods: vec![],
            line: 1,
        };
        let program = add_io_classes(Program {
            classes: vec![user_class],
        });
        let names: Vec<&str> = program.classes.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Input", "Output", "Main"]);
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
        assert_eq!(input.name, "Input");

        let read_int = method(input, "read_int");
        assert_eq!(read_int.return_type, Type::Int);
        assert!(read_int.params.is_empty());
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
        assert_eq!(output.name, "Output");

        let print_int = method(output, "print_int");
        assert_eq!(print_int.return_type, Type::Void);
        assert_eq!(print_int.params, vec![io_param(Type::Int, "n")]);
        assert_eq!(*io_op(print_int), IoOp::PrintInt);

        let print_bool = method(output, "print_bool");
        assert_eq!(print_bool.params, vec![io_param(Type::Bool, "b")]);
        assert_eq!(*io_op(print_bool), IoOp::PrintBool);

        let print_string = method(output, "print_string");
        assert_eq!(print_string.params, vec![io_param(Type::String, "s")]);
        assert_eq!(*io_op(print_string), IoOp::PrintString);

        let println = method(output, "println");
        assert_eq!(println.return_type, Type::Void);
        assert!(println.params.is_empty());
        assert_eq!(*io_op(println), IoOp::Println);
    }
}
