//! Pass 4: body checking.

mod calls;
mod expressions;

use std::collections::{HashMap, HashSet};

use super::class_table::check_type_reference;
use super::declarations::check_not_reserved_var_name;
use super::{
    BindingInfo, ClassTable, ErrorCode, TypeError, TypedClassDecl, TypedConstructor,
    TypedMethodBody, TypedMethodDecl, TypedProgram, TypedStmt,
};
use crate::ast::{Formal, MethodBody, Program, Stmt, Type, VarDecl};
use calls::{check_delegation, check_delegation_cycle, check_method_call};
use expressions::{assignment_compatible, check_expr, expect_bool, typed_of};

fn preamble_binding(name: &str) -> Option<Type> {
    match name {
        "in" => Some(Type::Class("Input".to_string())),
        "out" | "err" => Some(Type::Class("Output".to_string())),
        _ => None,
    }
}

struct Scope {
    bindings: HashMap<String, Type>,
    formal_names: HashSet<String>,
}

impl Scope {
    fn build(
        formals: &[Formal],
        locals: &[VarDecl],
        table: &ClassTable,
    ) -> Result<Scope, TypeError> {
        let mut bindings = HashMap::new();
        let mut formal_names = HashSet::new();
        for f in formals {
            bindings.insert(f.identifier.clone(), f.declared_type.clone());
            formal_names.insert(f.identifier.clone());
        }
        for decl in locals {
            if decl.declared_type == Type::Void {
                return Err(TypeError::new(
                    ErrorCode::EWellFormednessOther,
                    decl.line,
                    "a local variable cannot have type void",
                ));
            }
            check_type_reference(&decl.declared_type, table, decl.line, "local declaration")?;
            for name in &decl.identifiers {
                check_not_reserved_var_name(name, decl.line)?;
                if formal_names.contains(name) {
                    return Err(TypeError::new(
                        ErrorCode::ELocalShadowsFormal,
                        decl.line,
                        format!("local '{}' has the same name as a formal parameter", name),
                    ));
                }
                bindings.insert(name.clone(), decl.declared_type.clone());
            }
        }
        Ok(Scope {
            bindings,
            formal_names,
        })
    }
}

fn resolve_name(
    name: &str,
    scope: &Scope,
    class_name: &str,
    table: &ClassTable,
) -> Option<BindingInfo> {
    if let Some(t) = scope.bindings.get(name) {
        return Some(if scope.formal_names.contains(name) {
            BindingInfo::Formal(t.clone())
        } else {
            BindingInfo::Local(t.clone())
        });
    }
    let info = table.get(class_name)?;
    if let Some(field) = info.effective_fields.iter().find(|f| f.name == name) {
        return Some(BindingInfo::Field {
            owner: field.owner.clone(),
            ty: field.ty.clone(),
        });
    }
    preamble_binding(name).map(BindingInfo::Prebound)
}

#[derive(Clone, Copy)]
struct BodyCtx<'a> {
    class_name: &'a str,
    return_type: &'a Type,
    in_loop: bool,
    in_constructor: bool,
}

fn flatten_locals(locals: &[VarDecl]) -> Vec<(String, Type)> {
    locals
        .iter()
        .flat_map(|d| {
            d.identifiers
                .iter()
                .map(move |n| (n.clone(), d.declared_type.clone()))
        })
        .collect()
}

fn definitely_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_returns)
}

fn stmt_returns(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(..) => true,
        Stmt::If(_, then_b, else_b, _) => definitely_returns(then_b) && definitely_returns(else_b),
        Stmt::While(..) => false,
        _ => false,
    }
}

pub(super) fn check_bodies(
    program: &Program,
    table: &ClassTable,
) -> Result<TypedProgram, TypeError> {
    let mut typed_classes = Vec::with_capacity(program.classes.len());

    for class in &program.classes {
        let info = table.get(&class.class_name).unwrap();

        let mut typed_methods = Vec::with_capacity(class.methods.len());
        for method in &class.methods {
            let body = match &method.body {
                MethodBody::Io(op) => {
                    typed_methods.push(TypedMethodDecl {
                        method_name: method.method_name.clone(),
                        return_type: method.return_type.clone(),
                        formals: method
                            .formals
                            .iter()
                            .map(|f| (f.identifier.clone(), f.declared_type.clone()))
                            .collect(),
                        body: TypedMethodBody::Io(op.clone()),
                        line: method.line,
                    });
                    continue;
                }
                MethodBody::UserDefined(b) => b,
            };
            let scope = Scope::build(&method.formals, &body.locals, table)?;
            let ctx = BodyCtx {
                class_name: &class.class_name,
                return_type: &method.return_type,
                in_loop: false,
                in_constructor: false,
            };
            let mut typed_stmts = Vec::with_capacity(body.stmts.len());
            for stmt in &body.stmts {
                typed_stmts.push(check_stmt(stmt, &scope, &ctx, table)?);
            }
            if method.return_type != Type::Void && !definitely_returns(&body.stmts) {
                return Err(TypeError::new(
                    ErrorCode::EReturnMissing,
                    method.line,
                    format!("'{}' does not return on every path", method.method_name),
                ));
            }
            typed_methods.push(TypedMethodDecl {
                method_name: method.method_name.clone(),
                return_type: method.return_type.clone(),
                formals: method
                    .formals
                    .iter()
                    .map(|f| (f.identifier.clone(), f.declared_type.clone()))
                    .collect(),
                body: TypedMethodBody::UserDefined {
                    locals: flatten_locals(&body.locals),
                    stmts: typed_stmts,
                },
                line: method.line,
            });
        }

        let typed_constructors = if class.constructors.is_empty() {
            // Only reachable when extends.is_none() — resolve_inheritance already
            // rejected an inheriting class with no explicit constructor section.
            let fields = info
                .own_fields
                .iter()
                .map(|(n, t, _)| (n.clone(), t.clone()))
                .collect();
            vec![TypedConstructor::Implicit { fields }]
        } else {
            check_delegation_cycle(&class.class_name, table)?;
            let mut typed_ctors = Vec::with_capacity(class.constructors.len());
            for ctor in &class.constructors {
                if ctor.delegation.is_none()
                    && ctor.body.stmts.is_empty()
                    && ctor.body.locals.is_empty()
                {
                    return Err(TypeError::new(ErrorCode::EWellFormednessOther, ctor.line,
                        "constructor body must contain a delegation, a declaration, or at least one statement"));
                }
                let scope = Scope::build(&ctor.formals, &ctor.body.locals, table)?;
                let typed_delegation = check_delegation(ctor, &class.class_name, &scope, table)?;
                let ctx = BodyCtx {
                    class_name: &class.class_name,
                    return_type: &Type::Void,
                    in_loop: false,
                    in_constructor: true,
                };
                let mut typed_stmts = Vec::with_capacity(ctor.body.stmts.len());
                for stmt in &ctor.body.stmts {
                    typed_stmts.push(check_stmt(stmt, &scope, &ctx, table)?);
                }
                typed_ctors.push(TypedConstructor::Explicit {
                    formals: ctor
                        .formals
                        .iter()
                        .map(|f| (f.identifier.clone(), f.declared_type.clone()))
                        .collect(),
                    delegation: typed_delegation,
                    locals: flatten_locals(&ctor.body.locals),
                    stmts: typed_stmts,
                    line: ctor.line,
                });
            }
            typed_ctors
        };

        typed_classes.push(TypedClassDecl {
            class_name: class.class_name.clone(),
            kind: info.kind,
            extends: class.extends.clone(),
            fields: class
                .fields
                .iter()
                .flat_map(|d| {
                    d.identifiers
                        .iter()
                        .map(move |n| (n.clone(), d.declared_type.clone()))
                })
                .collect(),
            constructors: typed_constructors,
            methods: typed_methods,
            line: class.line,
        });
    }

    Ok(TypedProgram {
        classes: typed_classes,
    })
}

// ---------------------------------------------------------------------------
// Statement checking
// ---------------------------------------------------------------------------

fn check_stmt(
    stmt: &Stmt,
    scope: &Scope,
    ctx: &BodyCtx,
    table: &ClassTable,
) -> Result<TypedStmt, TypeError> {
    Ok(match stmt {
        Stmt::Empty(line) => TypedStmt::Empty(*line),

        Stmt::Assign(name, expr, line) => {
            let binding = resolve_name(name, scope, ctx.class_name, table).ok_or_else(|| {
                TypeError::new(
                    ErrorCode::EUnknownVariable,
                    *line,
                    format!("unknown variable '{}'", name),
                )
            })?;
            let typed_value = check_expr(expr, scope, ctx, table)?;
            if !assignment_compatible(&typed_of(&typed_value), binding.ty(), table) {
                return Err(TypeError::new(
                    ErrorCode::EAssignTypeMismatch,
                    *line,
                    format!("cannot assign to '{}'", name),
                ));
            }
            TypedStmt::Assign {
                target: name.clone(),
                binding,
                value: typed_value,
                line: *line,
            }
        }

        Stmt::Return(expr, line) => {
            if ctx.in_constructor {
                return Err(TypeError::new(
                    ErrorCode::EReturnInConstructor,
                    *line,
                    "constructors may not contain a return statement",
                ));
            }
            if *ctx.return_type == Type::Void {
                return Err(TypeError::new(
                    ErrorCode::EReturnInVoidMethod,
                    *line,
                    "a void method may not contain a return statement",
                ));
            }
            let typed_value = check_expr(expr, scope, ctx, table)?;
            if !assignment_compatible(&typed_of(&typed_value), ctx.return_type, table) {
                return Err(TypeError::new(
                    ErrorCode::EReturnTypeMismatch,
                    *line,
                    "returned expression's type does not match the declared return type",
                ));
            }
            TypedStmt::Return(typed_value, *line)
        }

        Stmt::If(cond, then_b, else_b, line) => {
            let typed_cond = expect_bool(cond, scope, ctx, table, *line)?;
            let typed_then = then_b
                .iter()
                .map(|s| check_stmt(s, scope, ctx, table))
                .collect::<Result<_, _>>()?;
            let typed_else = else_b
                .iter()
                .map(|s| check_stmt(s, scope, ctx, table))
                .collect::<Result<_, _>>()?;
            TypedStmt::If(typed_cond, typed_then, typed_else, *line)
        }

        Stmt::While(cond, body, line) => {
            let typed_cond = expect_bool(cond, scope, ctx, table, *line)?;
            let inner_ctx = BodyCtx {
                in_loop: true,
                ..*ctx
            };
            let typed_body = body
                .iter()
                .map(|s| check_stmt(s, scope, &inner_ctx, table))
                .collect::<Result<_, _>>()?;
            TypedStmt::While(typed_cond, typed_body, *line)
        }

        Stmt::Break(line) => {
            if !ctx.in_loop {
                return Err(TypeError::new(
                    ErrorCode::EBreakOutsideLoop,
                    *line,
                    "'break' outside an enclosing while loop",
                ));
            }
            TypedStmt::Break(*line)
        }

        Stmt::CallStmt(call) => {
            let typed_call = check_method_call(call, scope, ctx, table)?;
            if typed_call.return_type != Type::Void {
                return Err(TypeError::new(
                    ErrorCode::ENonvoidCallAsStatement,
                    call.line,
                    format!(
                        "result of non-void call to '{}' is discarded",
                        call.method_name
                    ),
                ));
            }
            TypedStmt::CallStmt(typed_call)
        }
    })
}
