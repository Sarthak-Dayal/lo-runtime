//! Pass 3: entry-point checks.

use super::{ClassTable, ErrorCode, TypeError};
use crate::ast::Type;

pub(super) fn check_entry_point(table: &ClassTable) -> Result<(), TypeError> {
    let main = table.get("Main").ok_or_else(|| {
        TypeError::new(ErrorCode::ENoMainClass, 0, "no class named 'Main' declared")
    })?;

    if main.parent.is_some() {
        return Err(TypeError::new(
            ErrorCode::EMainClassExtends,
            main.decl_line,
            "'Main' must not have an extends clause",
        ));
    }

    match main.own_methods.iter().find(|m| m.method_name == "main") {
        None => {
            return Err(TypeError::new(
                ErrorCode::ENoMainMethod,
                main.decl_line,
                "'Main' must declare 'int main()'",
            ))
        }
        Some(m) if m.return_type != Type::Int || !m.params.is_empty() => {
            return Err(TypeError::new(
                ErrorCode::EMainMethodSignature,
                m.line,
                "'main' must return int and take no formals",
            ));
        }
        _ => {}
    }

    if !main.own_constructors.iter().any(|c| c.arity == 0) {
        return Err(TypeError::new(
            ErrorCode::EMainNoZeroArgConstructor,
            main.decl_line,
            "'Main' must have a zero-arg constructor",
        ));
    }

    Ok(())
}
