//! Pass 1: declaration gathering and well-formedness checks.

use std::collections::{HashMap, HashSet};

use super::{ClassInfo, ClassKind, ClassTable, ConstructorSig, ErrorCode, MethodSig, TypeError};
use crate::ast::{ConstructorDelegation, Formal, MethodBody, Program, Type};

/// Runs before preamble injection, while every declaration in `program` is
/// user-written. Keeping the runtime-prefix rule here means synthetic I/O and
/// other compiler/runtime declarations never pass through it.
pub(super) fn validate_user_declared_names(program: &Program) -> Result<(), TypeError> {
    for class in &program.classes {
        if crate::add_io_classes::CLASS_NAMES.contains(&class.class_name.as_str()) {
            return Err(TypeError::new(
                ErrorCode::EReservedClassName,
                class.line,
                format!("class name '{}' is reserved", class.class_name),
            ));
        }
        for method in &class.methods {
            if method.method_name.starts_with("lo_") {
                return Err(TypeError::new(
                    ErrorCode::EReservedVariableName,
                    method.line,
                    format!(
                        "method name '{}' uses the reserved runtime prefix 'lo_'",
                        method.method_name
                    ),
                ));
            }
        }
    }
    Ok(())
}

const RESERVED_VAR_NAMES: &[&str] = &["in", "out", "err"];

pub(super) fn check_not_reserved_var_name(name: &str, line: u32) -> Result<(), TypeError> {
    if RESERVED_VAR_NAMES.contains(&name) {
        return Err(TypeError::new(
            ErrorCode::EReservedVariableName,
            line,
            format!("'{}' is a reserved name", name),
        ));
    }
    Ok(())
}

fn check_formals_well_formed(formals: &[Formal]) -> Result<(), TypeError> {
    let mut seen = HashSet::new();
    for f in formals {
        if f.declared_type == Type::Void {
            return Err(TypeError::new(
                ErrorCode::EFormalTypedVoid,
                f.line,
                format!("formal '{}' cannot have type void", f.identifier),
            ));
        }
        check_not_reserved_var_name(&f.identifier, f.line)?;
        if !seen.insert(f.identifier.clone()) {
            return Err(TypeError::new(
                ErrorCode::EWellFormednessOther,
                f.line,
                format!("duplicate formal parameter '{}'", f.identifier),
            ));
        }
    }
    Ok(())
}

pub(super) fn gather_declarations(program: &Program) -> Result<ClassTable, TypeError> {
    let mut classes = HashMap::new();
    let mut order = Vec::new();

    for class in &program.classes {
        if classes.contains_key(&class.class_name) {
            return Err(TypeError::new(
                ErrorCode::EDuplicateClassName,
                class.line,
                format!("class '{}' declared more than once", class.class_name),
            ));
        }

        // class.fields is Vec<VarDecl> — grouped by type ("int a, b;" is one
        // VarDecl naming two fields) — flatten to one entry per name.
        let mut own_fields = Vec::new();
        for decl in &class.fields {
            if decl.declared_type == Type::Void {
                return Err(TypeError::new(
                    ErrorCode::EFieldTypedVoid,
                    decl.line,
                    "a field cannot have type void",
                ));
            }
            for name in &decl.identifiers {
                check_not_reserved_var_name(name, decl.line)?;
                if own_fields
                    .iter()
                    .any(|(n, ..): &(String, Type, u32)| n == name)
                {
                    return Err(TypeError::new(
                        ErrorCode::EDuplicateField,
                        decl.line,
                        format!("duplicate field '{}'", name),
                    ));
                }
                own_fields.push((name.clone(), decl.declared_type.clone(), decl.line));
            }
        }

        let mut own_methods: Vec<MethodSig> = Vec::new();
        for method in &class.methods {
            if own_methods
                .iter()
                .any(|m| m.method_name == method.method_name)
            {
                return Err(TypeError::new(
                    ErrorCode::EDuplicateMethod,
                    method.line,
                    format!("duplicate method '{}'", method.method_name),
                ));
            }
            check_formals_well_formed(&method.formals)?;
            let is_io = match &method.body {
                MethodBody::Io(op) => Some(op.clone()),
                MethodBody::UserDefined(_) => None,
            };
            own_methods.push(MethodSig {
                method_name: method.method_name.clone(),
                params: method
                    .formals
                    .iter()
                    .map(|f| f.declared_type.clone())
                    .collect(),
                return_type: method.return_type.clone(),
                line: method.line,
                is_io,
            });
        }

        let mut own_constructors: Vec<ConstructorSig> = Vec::new();
        for ctor in &class.constructors {
            let arity = ctor.formals.len();
            if own_constructors.iter().any(|c| c.arity == arity) {
                return Err(TypeError::new(
                    ErrorCode::EDuplicateConstructorArity,
                    ctor.line,
                    format!(
                        "class '{}' already has a constructor of arity {}",
                        class.class_name, arity
                    ),
                ));
            }
            check_formals_well_formed(&ctor.formals)?;
            let this_target_arity = match &ctor.delegation {
                Some(ConstructorDelegation::ThisCall(args, _)) => Some(args.len()),
                _ => None,
            };
            own_constructors.push(ConstructorSig {
                arity,
                params: ctor
                    .formals
                    .iter()
                    .map(|f| f.declared_type.clone())
                    .collect(),
                this_target_arity,
                line: ctor.line,
            });
        }
        // Implicit constructor: only for a root (non-extending) class with no
        // explicit constructor section. An inheriting class with none is
        // E_MISSING_CONSTRUCTOR_IN_INHERITING_CLASS, checked in resolve_inheritance.
        if own_constructors.is_empty() && class.extends.is_none() {
            own_constructors.push(ConstructorSig {
                arity: own_fields.len(),
                params: own_fields.iter().map(|(_, t, _)| t.clone()).collect(),
                this_target_arity: None,
                line: class.line,
            });
        }

        let kind = if crate::add_io_classes::CLASS_NAMES.contains(&class.class_name.as_str()) {
            ClassKind::Preamble
        } else {
            ClassKind::User
        };

        order.push(class.class_name.clone());
        classes.insert(
            class.class_name.clone(),
            ClassInfo {
                decl_line: class.line,
                kind,
                parent: class.extends.clone(),
                own_fields,
                own_methods,
                own_constructors,
                ancestors: Vec::new(),
                effective_fields: Vec::new(),
                effective_methods: HashMap::new(),
                vtable: Vec::new(),
                method_slot: HashMap::new(),
            },
        );
    }

    Ok(ClassTable { classes, order })
}
