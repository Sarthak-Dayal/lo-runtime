//! Shared test helpers.

use crate::type_checker::ClassTable;

/// Runs the front end on `source` and returns its class table, preamble
/// included. Panics with the stage's message if the program is rejected.
pub fn check(source: &str) -> ClassTable {
    let tokens = crate::lexer::tokenize(source).expect("test program should lex");
    let program = crate::parser::parse_program(&tokens)
        .unwrap_or_else(|e| panic!("test program should parse: {}", e.message));
    crate::type_checker::check_program(program)
        .unwrap_or_else(|e| panic!("test program should type-check: {}", e.message))
        .1
}
