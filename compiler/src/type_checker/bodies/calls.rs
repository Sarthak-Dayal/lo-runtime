//! Method/constructor calls, receivers, and constructor delegation.

use super::super::class_table::find_cycle;
use super::super::{
    ClassTable, ConstructorSig, ErrorCode, MethodEntry, MethodResolution, TypeError,
    TypedDelegation, TypedExpr, TypedMethodCall, TypedObjName,
};
use super::expressions::{assignment_compatible, check_expr, typed_of, ExprType};
use super::{resolve_name, BodyCtx, Scope};
use crate::ast::{ConstructorDecl, ConstructorDelegation, Expr, MethodCall, ObjName, Type};

struct ReceiverResolution {
    typed: TypedObjName,
    /// Irrelevant/empty when `is_super` is true.
    search_class: String,
    is_super: bool,
}

fn resolve_receiver(
    obj_name: &ObjName,
    scope: &Scope,
    ctx: &BodyCtx,
    table: &ClassTable,
) -> Result<ReceiverResolution, TypeError> {
    match obj_name {
        ObjName::This(line) => Ok(ReceiverResolution {
            typed: TypedObjName::This(ctx.class_name.to_string(), *line),
            search_class: ctx.class_name.to_string(),
            is_super: false,
        }),

        ObjName::Super(line) => {
            // super.method() is a method-body form; constructor delegation has
            // its own dedicated form, super(...) (ConstructorDelegation::SuperCall).
            if ctx.in_constructor {
                return Err(TypeError::new(
                    ErrorCode::EInheritanceCheckOther,
                    *line,
                    "super.method() is a method-body form and may not appear in a constructor body",
                ));
            }
            if table
                .get(ctx.class_name)
                .and_then(|i| i.parent.as_ref())
                .is_none()
            {
                return Err(TypeError::new(
                    ErrorCode::ESuperMethodInRootClass,
                    *line,
                    "'super' used in a class with no parent",
                ));
            }
            Ok(ReceiverResolution {
                typed: TypedObjName::Super(*line),
                search_class: String::new(),
                is_super: true,
            })
        }

        ObjName::Var(name, line) => {
            let binding = resolve_name(name, scope, ctx.class_name, table).ok_or_else(|| {
                TypeError::new(
                    ErrorCode::EUnknownVariable,
                    *line,
                    format!("unknown variable '{}'", name),
                )
            })?;
            let Type::Class(c) = binding.ty().clone() else {
                return Err(TypeError::new(
                    ErrorCode::EReceiverNotClassType,
                    *line,
                    "receiver is not class-typed",
                ));
            };
            Ok(ReceiverResolution {
                typed: TypedObjName::Var {
                    name: name.clone(),
                    binding,
                    line: *line,
                },
                search_class: c,
                is_super: false,
            })
        }

        ObjName::Computed(expr, line) => {
            if matches!(**expr, Expr::Null(_)) {
                return Err(TypeError::new(
                    ErrorCode::ENullLiteralReceiver,
                    *line,
                    "receiver cannot be the literal 'null'",
                ));
            }
            let typed_expr = check_expr(expr, scope, ctx, table)?;
            match typed_of(&typed_expr) {
                ExprType::Concrete(Type::Class(c)) => Ok(ReceiverResolution {
                    typed: TypedObjName::Computed(Box::new(typed_expr), *line),
                    search_class: c,
                    is_super: false,
                }),
                _ => Err(TypeError::new(
                    ErrorCode::EReceiverNotClassType,
                    *line,
                    "receiver is not class-typed",
                )),
            }
        }
    }
}

pub(super) fn check_method_call(
    call: &MethodCall,
    scope: &Scope,
    ctx: &BodyCtx,
    table: &ClassTable,
) -> Result<TypedMethodCall, TypeError> {
    let r = resolve_receiver(&call.obj_name, scope, ctx, table)?;

    let entry = if r.is_super {
        let parent = table
            .get(ctx.class_name)
            .and_then(|i| i.parent.as_deref())
            .unwrap();
        table
            .get(parent)
            .unwrap()
            .effective_methods
            .get(&call.method_name)
            .cloned()
            .ok_or_else(|| {
                TypeError::new(
                    ErrorCode::ESuperMethodUnresolved,
                    call.line,
                    format!("no ancestor declares method '{}'", call.method_name),
                )
            })?
    } else {
        let info = table.get(&r.search_class).unwrap();
        info.effective_methods
            .get(&call.method_name)
            .cloned()
            .ok_or_else(|| {
                TypeError::new(
                    ErrorCode::EUnknownMethod,
                    call.line,
                    format!(
                        "unknown method '{}' on class '{}'",
                        call.method_name, r.search_class
                    ),
                )
            })?
    };
    let MethodEntry { owner, sig } = entry;

    let resolution = if r.is_super {
        MethodResolution::Super {
            declaring_class: owner,
        }
    } else if let Some(op) = &sig.is_io {
        MethodResolution::Io { op: op.clone() }
    } else {
        MethodResolution::Virtual {
            static_class: r.search_class.clone(),
        }
    };

    let typed_actuals = check_actuals(
        &sig.params,
        &call.actuals,
        scope,
        ctx,
        table,
        call.line,
        &sig.method_name,
    )?;
    Ok(TypedMethodCall {
        obj_name: r.typed,
        method_name: call.method_name.clone(),
        resolution,
        actuals: typed_actuals,
        return_type: sig.return_type,
        line: call.line,
    })
}

fn check_actuals(
    formal_types: &[Type],
    actuals: &[Expr],
    scope: &Scope,
    ctx: &BodyCtx,
    table: &ClassTable,
    line: u32,
    what: &str,
) -> Result<Vec<TypedExpr>, TypeError> {
    if formal_types.len() != actuals.len() {
        return Err(TypeError::new(
            ErrorCode::EArityMismatch,
            line,
            format!(
                "'{}' expects {} argument(s), got {}",
                what,
                formal_types.len(),
                actuals.len()
            ),
        ));
    }
    let mut typed_args = Vec::with_capacity(actuals.len());
    for (formal_ty, actual) in formal_types.iter().zip(actuals) {
        let typed_arg = check_expr(actual, scope, ctx, table)?;
        if !assignment_compatible(&typed_of(&typed_arg), formal_ty, table) {
            return Err(TypeError::new(
                ErrorCode::EActualTypeMismatch,
                line,
                format!(
                    "argument type does not match formal type in call to '{}'",
                    what
                ),
            ));
        }
        typed_args.push(typed_arg);
    }
    Ok(typed_args)
}

pub(super) fn check_constructor_call(
    ctors: &[ConstructorSig],
    actuals: &[Expr],
    scope: &Scope,
    ctx: &BodyCtx,
    table: &ClassTable,
    line: u32,
    class_name: &str,
) -> Result<Vec<TypedExpr>, TypeError> {
    let ctor = ctors
        .iter()
        .find(|c| c.arity == actuals.len())
        .ok_or_else(|| {
            TypeError::new(
                ErrorCode::EArityMismatch,
                line,
                format!(
                    "no constructor of class '{}' takes {} argument(s)",
                    class_name,
                    actuals.len()
                ),
            )
        })?;
    check_actuals(&ctor.params, actuals, scope, ctx, table, line, class_name)
}

// ---------------------------------------------------------------------------
// Constructor delegation
// ---------------------------------------------------------------------------

pub(super) fn check_delegation_cycle(
    class_name: &str,
    table: &ClassTable,
) -> Result<(), TypeError> {
    let info = table.get(class_name).unwrap();
    let arities: Vec<usize> = info.own_constructors.iter().map(|c| c.arity).collect();
    let this_edges = |arity: &usize| -> Vec<usize> {
        info.own_constructors
            .iter()
            .find(|c| c.arity == *arity)
            .and_then(|c| c.this_target_arity)
            .into_iter()
            .collect()
    };
    if find_cycle(arities.iter(), this_edges).is_some() {
        let line = info
            .own_constructors
            .iter()
            .find(|c| c.this_target_arity.is_some())
            .map(|c| c.line)
            .unwrap_or(info.decl_line);
        return Err(TypeError::new(
            ErrorCode::EDelegationCycle,
            line,
            format!(
                "constructor delegation in class '{}' forms a cycle",
                class_name
            ),
        ));
    }
    Ok(())
}

pub(super) fn check_delegation(
    ctor: &ConstructorDecl,
    class_name: &str,
    scope: &Scope,
    table: &ClassTable,
) -> Result<Option<TypedDelegation>, TypeError> {
    let info = table.get(class_name).unwrap();
    let inheriting = info.parent.is_some();

    match &ctor.delegation {
        None => {
            if inheriting {
                return Err(TypeError::new(
                    ErrorCode::EInheritanceCheckOther,
                    ctor.line,
                    "constructor of an inheriting class must start with super(...) or this(...)",
                ));
            }
            Ok(None)
        }
        Some(ConstructorDelegation::SuperCall(args, line)) => {
            let parent = info.parent.as_ref().ok_or_else(|| {
                TypeError::new(
                    ErrorCode::ESuperInRootClass,
                    *line,
                    "super(...) used in a class with no parent",
                )
            })?;
            let parent_info = table.get(parent).unwrap();
            let target = parent_info
                .own_constructors
                .iter()
                .find(|c| c.arity == args.len())
                .ok_or_else(|| {
                    TypeError::new(
                        ErrorCode::EDelegationArityMismatch,
                        *line,
                        format!("'{}' has no constructor of arity {}", parent, args.len()),
                    )
                })?;
            let ctx = BodyCtx {
                class_name,
                return_type: &Type::Void,
                in_loop: false,
                in_constructor: true,
            };
            let typed_actuals =
                check_actuals(&target.params, args, scope, &ctx, table, *line, parent)?;
            Ok(Some(TypedDelegation::SuperCall {
                actuals: typed_actuals,
                line: *line,
            }))
        }
        Some(ConstructorDelegation::ThisCall(args, line)) => {
            let target = info
                .own_constructors
                .iter()
                .find(|c| c.arity == args.len())
                .ok_or_else(|| {
                    TypeError::new(
                        ErrorCode::EDelegationArityMismatch,
                        *line,
                        format!(
                            "class '{}' has no constructor of arity {}",
                            class_name,
                            args.len()
                        ),
                    )
                })?;
            let ctx = BodyCtx {
                class_name,
                return_type: &Type::Void,
                in_loop: false,
                in_constructor: true,
            };
            let typed_actuals =
                check_actuals(&target.params, args, scope, &ctx, table, *line, class_name)?;
            Ok(Some(TypedDelegation::ThisCall {
                actuals: typed_actuals,
                line: *line,
            }))
        }
    }
}
