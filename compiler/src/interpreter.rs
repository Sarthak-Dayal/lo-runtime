mod abort;
mod env;
mod eval;
mod heap;
mod io;
mod strings;
mod value;

pub use abort::AbortKind;
pub use heap::DEFAULT_HEAP_LIMIT;
pub use io::Io;

use crate::type_checker::{ClassTable, TypedProgram};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Clean finish
    Exit(i32),
    /// Runtime abort
    Abort(AbortKind),
}

pub fn interpret(program: &TypedProgram, table: &ClassTable, heap_limit: usize, io: Io) -> Outcome {
    eval::Interp::with_io(program, table, heap_limit, io).run()
}

#[cfg(test)]
mod tests {
    use super::heap::DEFAULT_HEAP_LIMIT;
    use super::*;

    /// Compile `src` through the real front end and run it with the given stdin,
    /// returning the outcome and whatever reached stdout.
    fn run(src: &str, input: &str) -> (Outcome, String) {
        let tokens = crate::lexer::tokenize(src).expect("lex failed");
        let program = crate::parser::parse_program(&tokens).expect("parse failed");
        let (typed, table) = crate::type_checker::check_program(program).expect("check failed");
        let out = io::SharedBuf::new();
        let ports = Io::new(
            Box::new(std::io::Cursor::new(input.as_bytes().to_vec())),
            Box::new(out.clone()),
            Box::new(io::SharedBuf::new()),
        );
        let outcome = interpret(&typed, &table, DEFAULT_HEAP_LIMIT, ports);
        (outcome, out.contents())
    }

    #[test]
    fn exits_with_main_return_value() {
        let (outcome, out) = run("class Main () { int main() { return 48; } }", "");
        assert_eq!(outcome, Outcome::Exit(48));
        assert_eq!(out, "");
    }

    #[test]
    fn prints_then_exits() {
        let (outcome, out) = run(
            "class Main () { int main() { out.print_int(7); return 0; } }",
            "",
        );
        assert_eq!(outcome, Outcome::Exit(0));
        assert_eq!(out, "7");
    }

    #[test]
    fn a_loop_computes_a_factorial() {
        let src = "
            class Main () {
                int main() {
                    int i; int acc;
                    i = 1; acc = 1;
                    while ((i < 6)) { acc = (acc * i); i = (i + 1); }
                    out.print_int(acc);
                    return 0;
                }
            }
        ";
        let (outcome, out) = run(src, "");
        assert_eq!(outcome, Outcome::Exit(0));
        assert_eq!(out, "120");
    }

    #[test]
    fn an_object_program_runs_end_to_end() {
        // Void method dispatched as a statement, field mutation, constructor init.
        let src = "
            class Counter (int n;) [ Counter(int start) { n = start; } ] {
                int get() { return n; }
                void bump() { n = (n + 1); }
            }
            class Main () {
                int main() { Counter c; c = new Counter(10); c.bump(); c.bump(); return c.get(); }
            }
        ";
        let (outcome, _out) = run(src, "");
        assert_eq!(outcome, Outcome::Exit(12));
    }

    #[test]
    fn a_failed_downcast_becomes_an_abort_outcome() {
        let src = "
            class Animal () { int kind() { return 1; } }
            class Cat extends Animal () [ Cat() { super(); } ] { int kind() { return 3; } }
            class Dog extends Animal () [ Dog() { super(); } ] { int kind() { return 2; } }
            class Main () { int main() { Animal a; Dog d; a = new Cat(); d = ((Dog) a); return d.kind(); } }
        ";
        let (outcome, _out) = run(src, "");
        match outcome {
            Outcome::Abort(AbortKind::CastFailed { from, to }) => {
                assert_eq!(from, "Cat");
                assert_eq!(to, "Dog");
            }
            other => panic!("expected a CastFailed abort, got {other:?}"),
        }
    }

    #[test]
    fn reads_input_and_echoes_a_computation() {
        // Reads two ints and prints their sum.
        let src = "
            class Main () {
                int main() {
                    int a; int b;
                    a = in.read_int(); b = in.read_int();
                    out.print_int((a + b));
                    return 0;
                }
            }
        ";
        let (outcome, out) = run(src, "20 22\n");
        assert_eq!(outcome, Outcome::Exit(0));
        assert_eq!(out, "42");
    }
}
