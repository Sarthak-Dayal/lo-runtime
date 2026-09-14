//! Pass 2: validate inheritance and compute effective members parent first.

use std::collections::{HashMap, HashSet};

use super::class_table::{check_type_reference, find_cycle};
use super::{ClassKind, ClassTable, ErrorCode, FieldInfo, MethodEntry, TypeError};

pub(super) fn resolve_inheritance(table: &mut ClassTable) -> Result<(), TypeError> {
    let names: Vec<String> = table.order.clone();
    for name in &names {
        let info = table.classes.get(name).unwrap();
        if let Some(parent) = &info.parent {
            if !table.class_exists(parent) {
                return Err(TypeError::new(
                    ErrorCode::EUnknownClass,
                    info.decl_line,
                    format!("class '{}' extends unknown class '{}'", name, parent),
                ));
            }
            if parent == "Main" {
                return Err(TypeError::new(
                    ErrorCode::EEntryPointOther,
                    info.decl_line,
                    format!("class '{}' may not extend 'Main'", name),
                ));
            }
            if table.classes[parent].kind == ClassKind::Preamble {
                return Err(TypeError::new(
                    ErrorCode::EInheritanceCheckOther,
                    info.decl_line,
                    format!(
                        "class '{}' may not extend the non-extensible class '{}'",
                        name, parent
                    ),
                ));
            }
        }
        if info.parent.is_some() && info.own_constructors.is_empty() {
            return Err(TypeError::new(
                ErrorCode::EMissingConstructorInInheritingClass,
                info.decl_line,
                format!(
                    "class '{}' extends a parent but declares no constructor",
                    name
                ),
            ));
        }
        for (fname, ftype, fline) in &info.own_fields {
            check_type_reference(ftype, table, *fline, fname)?;
        }
        for m in &info.own_methods {
            for p in &m.params {
                check_type_reference(p, table, m.line, &m.method_name)?;
            }
            check_type_reference(&m.return_type, table, m.line, &m.method_name)?;
        }
        for c in &info.own_constructors {
            for p in &c.params {
                check_type_reference(p, table, c.line, name)?;
            }
        }
    }

    if let Some((from, _to)) = find_cycle(names.iter(), |n| {
        table
            .classes
            .get(n)
            .and_then(|i| i.parent.clone())
            .into_iter()
            .collect()
    }) {
        let line = table.classes[&from].decl_line;
        return Err(TypeError::new(
            ErrorCode::EInheritanceCycle,
            line,
            format!("inheritance cycle involving class '{}'", from),
        ));
    }

    let mut done: HashSet<String> = HashSet::new();
    for name in &names {
        compute_effective(name, table, &mut done)?;
    }

    Ok(())
}

fn compute_effective(
    name: &str,
    table: &mut ClassTable,
    done: &mut HashSet<String>,
) -> Result<(), TypeError> {
    if done.contains(name) {
        return Ok(());
    }
    let parent = table.classes[name].parent.clone();

    let (ancestors, mut effective_fields, mut effective_methods, mut vtable, mut method_slot) =
        match &parent {
            None => (vec![], vec![], HashMap::new(), vec![], HashMap::new()),
            Some(p) => {
                compute_effective(p, table, done)?;
                let parent_info = &table.classes[p];
                let mut ancestors = vec![p.clone()];
                ancestors.extend(parent_info.ancestors.iter().cloned());
                (
                    ancestors,
                    parent_info.effective_fields.clone(),
                    parent_info.effective_methods.clone(),
                    parent_info.vtable.clone(),
                    parent_info.method_slot.clone(),
                )
            }
        };

    let info = &table.classes[name];

    for (fname, ftype, fline) in &info.own_fields {
        if effective_fields.iter().any(|f| &f.name == fname) {
            return Err(TypeError::new(
                ErrorCode::EFieldShadowing,
                *fline,
                format!("field '{}' shadows an inherited field", fname),
            ));
        }
        effective_fields.push(FieldInfo {
            name: fname.clone(),
            ty: ftype.clone(),
            owner: name.to_string(),
        });
    }

    for m in &info.own_methods {
        if let Some(existing) = effective_methods.get(&m.method_name) {
            if existing.sig.params != m.params || existing.sig.return_type != m.return_type {
                return Err(TypeError::new(
                    ErrorCode::EOverrideSignatureMismatch,
                    m.line,
                    format!(
                        "'{}' overrides an ancestor method with a different signature",
                        m.method_name
                    ),
                ));
            }
        }
        effective_methods.insert(
            m.method_name.clone(),
            MethodEntry {
                owner: name.to_string(),
                sig: m.clone(),
            },
        );

        // Inherited or overridden: keep the existing slot. Genuinely new: append one.
        if !method_slot.contains_key(&m.method_name) {
            let slot = vtable.len();
            vtable.push(m.method_name.clone());
            method_slot.insert(m.method_name.clone(), slot);
        }
    }

    let info = table.classes.get_mut(name).unwrap();
    info.ancestors = ancestors;
    info.effective_fields = effective_fields;
    info.effective_methods = effective_methods;
    info.vtable = vtable;
    info.method_slot = method_slot;
    done.insert(name.to_string());
    Ok(())
}
