# Type Checker Implementation — LO (LiveOak) P1

Implements [type_checker_design.md](type_checker_design.md). The entry point is
[`compiler/src/type_checker.rs`](../compiler/src/type_checker.rs); helpers live
under `compiler/src/type_checker/`.

## Files

| File | Responsibility |
|---|---|
| `type_checker.rs` | Runs the passes and re-exports the public types. |
| `type_checker/declarations.rs` | User-name validation, duplicate declarations, formal checks, and signature gathering. |
| `type_checker/inheritance.rs` | Parent/type validation, inheritance cycles, effective members, and vtable slots. |
| `type_checker/entry_point.rs` | Required `Main` class, method, and constructor shape. |
| `type_checker/bodies.rs` | Scopes, name resolution, body traversal, statements, and return-path checks. |
| `type_checker/bodies/expressions.rs` | Expressions, compatibility, operators, ternaries, casts, and `instanceof`. |
| `type_checker/bodies/calls.rs` | Receivers, method/constructor arguments, and constructor delegation. |
| `type_checker/typed_ast.rs` | Typed nodes consumed by the interpreter and emitter. |
| `type_checker/class_table.rs` | Class metadata, subtype/constructor queries, type-reference validation, and shared cycle detection. |
| `type_checker/errors.rs` | `TypeError`, `ErrorCode`, and diagnostic-code strings. |

## Pass order

`check_program` first calls `validate_user_declared_names`, then
`add_io_classes::add_io_classes`. User classes cannot be named `Input` or `Output`,
and user methods cannot start with `lo_`.

| Pass | Function | Work |
|---|---|---|
| 1 | `gather_declarations` | Collect every class and member signature. Reject duplicate classes, fields, methods, constructor arities, and formals; check reserved variable names and void fields/formals. Synthesize a field-parameter constructor for root classes without explicit constructors. |
| 2 | `resolve_inheritance` | Validate parent and signature types, reject illegal parents and inheritance cycles, and require explicit constructors in inheriting classes. `compute_effective` preserves inherited fields, rejects shadowing and incompatible overrides, and assigns stable method slots. |
| 3 | `check_entry_point` | Require a root `Main` class declaring `int main()` with no formals and a zero-argument constructor. |
| 4 | `check_bodies` | Build typed classes, checking methods and then constructors within each class. Validate scopes, statements, expressions, calls, returns, and delegation. |

Every step uses `?`, so a failure prevents subsequent passes from running.

## Body checking

`Scope::build` stores name-to-type bindings and a set of formal names.
`resolve_name` uses these to construct `BindingInfo`. `BodyCtx` carries the
current class, return type, loop status, and constructor status.

`check_stmt` checks assignments, boolean conditions, return restrictions, and
`break` placement. Non-void methods must definitely return: a return statement
suffices, an `if` requires both branches, and a loop alone does not establish a
return. Void calls are allowed as statements; non-void calls are expressions.

`check_expr` builds typed expressions. `typed_of`, `assignment_compatible`, and
`combine_ternary_branches` share the null and subtype rules. `check_binop` and
`check_unop` enforce operator-specific types.

`check_method_call` resolves the receiver before checking actuals.
`check_constructor_call` selects by arity. `check_delegation` validates `this` and
`super` targets and arguments; `check_delegation_cycle` rejects cyclic `this`
chains. An inheriting constructor must delegate. `super.method()` is restricted
to method bodies in classes with parents.

## Integration

Consumers continue importing through `crate::type_checker`. Implementation
modules are private, and shared helpers use restricted visibility. The split
preserves the pass order and semantic rules; concrete runtime layout and
execution stay outside the checker.

## Implementation walkthrough

The sections below retain the code-level detail of the original plan, organized
by the current modules. Source links identify the implementation each listing
corresponds to. The entry module is shown first, followed by shared data and the
four passes.

### Entry point and public exports

[type_checker.rs](../compiler/src/type_checker.rs)

The pass sequence and public re-exports remain together.

```rust
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
```

### Diagnostics

[type_checker/errors.rs](../compiler/src/type_checker/errors.rs)

Each error carries a code, source line, and message. The code-to-string mapping is kept in one place.

```rust
//! Type-checker diagnostics and their stable external codes.

#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub code: ErrorCode,
    pub line: u32,
    pub message: String,
}

impl TypeError {
    pub(super) fn new(code: ErrorCode, line: u32, message: impl Into<String>) -> Self {
        TypeError {
            code,
            line,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ErrorCode {
    // well-formedness
    EDuplicateClassName,
    EDuplicateField,
    EDuplicateMethod,
    EDuplicateConstructorArity,
    EFieldTypedVoid,
    EFormalTypedVoid,
    EReturnInVoidMethod,
    EReturnMissing,
    EReturnInConstructor,
    ELocalShadowsFormal,
    EBreakOutsideLoop,
    EWellFormednessOther,
    // name resolution
    EUnknownVariable,
    EReservedVariableName,
    EReservedClassName,
    EUnknownClass,
    EUnknownMethod,
    /// Never constructed: `Expr::This` only ever parses inside a `BodyScope`
    /// (a method or constructor body), so this AST has no position for
    /// `this` to appear "outside" one. Kept for 1:1 coverage of the
    /// reference vocabulary.
    EThisOutsideInstance,
    ENameResolutionOther,
    // type-check
    ETypeMismatch,
    EAssignTypeMismatch,
    EReturnTypeMismatch,
    EActualTypeMismatch,
    EBinopTypeMismatch,
    EUnopTypeMismatch,
    EConditionalTypeMismatch,
    EArityMismatch,
    EReceiverNotClassType,
    ENullLiteralReceiver,
    ENonvoidCallAsStatement,
    EVoidCallInExpression,
    ETypeCheckOther,
    // inheritance-check
    EInheritanceCycle,
    EFieldShadowing,
    EOverrideSignatureMismatch,
    EMissingConstructorInInheritingClass,
    ESuperInRootClass,
    ESuperMethodInRootClass,
    ESuperMethodUnresolved,
    EDelegationCycle,
    EDelegationArityMismatch,
    EInheritanceCheckOther,
    // cast / instanceof
    ECastTargetNotClass,
    ECastSourceNotClass,
    ECastUnrelatedTypes,
    EInstanceofSourceNotClass,
    ECastInstanceofOther,
    // entry point
    ENoMainClass,
    EMainClassExtends,
    ENoMainMethod,
    EMainMethodSignature,
    EMainNoZeroArgConstructor,
    EEntryPointOther,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        use ErrorCode::*;
        match self {
            EDuplicateClassName => "E_DUPLICATE_CLASS_NAME",
            EDuplicateField => "E_DUPLICATE_FIELD",
            EDuplicateMethod => "E_DUPLICATE_METHOD",
            EDuplicateConstructorArity => "E_DUPLICATE_CONSTRUCTOR_ARITY",
            EFieldTypedVoid => "E_FIELD_TYPED_VOID",
            EFormalTypedVoid => "E_FORMAL_TYPED_VOID",
            EReturnInVoidMethod => "E_RETURN_IN_VOID_METHOD",
            EReturnMissing => "E_RETURN_MISSING",
            EReturnInConstructor => "E_RETURN_IN_CONSTRUCTOR",
            ELocalShadowsFormal => "E_LOCAL_SHADOWS_FORMAL",
            EBreakOutsideLoop => "E_BREAK_OUTSIDE_LOOP",
            EWellFormednessOther => "E_WELL_FORMEDNESS_OTHER",
            EUnknownVariable => "E_UNKNOWN_VARIABLE",
            EReservedVariableName => "E_RESERVED_VARIABLE_NAME",
            EReservedClassName => "E_RESERVED_CLASS_NAME",
            EUnknownClass => "E_UNKNOWN_CLASS",
            EUnknownMethod => "E_UNKNOWN_METHOD",
            EThisOutsideInstance => "E_THIS_OUTSIDE_INSTANCE",
            ENameResolutionOther => "E_NAME_RESOLUTION_OTHER",
            ETypeMismatch => "E_TYPE_MISMATCH",
            EAssignTypeMismatch => "E_ASSIGN_TYPE_MISMATCH",
            EReturnTypeMismatch => "E_RETURN_TYPE_MISMATCH",
            EActualTypeMismatch => "E_ACTUAL_TYPE_MISMATCH",
            EBinopTypeMismatch => "E_BINOP_TYPE_MISMATCH",
            EUnopTypeMismatch => "E_UNOP_TYPE_MISMATCH",
            EConditionalTypeMismatch => "E_CONDITIONAL_TYPE_MISMATCH",
            EArityMismatch => "E_ARITY_MISMATCH",
            EReceiverNotClassType => "E_RECEIVER_NOT_CLASS_TYPE",
            ENullLiteralReceiver => "E_NULL_LITERAL_RECEIVER",
            ENonvoidCallAsStatement => "E_NONVOID_CALL_AS_STATEMENT",
            EVoidCallInExpression => "E_VOID_CALL_IN_EXPRESSION",
            ETypeCheckOther => "E_TYPE_CHECK_OTHER",
            EInheritanceCycle => "E_INHERITANCE_CYCLE",
            EFieldShadowing => "E_FIELD_SHADOWING",
            EOverrideSignatureMismatch => "E_OVERRIDE_SIGNATURE_MISMATCH",
            EMissingConstructorInInheritingClass => "E_MISSING_CONSTRUCTOR_IN_INHERITING_CLASS",
            ESuperInRootClass => "E_SUPER_IN_ROOT_CLASS",
            ESuperMethodInRootClass => "E_SUPER_METHOD_IN_ROOT_CLASS",
            ESuperMethodUnresolved => "E_SUPER_METHOD_UNRESOLVED",
            EDelegationCycle => "E_DELEGATION_CYCLE",
            EDelegationArityMismatch => "E_DELEGATION_ARITY_MISMATCH",
            EInheritanceCheckOther => "E_INHERITANCE_CHECK_OTHER",
            ECastTargetNotClass => "E_CAST_TARGET_NOT_CLASS",
            ECastSourceNotClass => "E_CAST_SOURCE_NOT_CLASS",
            ECastUnrelatedTypes => "E_CAST_UNRELATED_TYPES",
            EInstanceofSourceNotClass => "E_INSTANCEOF_SOURCE_NOT_CLASS",
            ECastInstanceofOther => "E_CAST_INSTANCEOF_OTHER",
            ENoMainClass => "E_NO_MAIN_CLASS",
            EMainClassExtends => "E_MAIN_CLASS_EXTENDS",
            ENoMainMethod => "E_NO_MAIN_METHOD",
            EMainMethodSignature => "E_MAIN_METHOD_SIGNATURE",
            EMainNoZeroArgConstructor => "E_MAIN_NO_ZERO_ARG_CONSTRUCTOR",
            EEntryPointOther => "E_ENTRY_POINT_OTHER",
        }
    }
}
```

### Typed AST

[type_checker/typed_ast.rs](../compiler/src/type_checker/typed_ast.rs)

These types preserve resolved bindings, call categories, constructor metadata, and source locations.

```rust
use crate::ast::{Binop, IoOp, Type, Unop};

// ---------------------------------------------------------------------------
// Typed AST
//
// Deliberately does not borrow ast::Formal/VarDecl for its own fields/params —
// both are stored here as flattened (String, Type) pairs. That decouples this
// tree from however the parser's own field/param types are named or shaped,
// which has already changed twice.
// ---------------------------------------------------------------------------

pub struct TypedProgram {
    pub classes: Vec<TypedClassDecl>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ClassKind {
    User,
    Preamble,
}

pub struct TypedClassDecl {
    pub class_name: String,
    pub kind: ClassKind,
    pub extends: Option<String>,
    pub fields: Vec<(String, Type)>,
    pub constructors: Vec<TypedConstructor>,
    pub methods: Vec<TypedMethodDecl>,
    pub line: u32,
}

pub enum TypedConstructor {
    Explicit {
        formals: Vec<(String, Type)>,
        delegation: Option<TypedDelegation>,
        locals: Vec<(String, Type)>,
        stmts: Vec<TypedStmt>,
        line: u32,
    },
    /// `this.field_i = formal_i`, in field order — synthesized once here so
    /// downstream consumers don't each have to re-derive what an implicit
    /// constructor does. No `line`: unlike `Explicit`, this has no source
    /// constructor to point at.
    Implicit { fields: Vec<(String, Type)> },
}

pub enum TypedDelegation {
    ThisCall { actuals: Vec<TypedExpr>, line: u32 },
    SuperCall { actuals: Vec<TypedExpr>, line: u32 },
}

pub struct TypedMethodDecl {
    pub method_name: String,
    pub return_type: Type,
    pub formals: Vec<(String, Type)>,
    pub body: TypedMethodBody,
    pub line: u32,
}

pub enum TypedMethodBody {
    UserDefined {
        locals: Vec<(String, Type)>,
        stmts: Vec<TypedStmt>,
    },
    Io(IoOp),
}

#[derive(Clone)]
pub enum BindingInfo {
    Local(Type),
    Formal(Type),
    Field {
        owner: String,
        ty: Type,
    },
    /// `in`/`out`/`err` — the program-scope names the preamble binds. Not a
    /// field of any class, so distinct from `Field` rather than faked as one.
    Prebound(Type),
}

impl BindingInfo {
    pub fn ty(&self) -> &Type {
        match self {
            BindingInfo::Local(t) | BindingInfo::Formal(t) | BindingInfo::Prebound(t) => t,
            BindingInfo::Field { ty, .. } => ty,
        }
    }
}

pub enum TypedStmt {
    Assign {
        target: String,
        binding: BindingInfo,
        value: TypedExpr,
        line: u32,
    },
    Return(TypedExpr, u32),
    If(TypedExpr, Vec<TypedStmt>, Vec<TypedStmt>, u32),
    While(TypedExpr, Vec<TypedStmt>, u32),
    Break(u32),
    Empty(u32),
    /// No `line`: `TypedMethodCall` already carries one.
    CallStmt(TypedMethodCall),
}

pub struct TypedMethodCall {
    pub obj_name: TypedObjName,
    pub method_name: String,
    pub resolution: MethodResolution,
    pub actuals: Vec<TypedExpr>,
    pub return_type: Type,
    pub line: u32,
}

/// How a call's target is found at runtime — distinct from `r.m()` and
/// `super.m()` having different resolution semantics (static-type virtual
/// dispatch vs. resolving from the nearest ancestor that declares the
/// method), and from a built-in IO operation having no vtable slot at all.
pub enum MethodResolution {
    Virtual { static_class: String },
    Super { declaring_class: String },
    Io { op: IoOp },
}

pub enum TypedObjName {
    This(String, u32),
    /// The declaring class lives on the enclosing `TypedMethodCall`'s
    /// `MethodResolution::Super`.
    Super(u32),
    Var {
        name: String,
        binding: BindingInfo,
        line: u32,
    },
    Computed(Box<TypedExpr>, u32),
}

pub enum TypedExpr {
    Num(i32, u32),
    Bool(bool, u32),
    Str(String, u32),
    /// No `Type` field — `null` has none of its own; every consumer already
    /// has the surrounding context's type (assignment target, cast target,
    /// ternary LCA) when it needs one.
    Null(u32),
    This(String, u32),
    Var {
        name: String,
        binding: BindingInfo,
        line: u32,
    },
    New {
        class: String,
        actuals: Vec<TypedExpr>,
        line: u32,
    },
    /// No `line`: `TypedMethodCall` already carries one.
    Call(TypedMethodCall),
    /// `ty: None` iff both branches are `null` — like `TypedExpr::Null`, no
    /// type of its own; the surrounding context (assignment target, cast,
    /// enclosing ternary) resolves it.
    Ternary {
        cond: Box<TypedExpr>,
        then_branch: Box<TypedExpr>,
        else_branch: Box<TypedExpr>,
        ty: Option<Type>,
        line: u32,
    },
    Binop {
        lhs: Box<TypedExpr>,
        op: Binop,
        rhs: Box<TypedExpr>,
        ty: Type,
        line: u32,
    },
    Unop {
        op: Unop,
        operand: Box<TypedExpr>,
        ty: Type,
        line: u32,
    },
    Cast {
        target: Type,
        operand: Box<TypedExpr>,
        direction: CastDirection,
        line: u32,
    },
    InstanceOf {
        operand: Box<TypedExpr>,
        class: String,
        line: u32,
    },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CastDirection {
    Upcast,
    Downcast,
    /// The operand is the literal `null` — always succeeds, no runtime check.
    Null,
}
```

### Class table and shared queries

[type_checker/class_table.rs](../compiler/src/type_checker/class_table.rs)

Own declarations remain internal; consumers use effective members and the existing query methods. Cycle detection is shared by inheritance and delegation.

```rust
//! Class signatures and effective members.

use std::collections::{HashMap, HashSet};

use super::{ClassKind, ErrorCode, TypeError};
use crate::ast::{IoOp, Type};

pub struct ClassTable {
    pub(super) classes: HashMap<String, ClassInfo>,
    pub(super) order: Vec<String>,
}

pub struct ClassInfo {
    pub decl_line: u32,
    pub kind: ClassKind,
    pub parent: Option<String>,
    pub(super) own_fields: Vec<(String, Type, u32)>,
    pub(super) own_methods: Vec<MethodSig>,
    pub(super) own_constructors: Vec<ConstructorSig>,

    pub ancestors: Vec<String>,
    pub effective_fields: Vec<FieldInfo>,
    pub effective_methods: HashMap<String, MethodEntry>,

    /// Method names in vtable-slot order: an inherited method keeps its
    /// parent's slot, an override reuses that slot, and a genuinely new
    /// method is appended. `method_slot[name]` is that name's index here.
    pub vtable: Vec<String>,
    pub method_slot: HashMap<String, usize>,
}

/// An effective (possibly inherited) field: where it's declared and its type.
#[derive(Clone)]
pub struct FieldInfo {
    pub name: String,
    pub ty: Type,
    pub owner: String,
}

/// An effective (possibly inherited) method: which class's signature won out
/// and what that signature is. `owner` is the declaring class, not
/// necessarily the class this `ClassInfo` belongs to.
#[derive(Clone)]
pub struct MethodEntry {
    pub owner: String,
    pub sig: MethodSig,
}

#[derive(Clone, PartialEq)]
pub struct MethodSig {
    pub method_name: String,
    pub params: Vec<Type>,
    pub return_type: Type,
    pub line: u32,
    /// `Some(op)` iff this signature's body is a built-in IO operation rather
    /// than user-defined code.
    pub is_io: Option<IoOp>,
}

#[derive(Clone)]
pub struct ConstructorSig {
    pub arity: usize,
    pub params: Vec<Type>,
    /// `Some(n)` iff this constructor's own delegation is `this(...)` targeting
    /// the n-arity constructor of the same class.
    pub this_target_arity: Option<usize>,
    pub line: u32,
}

impl ClassTable {
    pub fn get(&self, name: &str) -> Option<&ClassInfo> {
        self.classes.get(name)
    }

    pub fn class_exists(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    pub fn is_subtype(&self, a: &str, b: &str) -> bool {
        a == b
            || self
                .classes
                .get(a)
                .is_some_and(|info| info.ancestors.iter().any(|anc| anc == b))
    }
}

impl ClassInfo {
    /// This class's own constructors (LO has no constructor inheritance --
    /// every instantiable class declares, or is given, its own). Exposed so
    /// downstream consumers (the interpreter, wasm codegen) can dispatch
    /// `new` without re-deriving arity/param information the type checker
    /// already computed.
    pub fn constructors(&self) -> &[ConstructorSig] {
        &self.own_constructors
    }

    pub fn find_constructor(&self, arity: usize) -> Option<&ConstructorSig> {
        self.own_constructors.iter().find(|c| c.arity == arity)
    }
}

pub(super) fn check_type_reference(
    ty: &Type,
    table: &ClassTable,
    line: u32,
    ctx: &str,
) -> Result<(), TypeError> {
    if let Type::Class(name) = ty {
        if !table.class_exists(name) {
            return Err(TypeError::new(
                ErrorCode::EUnknownClass,
                line,
                format!("unknown class '{}' referenced in '{}'", name, ctx),
            ));
        }
    }
    Ok(())
}

/// Shared cycle detection for inheritance and constructor delegation.
pub(super) fn find_cycle<'a, N, F>(nodes: impl Iterator<Item = &'a N>, edges: F) -> Option<(N, N)>
where
    N: Eq + std::hash::Hash + Clone + 'a,
    F: Fn(&N) -> Vec<N>,
{
    let mut visited: HashSet<N> = HashSet::new();
    let mut on_path: Vec<N> = Vec::new();

    fn visit<N, F>(
        node: &N,
        edges: &F,
        visited: &mut HashSet<N>,
        on_path: &mut Vec<N>,
    ) -> Option<(N, N)>
    where
        N: Eq + std::hash::Hash + Clone,
        F: Fn(&N) -> Vec<N>,
    {
        if on_path.contains(node) {
            return Some((on_path.last().unwrap().clone(), node.clone()));
        }
        if !visited.insert(node.clone()) {
            return None;
        }
        on_path.push(node.clone());
        for next in edges(node) {
            if let Some(cycle) = visit(&next, edges, visited, on_path) {
                return Some(cycle);
            }
        }
        on_path.pop();
        None
    }

    for node in nodes {
        if !visited.contains(node) {
            if let Some(cycle) = visit(node, &edges, &mut visited, &mut on_path) {
                return Some(cycle);
            }
        }
    }
    None
}
```

### Pass 1 — declarations

[type_checker/declarations.rs](../compiler/src/type_checker/declarations.rs)

User-name validation precedes injection. Signature gathering follows injection and preserves declaration order.

```rust
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
```

### Pass 2 — inheritance

[type_checker/inheritance.rs](../compiler/src/type_checker/inheritance.rs)

Parent/type validation and cycle rejection precede effective-member computation. Overrides retain inherited slots.

```rust
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
```

### Pass 3 — entry point

[type_checker/entry_point.rs](../compiler/src/type_checker/entry_point.rs)

The required Main shape is checked before any body is transformed.

```rust
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
```

### Pass 4 — scopes, statements, and bodies

[type_checker/bodies.rs](../compiler/src/type_checker/bodies.rs)

Scopes and BodyCtx are shared with the expression/call child modules. Methods are checked before constructors within each class.

```rust
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
```

### Expression and compatibility rules

[type_checker/bodies/expressions.rs](../compiler/src/type_checker/bodies/expressions.rs)

Null stays distinct until context supplies a concrete type. Operator rules, ternary LCA, and cast direction are resolved here.

```rust
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
```

### Calls and constructor delegation

[type_checker/bodies/calls.rs](../compiler/src/type_checker/bodies/calls.rs)

Receiver lookup precedes argument checking. Ordinary construction and delegation select constructors by arity, then check types.

```rust
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
```

## Behavioral examples

These examples describe checker behavior, assuming earlier passes succeed and
any required surrounding declarations are present. They are documentation, not
an embedded test harness.

| Case | Expected behavior |
|---|---|
| Method calls another method declared later | Accepted; signatures are collected before bodies. |
| Duplicate formal name | `E_WELL_FORMEDNESS_OTHER`. |
| Local repeats a formal name | `E_LOCAL_SHADOWS_FORMAL`. |
| Unknown class in a local declaration | `E_UNKNOWN_CLASS`. |
| Root class with no explicit constructor | Implicit constructor parameters follow field order. |
| Inheriting class has no constructor | `E_MISSING_CONSTRUCTOR_IN_INHERITING_CLASS`. |
| Explicit constructor has no delegation, locals, or statements | `E_WELL_FORMEDNESS_OTHER`; a `;` statement makes it nonempty. |
| Override changes a parameter or return type | `E_OVERRIDE_SIGNATURE_MISMATCH`. |
| Override keeps its signature | Accepted; reuses the inherited slot. |
| Void call used as an expression | `E_VOID_CALL_IN_EXPRESSION`. |
| Non-void call used as a statement | `E_NONVOID_CALL_AS_STATEMENT`. |
| Non-void method has statements but no guaranteed return | `E_RETURN_MISSING`. |
| Break outside a loop | `E_BREAK_OUTSIDE_LOOP`. |
| Class reference compared with null | Accepted with bool result. |
| Ternary between sibling classes with a shared ancestor | Result type is the least common ancestor. |
| Ternary with two null branches | Accepted with `ty: None`. |
| Cast of literal null to a class | Accepted with `CastDirection::Null`. |
| Downcast of a class-typed variable holding null | Accepted as `Downcast`; runtime must permit null. |
| `instanceof` against an unrelated class | Accepted; runtime returns false. |
| `super.method()` in a constructor | `E_INHERITANCE_CHECK_OTHER`. |
| Cycle among `this(...)` delegations | `E_DELEGATION_CYCLE`. |
| User method named `lo_alloc` | `E_RESERVED_VARIABLE_NAME`. |

## Scope boundary

The driver already exposes checking, interpretation, and WASM compilation.
Those consumers remain outside this implementation. The checker supplies field
order and slots; the emitter supplies byte offsets, descriptors, and emitted
vtable data. Runtime String operations, allocation, collection, I/O, and abort
behavior are implemented by the execution path, not by these passes.
