//! Expression traversal, null-aware compatibility, and operator typing.

use std::collections::HashSet;

use super::super::{CastDirection, ClassKind, ClassTable, ErrorCode, TypeError, TypedExpr};
use super::calls::{check_constructor_call, check_method_call};
use super::{resolve_name, BodyCtx, Scope};
use crate::ast::{Binop, Expr, Type, Unop};

/// Internal comparison currency during checking only — never part of the typed AST.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ExprType {
    Concrete(Type),
    NullLiteral,
}

pub(super) fn typed_of(e: &TypedExpr) -> ExprType {
    match e {
        TypedExpr::Null(_) => ExprType::NullLiteral,
        TypedExpr::Num(..) => ExprType::Concrete(Type::Int),
        TypedExpr::Bool(..) => ExprType::Concrete(Type::Bool),
        TypedExpr::Str(..) => ExprType::Concrete(Type::String),
        TypedExpr::This(c, _) => ExprType::Concrete(Type::Class(c.clone())),
        TypedExpr::Var { binding, .. } => ExprType::Concrete(binding.ty().clone()),
        TypedExpr::New { class, .. } => ExprType::Concrete(Type::Class(class.clone())),
        TypedExpr::Call(call) => ExprType::Concrete(call.return_type.clone()),
        TypedExpr::Ternary { ty: Some(t), .. } => ExprType::Concrete(t.clone()),
        TypedExpr::Ternary { ty: None, .. } => ExprType::NullLiteral,
        TypedExpr::Binop { ty, .. } | TypedExpr::Unop { ty, .. } => ExprType::Concrete(ty.clone()),
        TypedExpr::Cast { target, .. } => ExprType::Concrete(target.clone()),
        TypedExpr::InstanceOf { .. } => ExprType::Concrete(Type::Bool),
    }
}

fn least_common_ancestor(a: &str, b: &str, table: &ClassTable) -> Option<String> {
    let chain_a: HashSet<&str> = std::iter::once(a)
        .chain(table.get(a)?.ancestors.iter().map(String::as_str))
        .collect();
    std::iter::once(b)
        .chain(table.get(b)?.ancestors.iter().map(String::as_str))
        .find(|c| chain_a.contains(c))
        .map(String::from)
}

pub(super) fn expect_bool(
    expr: &Expr,
    scope: &Scope,
    ctx: &BodyCtx,
    table: &ClassTable,
    line: u32,
) -> Result<TypedExpr, TypeError> {
    let typed = check_expr(expr, scope, ctx, table)?;
    match typed_of(&typed) {
        ExprType::Concrete(Type::Bool) => Ok(typed),
        _ => Err(TypeError::new(
            ErrorCode::ETypeMismatch,
            line,
            "condition must be bool",
        )),
    }
}

// ---------------------------------------------------------------------------
// Expression checking
// ---------------------------------------------------------------------------

pub(super) fn check_expr(
    expr: &Expr,
    scope: &Scope,
    ctx: &BodyCtx,
    table: &ClassTable,
) -> Result<TypedExpr, TypeError> {
    Ok(match expr {
        Expr::Num(n, line) => TypedExpr::Num(*n, *line),
        Expr::Bool(b, line) => TypedExpr::Bool(*b, *line),
        Expr::Str(s, line) => TypedExpr::Str(s.clone(), *line),
        Expr::Null(line) => TypedExpr::Null(*line),
        Expr::This(line) => TypedExpr::This(ctx.class_name.to_string(), *line),

        Expr::Var(name, line) => {
            let binding = resolve_name(name, scope, ctx.class_name, table).ok_or_else(|| {
                TypeError::new(
                    ErrorCode::EUnknownVariable,
                    *line,
                    format!("unknown variable '{}'", name),
                )
            })?;
            TypedExpr::Var {
                name: name.clone(),
                binding,
                line: *line,
            }
        }

        Expr::New(class_name, actuals, line) => {
            let info = table.get(class_name).ok_or_else(|| {
                TypeError::new(
                    ErrorCode::EUnknownClass,
                    *line,
                    format!("unknown class '{}'", class_name),
                )
            })?;
            if info.kind == ClassKind::Preamble {
                return Err(TypeError::new(
                    ErrorCode::ETypeCheckOther,
                    *line,
                    format!("'{}' cannot be instantiated directly", class_name),
                ));
            }
            let typed_actuals = check_constructor_call(
                &info.own_constructors,
                actuals,
                scope,
                ctx,
                table,
                *line,
                class_name,
            )?;
            TypedExpr::New {
                class: class_name.clone(),
                actuals: typed_actuals,
                line: *line,
            }
        }

        Expr::Call(call) => {
            let typed_call = check_method_call(call, scope, ctx, table)?;
            if typed_call.return_type == Type::Void {
                return Err(TypeError::new(
                    ErrorCode::EVoidCallInExpression,
                    call.line,
                    format!(
                        "void call to '{}' used in expression position",
                        call.method_name
                    ),
                ));
            }
            TypedExpr::Call(typed_call)
        }

        Expr::Ternary(cond, then_e, else_e, line) => {
            let typed_cond = expect_bool(cond, scope, ctx, table, *line)?;
            let typed_then = check_expr(then_e, scope, ctx, table)?;
            let typed_else = check_expr(else_e, scope, ctx, table)?;
            let combined = combine_ternary_branches(
                &typed_of(&typed_then),
                &typed_of(&typed_else),
                table,
                *line,
            )?;
            let ty = match combined {
                ExprType::Concrete(t) => Some(t),
                ExprType::NullLiteral => None,
            };
            TypedExpr::Ternary {
                cond: Box::new(typed_cond),
                then_branch: Box::new(typed_then),
                else_branch: Box::new(typed_else),
                ty,
                line: *line,
            }
        }

        Expr::Binop(lhs, op, rhs, line) => {
            let typed_lhs = check_expr(lhs, scope, ctx, table)?;
            let typed_rhs = check_expr(rhs, scope, ctx, table)?;
            let ty = check_binop(*op, &typed_of(&typed_lhs), &typed_of(&typed_rhs), *line)?;
            TypedExpr::Binop {
                lhs: Box::new(typed_lhs),
                op: *op,
                rhs: Box::new(typed_rhs),
                ty,
                line: *line,
            }
        }

        Expr::Unop(op, operand, line) => {
            let typed_operand = check_expr(operand, scope, ctx, table)?;
            let ty = check_unop(*op, &typed_of(&typed_operand), *line)?;
            TypedExpr::Unop {
                op: *op,
                operand: Box::new(typed_operand),
                ty,
                line: *line,
            }
        }

        Expr::Cast(target, operand, line) => {
            let Type::Class(target_name) = target else {
                return Err(TypeError::new(
                    ErrorCode::ECastTargetNotClass,
                    *line,
                    "cast target must be a class type",
                ));
            };
            if !table.class_exists(target_name) {
                return Err(TypeError::new(
                    ErrorCode::EUnknownClass,
                    *line,
                    format!("unknown class '{}'", target_name),
                ));
            }
            let typed_operand = check_expr(operand, scope, ctx, table)?;
            let direction = match typed_of(&typed_operand) {
                ExprType::NullLiteral => CastDirection::Null,
                ExprType::Concrete(Type::Class(source_name)) => {
                    if table.is_subtype(&source_name, target_name) {
                        CastDirection::Upcast
                    } else if table.is_subtype(target_name, &source_name) {
                        CastDirection::Downcast
                    } else {
                        return Err(TypeError::new(
                            ErrorCode::ECastUnrelatedTypes,
                            *line,
                            format!(
                                "cannot cast '{}' to unrelated class '{}'",
                                source_name, target_name
                            ),
                        ));
                    }
                }
                _ => {
                    return Err(TypeError::new(
                        ErrorCode::ECastSourceNotClass,
                        *line,
                        "cast source must be a class-typed expression",
                    ))
                }
            };
            TypedExpr::Cast {
                target: target.clone(),
                operand: Box::new(typed_operand),
                direction,
                line: *line,
            }
        }

        Expr::InstanceOf(operand, class_name, line) => {
            if !table.class_exists(class_name) {
                return Err(TypeError::new(
                    ErrorCode::EUnknownClass,
                    *line,
                    format!("unknown class '{}'", class_name),
                ));
            }
            let typed_operand = check_expr(operand, scope, ctx, table)?;
            match typed_of(&typed_operand) {
                // null is a legal instanceof source -- always false at runtime.
                ExprType::Concrete(Type::Class(_)) | ExprType::NullLiteral => {}
                _ => {
                    return Err(TypeError::new(
                        ErrorCode::EInstanceofSourceNotClass,
                        *line,
                        "instanceof source must be a class-typed expression",
                    ))
                }
            }
            TypedExpr::InstanceOf {
                operand: Box::new(typed_operand),
                class: class_name.clone(),
                line: *line,
            }
        }
    })
}

pub(super) fn assignment_compatible(from: &ExprType, to: &Type, table: &ClassTable) -> bool {
    match from {
        ExprType::NullLiteral => matches!(to, Type::Class(_)),
        ExprType::Concrete(Type::Class(a)) => {
            matches!(to, Type::Class(b) if table.is_subtype(a, b))
        }
        ExprType::Concrete(t) => t == to,
    }
}

/// The domain here is `Type` or `Null`, not just `Type`: `x ? null : null` is
/// legal — its type is deferred to whatever context the ternary itself sits
/// in (an assignment target, a cast, an enclosing ternary), exactly like a
/// bare `null` literal.
fn combine_ternary_branches(
    a: &ExprType,
    b: &ExprType,
    table: &ClassTable,
    line: u32,
) -> Result<ExprType, TypeError> {
    use ExprType::*;
    let mismatch = || {
        TypeError::new(
            ErrorCode::EConditionalTypeMismatch,
            line,
            "ternary branches have incompatible types",
        )
    };
    match (a, b) {
        (NullLiteral, NullLiteral) => Ok(NullLiteral),
        (NullLiteral, Concrete(t @ Type::Class(_)))
        | (Concrete(t @ Type::Class(_)), NullLiteral) => Ok(Concrete(t.clone())),
        (Concrete(t1), Concrete(t2)) if t1 == t2 => Ok(Concrete(t1.clone())),
        (Concrete(Type::Class(c1)), Concrete(Type::Class(c2))) => {
            least_common_ancestor(c1, c2, table)
                .map(|c| Concrete(Type::Class(c)))
                .ok_or_else(mismatch)
        }
        _ => Err(mismatch()),
    }
}

fn check_binop(op: Binop, lhs: &ExprType, rhs: &ExprType, line: u32) -> Result<Type, TypeError> {
    use Binop::*;
    use ExprType::*;
    if op == Eq
        && matches!(
            (lhs, rhs),
            (Concrete(Type::Class(_)), Concrete(Type::Class(_)))
                | (Concrete(Type::Class(_)), NullLiteral)
                | (NullLiteral, Concrete(Type::Class(_)))
                | (NullLiteral, NullLiteral)
        )
    {
        return Ok(Type::Bool);
    }
    let (ExprType::Concrete(l), ExprType::Concrete(r)) = (lhs, rhs) else {
        return Err(TypeError::new(
            ErrorCode::EBinopTypeMismatch,
            line,
            "null is not a legal operand",
        ));
    };
    let mismatch = || TypeError::new(ErrorCode::EBinopTypeMismatch, line, "operand type mismatch");
    match op {
        Add | Sub | Mul | Div | Mod => {
            if l == &Type::Int && r == &Type::Int {
                Ok(Type::Int)
            } else if op == Add && l == &Type::String && r == &Type::String {
                Ok(Type::String)
            } else if op == Mul && l == &Type::String && r == &Type::Int {
                Ok(Type::String)
            } else {
                Err(mismatch())
            }
        }
        And | Or => {
            if l == &Type::Bool && r == &Type::Bool {
                Ok(Type::Bool)
            } else {
                Err(mismatch())
            }
        }
        Lt | Gt | Eq => {
            if l == r && matches!(l, Type::Int | Type::String) {
                Ok(Type::Bool)
            } else {
                Err(mismatch())
            }
        }
    }
}

fn check_unop(op: Unop, operand: &ExprType, line: u32) -> Result<Type, TypeError> {
    let ExprType::Concrete(t) = operand else {
        return Err(TypeError::new(
            ErrorCode::EUnopTypeMismatch,
            line,
            "null is not a legal operand",
        ));
    };
    match (op, t) {
        (Unop::Not, Type::Bool) => Ok(Type::Bool),
        (Unop::Neg, Type::Int) => Ok(Type::Int),
        (Unop::Neg, Type::String) => Ok(Type::String), // `~` also means string reversal
        _ => Err(TypeError::new(
            ErrorCode::EUnopTypeMismatch,
            line,
            "operand type mismatch",
        )),
    }
}
