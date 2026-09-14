// Four passes: declarations, inheritance, entry point, then bodies.

mod bodies;
mod class_table;
mod declarations;
mod entry_point;
mod errors;
mod inheritance;
mod typed_ast;

pub use class_table::{ClassInfo, ClassTable, ConstructorSig, FieldInfo, MethodEntry, MethodSig};
pub use errors::{ErrorCode, TypeError};
pub use typed_ast::{
    BindingInfo, CastDirection, ClassKind, MethodResolution, TypedClassDecl, TypedConstructor,
    TypedDelegation, TypedExpr, TypedMethodBody, TypedMethodCall, TypedMethodDecl, TypedObjName,
    TypedProgram, TypedStmt,
};

use crate::ast::Program;
use bodies::check_bodies;
use declarations::{gather_declarations, validate_user_declared_names};
use entry_point::check_entry_point;
use inheritance::resolve_inheritance;

pub fn check_program(program: Program) -> Result<(TypedProgram, ClassTable), TypeError> {
    validate_user_declared_names(&program)?;
    let program = crate::add_io_classes::add_io_classes(program);
    let mut table = gather_declarations(&program)?;
    resolve_inheritance(&mut table)?;
    check_entry_point(&table)?;
    let typed = check_bodies(&program, &table)?;
    Ok((typed, table))
}
