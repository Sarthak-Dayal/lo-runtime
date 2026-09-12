use crate::ast::{
    Binop, ConstructorDecl, ConstructorDelegation, Expr, Formal, IoOp, MethodBody, MethodCall,
    ObjName, Program, Stmt, Type, Unop, VarDecl,
};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub code: ErrorCode,
    pub line: u32,
    pub message: String,
}

impl TypeError {
    fn new(code: ErrorCode, line: u32, message: impl Into<String>) -> Self {
        TypeError { code, line, message: message.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ErrorCode {
    // well-formedness
    EDuplicateClassName,
    EReservedClassName,
    EDuplicateField,
    EDuplicateMethod,
    EDuplicateFormal,
    EDuplicateConstructorArity,
    EFieldTypedVoid,
    EFormalTypedVoid,
    ELocalTypedVoid,
    EReturnInVoidMethod,
    EReturnMissing,
    EReturnInConstructor,
    EBreakOutsideLoop,
    EWellFormednessOther,
    // name resolution
    EUnknownVariable,
    ELocalShadowsFormal,
    EReservedVariableName,
    EUnknownMethod,
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
    EUnknownClass,
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
            EReservedClassName => "E_RESERVED_CLASS_NAME",
            EDuplicateField => "E_DUPLICATE_FIELD",
            EDuplicateMethod => "E_DUPLICATE_METHOD",
            EDuplicateFormal => "E_DUPLICATE_FORMAL",
            EDuplicateConstructorArity => "E_DUPLICATE_CONSTRUCTOR_ARITY",
            EFieldTypedVoid => "E_FIELD_TYPED_VOID",
            EFormalTypedVoid => "E_FORMAL_TYPED_VOID",
            ELocalTypedVoid => "E_LOCAL_TYPED_VOID",
            EReturnInVoidMethod => "E_RETURN_IN_VOID_METHOD",
            EReturnMissing => "E_RETURN_MISSING",
            EReturnInConstructor => "E_RETURN_IN_CONSTRUCTOR",
            EBreakOutsideLoop => "E_BREAK_OUTSIDE_LOOP",
            EWellFormednessOther => "E_WELL_FORMEDNESS_OTHER",
            EUnknownVariable => "E_UNKNOWN_VARIABLE",
            ELocalShadowsFormal => "E_LOCAL_SHADOWS_FORMAL",
            EReservedVariableName => "E_RESERVED_VARIABLE_NAME",
            EUnknownMethod => "E_UNKNOWN_METHOD",
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
            EUnknownClass => "E_UNKNOWN_CLASS",
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

/// Internal comparison currency during checking only — never part of the typed AST.
#[derive(Debug, Clone, PartialEq)]
enum ExprType {
    Concrete(Type),
    NullLiteral,
}

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
    pub name: String,
    pub kind: ClassKind,
    pub extends: Option<String>,
    pub fields: Vec<(String, Type)>,
    pub constructors: Vec<TypedConstructor>,
    pub methods: Vec<TypedMethodDecl>,
}

pub enum TypedConstructor {
    Explicit {
        params: Vec<(String, Type)>,
        delegation: Option<TypedDelegation>,
        locals: Vec<(String, Type)>,
        stmts: Vec<TypedStmt>,
    },
    /// `this.field_i = formal_i`, in field order — synthesized once here so
    /// downstream consumers don't each have to re-derive what an implicit
    /// constructor does.
    Implicit { fields: Vec<(String, Type)> },
}

pub enum TypedDelegation {
    This { args: Vec<TypedExpr> },
    Super { args: Vec<TypedExpr> },
}

pub struct TypedMethodDecl {
    pub name: String,
    pub return_type: Type,
    pub params: Vec<(String, Type)>,
    pub body: TypedMethodBody,
}

pub enum TypedMethodBody {
    UserDefined { locals: Vec<(String, Type)>, stmts: Vec<TypedStmt> },
    Io(IoOp),
}

#[derive(Clone)]
pub enum BindingInfo {
    Local(Type),
    Formal(Type),
    Field { owner: String, ty: Type },
}

impl BindingInfo {
    pub fn ty(&self) -> &Type {
        match self {
            BindingInfo::Local(t) | BindingInfo::Formal(t) => t,
            BindingInfo::Field { ty, .. } => ty,
        }
    }
}

pub enum TypedStmt {
    Assign { target: String, binding: BindingInfo, value: TypedExpr },
    Return(TypedExpr),
    If(TypedExpr, Vec<TypedStmt>, Vec<TypedStmt>),
    While(TypedExpr, Vec<TypedStmt>),
    Break,
    Empty,
    CallStmt(TypedMethodCall),
}

pub struct TypedMethodCall {
    pub receiver: TypedReceiver,
    pub name: String,
    /// The class whose `effective_methods` entry resolved this call.
    pub owner: String,
    pub args: Vec<TypedExpr>,
    pub return_type: Type,
}

pub enum TypedReceiver {
    This(String),
    /// Owner lives on the enclosing `TypedMethodCall`.
    Super,
    Var { name: String, binding: BindingInfo },
    Computed(Box<TypedExpr>),
}

pub enum TypedExpr {
    Num(i32),
    Bool(bool),
    Str(String),
    /// No `Type` field — `null` has none of its own; every consumer already
    /// has the surrounding context's type (assignment target, cast target,
    /// ternary LCA) when it needs one.
    Null,
    This(String),
    Var { name: String, binding: BindingInfo },
    New { class: String, args: Vec<TypedExpr> },
    Call(TypedMethodCall),
    Ternary { cond: Box<TypedExpr>, then_branch: Box<TypedExpr>, else_branch: Box<TypedExpr>, ty: Type },
    Binary { lhs: Box<TypedExpr>, op: Binop, rhs: Box<TypedExpr>, ty: Type },
    Unary { op: Unop, operand: Box<TypedExpr>, ty: Type },
    Cast { target: Type, operand: Box<TypedExpr>, direction: CastDirection },
    InstanceOf { operand: Box<TypedExpr>, class: String },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CastDirection {
    Upcast,
    Downcast,
}

fn typed_of(e: &TypedExpr) -> ExprType {
    match e {
        TypedExpr::Null => ExprType::NullLiteral,
        TypedExpr::Num(_) => ExprType::Concrete(Type::Int),
        TypedExpr::Bool(_) => ExprType::Concrete(Type::Bool),
        TypedExpr::Str(_) => ExprType::Concrete(Type::String),
        TypedExpr::This(c) => ExprType::Concrete(Type::Class(c.clone())),
        TypedExpr::Var { binding, .. } => ExprType::Concrete(binding.ty().clone()),
        TypedExpr::New { class, .. } => ExprType::Concrete(Type::Class(class.clone())),
        TypedExpr::Call(call) => ExprType::Concrete(call.return_type.clone()),
        TypedExpr::Ternary { ty, .. }
        | TypedExpr::Binary { ty, .. }
        | TypedExpr::Unary { ty, .. } => ExprType::Concrete(ty.clone()),
        TypedExpr::Cast { target, .. } => ExprType::Concrete(target.clone()),
        TypedExpr::InstanceOf { .. } => ExprType::Concrete(Type::Bool),
    }
}

// ---------------------------------------------------------------------------
// Class table
// ---------------------------------------------------------------------------

pub struct ClassTable {
    classes: HashMap<String, ClassInfo>,
    order: Vec<String>,
}

pub struct ClassInfo {
    pub decl_line: u32,
    pub kind: ClassKind,
    pub parent: Option<String>,
    own_fields: Vec<(String, Type, u32)>,
    own_methods: Vec<MethodSig>,
    own_constructors: Vec<ConstructorSig>,

    pub ancestors: Vec<String>,
    pub effective_fields: Vec<(String, Type, String)>,
    pub effective_methods: HashMap<String, (String, MethodSig)>,
}

#[derive(Clone, PartialEq)]
pub struct MethodSig {
    pub name: String,
    pub params: Vec<Type>,
    pub return_type: Type,
    pub line: u32,
}

#[derive(Clone)]
struct ConstructorSig {
    arity: usize,
    params: Vec<Type>,
    /// `Some(n)` iff this constructor's own delegation is `this(...)` targeting
    /// the n-arity constructor of the same class.
    this_target_arity: Option<usize>,
    line: u32,
}

impl ClassTable {
    pub fn get(&self, name: &str) -> Option<&ClassInfo> {
        self.classes.get(name)
    }

    pub fn class_exists(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    pub fn is_subtype(&self, a: &str, b: &str) -> bool {
        a == b || self.classes.get(a).is_some_and(|info| info.ancestors.iter().any(|anc| anc == b))
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

fn find_cycle<'a, N, F>(nodes: impl Iterator<Item = &'a N>, edges: F) -> Option<(N, N)>
where
    N: Eq + std::hash::Hash + Clone + 'a,
    F: Fn(&N) -> Vec<N>,
{
    let mut visited: HashSet<N> = HashSet::new();
    let mut on_path: Vec<N> = Vec::new();

    fn visit<N, F>(node: &N, edges: &F, visited: &mut HashSet<N>, on_path: &mut Vec<N>) -> Option<(N, N)>
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

// ---------------------------------------------------------------------------
// Preamble
//
// The `Input`/`Output` class injection itself lives in the shared
// `add_io_classes` module; this just maps `in`/`out`/`err` to the types it
// injects, for binding lookup during body-checking.
// ---------------------------------------------------------------------------

fn preamble_binding(name: &str) -> Option<Type> {
    match name {
        "in" => Some(Type::Class("Input".to_string())),
        "out" | "err" => Some(Type::Class("Output".to_string())),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Pass 1 — gather_declarations
// ---------------------------------------------------------------------------

const RESERVED_CLASS_NAMES: &[&str] = &["Input", "Output"];
const RESERVED_VAR_NAMES: &[&str] = &["in", "out", "err"];

fn check_not_reserved_var_name(name: &str, line: u32) -> Result<(), TypeError> {
    if RESERVED_VAR_NAMES.contains(&name) {
        return Err(TypeError::new(ErrorCode::EReservedVariableName, line, format!("'{}' is a reserved name", name)));
    }
    Ok(())
}

fn check_formals_well_formed(formals: &[Formal]) -> Result<(), TypeError> {
    let mut seen = HashSet::new();
    for f in formals {
        if f.declared_type == Type::Void {
            return Err(TypeError::new(ErrorCode::EFormalTypedVoid, f.line, format!("formal '{}' cannot have type void", f.identifier)));
        }
        check_not_reserved_var_name(&f.identifier, f.line)?;
        if !seen.insert(f.identifier.clone()) {
            return Err(TypeError::new(ErrorCode::EDuplicateFormal, f.line, format!("duplicate formal parameter '{}'", f.identifier)));
        }
    }
    Ok(())
}

fn gather_declarations(program: &Program) -> Result<ClassTable, TypeError> {
    let mut classes = HashMap::new();
    let mut order = Vec::new();

    for class in &program.classes {
        // Reserved-name check before duplicate-name: both preamble classes are
        // already present by the time any user class is visited, so a user's
        // `class Input(){}` would otherwise collide with EDuplicateClassName first.
        if RESERVED_CLASS_NAMES.contains(&class.class_name.as_str()) && class.line != 0 {
            return Err(TypeError::new(ErrorCode::EReservedClassName, class.line, format!("class name '{}' is reserved", class.class_name)));
        }
        if classes.contains_key(&class.class_name) {
            return Err(TypeError::new(ErrorCode::EDuplicateClassName, class.line, format!("class '{}' declared more than once", class.class_name)));
        }

        // class.fields is Vec<VarDecl> — grouped by type ("int a, b;" is one
        // VarDecl naming two fields) — flatten to one entry per name.
        let mut own_fields = Vec::new();
        for decl in &class.fields {
            if decl.declared_type == Type::Void {
                return Err(TypeError::new(ErrorCode::EFieldTypedVoid, decl.line, "a field cannot have type void"));
            }
            for name in &decl.identifiers {
                check_not_reserved_var_name(name, decl.line)?;
                if own_fields.iter().any(|(n, ..): &(String, Type, u32)| n == name) {
                    return Err(TypeError::new(ErrorCode::EDuplicateField, decl.line, format!("duplicate field '{}'", name)));
                }
                own_fields.push((name.clone(), decl.declared_type.clone(), decl.line));
            }
        }

        let mut own_methods: Vec<MethodSig> = Vec::new();
        for method in &class.methods {
            if own_methods.iter().any(|m| m.name == method.method_name) {
                return Err(TypeError::new(ErrorCode::EDuplicateMethod, method.line, format!("duplicate method '{}'", method.method_name)));
            }
            check_formals_well_formed(&method.formals)?;
            own_methods.push(MethodSig {
                name: method.method_name.clone(),
                params: method.formals.iter().map(|f| f.declared_type.clone()).collect(),
                return_type: method.return_type.clone(),
                line: method.line,
            });
        }

        let mut own_constructors: Vec<ConstructorSig> = Vec::new();
        for ctor in &class.constructors {
            let arity = ctor.formals.len();
            if own_constructors.iter().any(|c| c.arity == arity) {
                return Err(TypeError::new(ErrorCode::EDuplicateConstructorArity, ctor.line,
                    format!("class '{}' already has a constructor of arity {}", class.class_name, arity)));
            }
            check_formals_well_formed(&ctor.formals)?;
            let this_target_arity = match &ctor.delegation {
                Some(ConstructorDelegation::ThisCall(args, _)) => Some(args.len()),
                _ => None,
            };
            own_constructors.push(ConstructorSig {
                arity,
                params: ctor.formals.iter().map(|f| f.declared_type.clone()).collect(),
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

        order.push(class.class_name.clone());
        classes.insert(class.class_name.clone(), ClassInfo {
            decl_line: class.line,
            kind: if class.line == 0 { ClassKind::Preamble } else { ClassKind::User },
            parent: class.extends.clone(),
            own_fields,
            own_methods,
            own_constructors,
            ancestors: Vec::new(),
            effective_fields: Vec::new(),
            effective_methods: HashMap::new(),
        });
    }

    Ok(ClassTable { classes, order })
}

// ---------------------------------------------------------------------------
// Pass 2 — resolve_inheritance
// ---------------------------------------------------------------------------

fn resolve_inheritance(table: &mut ClassTable) -> Result<(), TypeError> {
    let names: Vec<String> = table.order.clone();
    for name in &names {
        let info = table.classes.get(name).unwrap();
        if let Some(parent) = &info.parent {
            if !table.class_exists(parent) {
                return Err(TypeError::new(ErrorCode::EUnknownClass, info.decl_line, format!("class '{}' extends unknown class '{}'", name, parent)));
            }
            if parent == "Main" {
                return Err(TypeError::new(ErrorCode::EEntryPointOther, info.decl_line, format!("class '{}' may not extend 'Main'", name)));
            }
            if table.classes[parent].kind == ClassKind::Preamble {
                return Err(TypeError::new(ErrorCode::EInheritanceCheckOther, info.decl_line,
                    format!("class '{}' may not extend the non-extensible class '{}'", name, parent)));
            }
        }
        if info.parent.is_some() && info.own_constructors.is_empty() {
            return Err(TypeError::new(ErrorCode::EMissingConstructorInInheritingClass, info.decl_line,
                format!("class '{}' extends a parent but declares no constructor", name)));
        }
        for (fname, ftype, fline) in &info.own_fields {
            check_type_reference(ftype, table, *fline, fname)?;
        }
        for m in &info.own_methods {
            for p in &m.params { check_type_reference(p, table, m.line, &m.name)?; }
            check_type_reference(&m.return_type, table, m.line, &m.name)?;
        }
        for c in &info.own_constructors {
            for p in &c.params { check_type_reference(p, table, c.line, name)?; }
        }
    }

    if let Some((from, _to)) = find_cycle(names.iter(), |n| {
        table.classes.get(n).and_then(|i| i.parent.clone()).into_iter().collect()
    }) {
        let line = table.classes[&from].decl_line;
        return Err(TypeError::new(ErrorCode::EInheritanceCycle, line, format!("inheritance cycle involving class '{}'", from)));
    }

    let mut done: HashSet<String> = HashSet::new();
    for name in &names {
        compute_effective(name, table, &mut done)?;
    }

    Ok(())
}

fn check_type_reference(ty: &Type, table: &ClassTable, line: u32, ctx: &str) -> Result<(), TypeError> {
    if let Type::Class(name) = ty {
        if !table.class_exists(name) {
            return Err(TypeError::new(ErrorCode::EUnknownClass, line, format!("unknown class '{}' referenced in '{}'", name, ctx)));
        }
    }
    Ok(())
}

fn compute_effective(name: &str, table: &mut ClassTable, done: &mut HashSet<String>) -> Result<(), TypeError> {
    if done.contains(name) {
        return Ok(());
    }
    let parent = table.classes[name].parent.clone();

    let (ancestors, mut effective_fields, mut effective_methods) = match &parent {
        None => (vec![], vec![], HashMap::new()),
        Some(p) => {
            compute_effective(p, table, done)?;
            let parent_info = &table.classes[p];
            let mut ancestors = vec![p.clone()];
            ancestors.extend(parent_info.ancestors.iter().cloned());
            (ancestors, parent_info.effective_fields.clone(), parent_info.effective_methods.clone())
        }
    };

    let info = &table.classes[name];

    for (fname, ftype, fline) in &info.own_fields {
        if effective_fields.iter().any(|(n, ..)| n == fname) {
            return Err(TypeError::new(ErrorCode::EFieldShadowing, *fline, format!("field '{}' shadows an inherited field", fname)));
        }
        effective_fields.push((fname.clone(), ftype.clone(), name.to_string()));
    }

    for m in &info.own_methods {
        if let Some((_, existing)) = effective_methods.get(&m.name) {
            if existing.params != m.params || existing.return_type != m.return_type {
                return Err(TypeError::new(ErrorCode::EOverrideSignatureMismatch, m.line,
                    format!("'{}' overrides an ancestor method with a different signature", m.name)));
            }
        }
        effective_methods.insert(m.name.clone(), (name.to_string(), m.clone()));
    }

    let info = table.classes.get_mut(name).unwrap();
    info.ancestors = ancestors;
    info.effective_fields = effective_fields;
    info.effective_methods = effective_methods;
    done.insert(name.to_string());
    Ok(())
}

// ---------------------------------------------------------------------------
// Pass 3 — check_entry_point
// ---------------------------------------------------------------------------

fn check_entry_point(table: &ClassTable) -> Result<(), TypeError> {
    let main = table.get("Main").ok_or_else(|| TypeError::new(ErrorCode::ENoMainClass, 0, "no class named 'Main' declared"))?;

    if main.parent.is_some() {
        return Err(TypeError::new(ErrorCode::EMainClassExtends, main.decl_line, "'Main' must not have an extends clause"));
    }

    match main.own_methods.iter().find(|m| m.name == "main") {
        None => return Err(TypeError::new(ErrorCode::ENoMainMethod, main.decl_line, "'Main' must declare 'int main()'")),
        Some(m) if m.return_type != Type::Int || !m.params.is_empty() => {
            return Err(TypeError::new(ErrorCode::EMainMethodSignature, m.line, "'main' must return int and take no formals"));
        }
        _ => {}
    }

    if !main.own_constructors.iter().any(|c| c.arity == 0) {
        return Err(TypeError::new(ErrorCode::EMainNoZeroArgConstructor, main.decl_line, "'Main' must have a zero-arg constructor"));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Scope and name resolution
// ---------------------------------------------------------------------------

struct Scope {
    bindings: HashMap<String, Type>,
    formal_names: HashSet<String>,
}

impl Scope {
    fn build(formals: &[Formal], locals: &[VarDecl], table: &ClassTable) -> Result<Scope, TypeError> {
        let mut bindings = HashMap::new();
        let mut formal_names = HashSet::new();
        for f in formals {
            bindings.insert(f.identifier.clone(), f.declared_type.clone());
            formal_names.insert(f.identifier.clone());
        }
        for decl in locals {
            if decl.declared_type == Type::Void {
                return Err(TypeError::new(ErrorCode::ELocalTypedVoid, decl.line, "a local variable cannot have type void"));
            }
            check_type_reference(&decl.declared_type, table, decl.line, "local declaration")?;
            for name in &decl.identifiers {
                check_not_reserved_var_name(name, decl.line)?;
                if formal_names.contains(name) {
                    return Err(TypeError::new(ErrorCode::ELocalShadowsFormal, decl.line,
                        format!("local '{}' has the same name as a formal parameter", name)));
                }
                bindings.insert(name.clone(), decl.declared_type.clone());
            }
        }
        Ok(Scope { bindings, formal_names })
    }
}

fn resolve_name(name: &str, scope: &Scope, class_name: &str, table: &ClassTable) -> Option<BindingInfo> {
    if let Some(t) = scope.bindings.get(name) {
        return Some(if scope.formal_names.contains(name) {
            BindingInfo::Formal(t.clone())
        } else {
            BindingInfo::Local(t.clone())
        });
    }
    let info = table.get(class_name)?;
    if let Some((_, t, owner)) = info.effective_fields.iter().find(|(n, ..)| n == name) {
        return Some(BindingInfo::Field { owner: owner.clone(), ty: t.clone() });
    }
    preamble_binding(name).map(|t| BindingInfo::Field { owner: "<preamble>".to_string(), ty: t })
}

// ---------------------------------------------------------------------------
// Pass 4 — body checking
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct BodyCtx<'a> {
    class_name: &'a str,
    return_type: &'a Type,
    in_loop: bool,
    in_constructor: bool,
}

fn flatten_locals(locals: &[VarDecl]) -> Vec<(String, Type)> {
    locals.iter()
        .flat_map(|d| d.identifiers.iter().map(move |n| (n.clone(), d.declared_type.clone())))
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

fn check_bodies(program: &Program, table: &ClassTable) -> Result<TypedProgram, TypeError> {
    let mut typed_classes = Vec::with_capacity(program.classes.len());

    for class in &program.classes {
        let info = table.get(&class.class_name).unwrap();

        let mut typed_methods = Vec::with_capacity(class.methods.len());
        for method in &class.methods {
            let body = match &method.body {
                MethodBody::Io(op) => {
                    typed_methods.push(TypedMethodDecl {
                        name: method.method_name.clone(),
                        return_type: method.return_type.clone(),
                        params: method.formals.iter().map(|f| (f.identifier.clone(), f.declared_type.clone())).collect(),
                        body: TypedMethodBody::Io(op.clone()),
                    });
                    continue;
                }
                MethodBody::UserDefined(b) => b,
            };
            let scope = Scope::build(&method.formals, &body.locals, table)?;
            let ctx = BodyCtx { class_name: &class.class_name, return_type: &method.return_type, in_loop: false, in_constructor: false };
            let mut typed_stmts = Vec::with_capacity(body.stmts.len());
            for stmt in &body.stmts {
                typed_stmts.push(check_stmt(stmt, &scope, &ctx, table)?);
            }
            if method.return_type != Type::Void && !definitely_returns(&body.stmts) {
                return Err(TypeError::new(ErrorCode::EReturnMissing, method.line, format!("'{}' does not return on every path", method.method_name)));
            }
            typed_methods.push(TypedMethodDecl {
                name: method.method_name.clone(),
                return_type: method.return_type.clone(),
                params: method.formals.iter().map(|f| (f.identifier.clone(), f.declared_type.clone())).collect(),
                body: TypedMethodBody::UserDefined { locals: flatten_locals(&body.locals), stmts: typed_stmts },
            });
        }

        let typed_constructors = if class.constructors.is_empty() {
            // Only reachable when extends.is_none() — resolve_inheritance already
            // rejected an inheriting class with no explicit constructor section.
            let fields = info.own_fields.iter().map(|(n, t, _)| (n.clone(), t.clone())).collect();
            vec![TypedConstructor::Implicit { fields }]
        } else {
            check_delegation_cycle(&class.class_name, table)?;
            let mut typed_ctors = Vec::with_capacity(class.constructors.len());
            for ctor in &class.constructors {
                if ctor.delegation.is_none() && ctor.body.stmts.is_empty() {
                    return Err(TypeError::new(ErrorCode::EWellFormednessOther, ctor.line,
                        "constructor body must contain a delegation or at least one statement"));
                }
                let scope = Scope::build(&ctor.formals, &ctor.body.locals, table)?;
                let typed_delegation = check_delegation(ctor, &class.class_name, &scope, table)?;
                let ctx = BodyCtx { class_name: &class.class_name, return_type: &Type::Void, in_loop: false, in_constructor: true };
                let mut typed_stmts = Vec::with_capacity(ctor.body.stmts.len());
                for stmt in &ctor.body.stmts {
                    typed_stmts.push(check_stmt(stmt, &scope, &ctx, table)?);
                }
                typed_ctors.push(TypedConstructor::Explicit {
                    params: ctor.formals.iter().map(|f| (f.identifier.clone(), f.declared_type.clone())).collect(),
                    delegation: typed_delegation,
                    locals: flatten_locals(&ctor.body.locals),
                    stmts: typed_stmts,
                });
            }
            typed_ctors
        };

        typed_classes.push(TypedClassDecl {
            name: class.class_name.clone(),
            kind: info.kind,
            extends: class.extends.clone(),
            fields: class.fields.iter()
                .flat_map(|d| d.identifiers.iter().map(move |n| (n.clone(), d.declared_type.clone())))
                .collect(),
            constructors: typed_constructors,
            methods: typed_methods,
        });
    }

    Ok(TypedProgram { classes: typed_classes })
}

// ---------------------------------------------------------------------------
// Statement checking
// ---------------------------------------------------------------------------

fn check_stmt(stmt: &Stmt, scope: &Scope, ctx: &BodyCtx, table: &ClassTable) -> Result<TypedStmt, TypeError> {
    Ok(match stmt {
        Stmt::Empty(_) => TypedStmt::Empty,

        Stmt::Assign(name, expr, line) => {
            let binding = resolve_name(name, scope, ctx.class_name, table)
                .ok_or_else(|| TypeError::new(ErrorCode::EUnknownVariable, *line, format!("unknown variable '{}'", name)))?;
            let typed_value = check_expr(expr, scope, ctx, table)?;
            if !assignment_compatible(&typed_of(&typed_value), binding.ty(), table) {
                return Err(TypeError::new(ErrorCode::EAssignTypeMismatch, *line, format!("cannot assign to '{}'", name)));
            }
            TypedStmt::Assign { target: name.clone(), binding, value: typed_value }
        }

        Stmt::Return(expr, line) => {
            if ctx.in_constructor {
                return Err(TypeError::new(ErrorCode::EReturnInConstructor, *line, "constructors may not contain a return statement"));
            }
            if *ctx.return_type == Type::Void {
                return Err(TypeError::new(ErrorCode::EReturnInVoidMethod, *line, "a void method may not contain a return statement"));
            }
            let typed_value = check_expr(expr, scope, ctx, table)?;
            if !assignment_compatible(&typed_of(&typed_value), ctx.return_type, table) {
                return Err(TypeError::new(ErrorCode::EReturnTypeMismatch, *line,
                    "returned expression's type does not match the declared return type"));
            }
            TypedStmt::Return(typed_value)
        }

        Stmt::If(cond, then_b, else_b, line) => {
            let typed_cond = expect_bool(cond, scope, ctx, table, *line)?;
            let typed_then = then_b.iter().map(|s| check_stmt(s, scope, ctx, table)).collect::<Result<_, _>>()?;
            let typed_else = else_b.iter().map(|s| check_stmt(s, scope, ctx, table)).collect::<Result<_, _>>()?;
            TypedStmt::If(typed_cond, typed_then, typed_else)
        }

        Stmt::While(cond, body, line) => {
            let typed_cond = expect_bool(cond, scope, ctx, table, *line)?;
            let inner_ctx = BodyCtx { in_loop: true, ..*ctx };
            let typed_body = body.iter().map(|s| check_stmt(s, scope, &inner_ctx, table)).collect::<Result<_, _>>()?;
            TypedStmt::While(typed_cond, typed_body)
        }

        Stmt::Break(line) => {
            if !ctx.in_loop {
                return Err(TypeError::new(ErrorCode::EBreakOutsideLoop, *line, "'break' outside an enclosing while loop"));
            }
            TypedStmt::Break
        }

        Stmt::CallStmt(call) => {
            let typed_call = check_method_call(call, scope, ctx, table)?;
            if typed_call.return_type != Type::Void {
                return Err(TypeError::new(ErrorCode::ENonvoidCallAsStatement, call.line,
                    format!("result of non-void call to '{}' is discarded", call.method_name)));
            }
            TypedStmt::CallStmt(typed_call)
        }
    })
}

fn expect_bool(expr: &Expr, scope: &Scope, ctx: &BodyCtx, table: &ClassTable, line: u32) -> Result<TypedExpr, TypeError> {
    let typed = check_expr(expr, scope, ctx, table)?;
    match typed_of(&typed) {
        ExprType::Concrete(Type::Bool) => Ok(typed),
        _ => Err(TypeError::new(ErrorCode::ETypeMismatch, line, "condition must be bool")),
    }
}

// ---------------------------------------------------------------------------
// Expression checking
// ---------------------------------------------------------------------------

fn check_expr(expr: &Expr, scope: &Scope, ctx: &BodyCtx, table: &ClassTable) -> Result<TypedExpr, TypeError> {
    Ok(match expr {
        Expr::Num(n, _) => TypedExpr::Num(*n),
        Expr::Bool(b, _) => TypedExpr::Bool(*b),
        Expr::Str(s, _) => TypedExpr::Str(s.clone()),
        Expr::Null(_) => TypedExpr::Null,
        Expr::This(_) => TypedExpr::This(ctx.class_name.to_string()),

        Expr::Var(name, line) => {
            let binding = resolve_name(name, scope, ctx.class_name, table)
                .ok_or_else(|| TypeError::new(ErrorCode::EUnknownVariable, *line, format!("unknown variable '{}'", name)))?;
            TypedExpr::Var { name: name.clone(), binding }
        }

        Expr::New(class_name, actuals, line) => {
            let info = table.get(class_name).ok_or_else(|| TypeError::new(ErrorCode::EUnknownClass, *line, format!("unknown class '{}'", class_name)))?;
            if info.kind == ClassKind::Preamble {
                return Err(TypeError::new(ErrorCode::ETypeCheckOther, *line, format!("'{}' cannot be instantiated directly", class_name)));
            }
            let typed_args = check_constructor_call(&info.own_constructors, actuals, scope, ctx, table, *line, class_name)?;
            TypedExpr::New { class: class_name.clone(), args: typed_args }
        }

        Expr::Call(call) => {
            let typed_call = check_method_call(call, scope, ctx, table)?;
            if typed_call.return_type == Type::Void {
                return Err(TypeError::new(ErrorCode::EVoidCallInExpression, call.line,
                    format!("void call to '{}' used in expression position", call.method_name)));
            }
            TypedExpr::Call(typed_call)
        }

        Expr::Ternary(cond, then_e, else_e, line) => {
            let typed_cond = expect_bool(cond, scope, ctx, table, *line)?;
            let typed_then = check_expr(then_e, scope, ctx, table)?;
            let typed_else = check_expr(else_e, scope, ctx, table)?;
            let ty = combine_ternary_branches(&typed_of(&typed_then), &typed_of(&typed_else), table, *line)?;
            TypedExpr::Ternary { cond: Box::new(typed_cond), then_branch: Box::new(typed_then), else_branch: Box::new(typed_else), ty }
        }

        Expr::Binop(lhs, op, rhs, line) => {
            let typed_lhs = check_expr(lhs, scope, ctx, table)?;
            let typed_rhs = check_expr(rhs, scope, ctx, table)?;
            let ty = check_binop(*op, &typed_of(&typed_lhs), &typed_of(&typed_rhs), *line)?;
            TypedExpr::Binary { lhs: Box::new(typed_lhs), op: *op, rhs: Box::new(typed_rhs), ty }
        }

        Expr::Unop(op, operand, line) => {
            let typed_operand = check_expr(operand, scope, ctx, table)?;
            let ty = check_unop(*op, &typed_of(&typed_operand), *line)?;
            TypedExpr::Unary { op: *op, operand: Box::new(typed_operand), ty }
        }

        Expr::Cast(target, operand, line) => {
            let Type::Class(target_name) = target else {
                return Err(TypeError::new(ErrorCode::ECastTargetNotClass, *line, "cast target must be a class type"));
            };
            if !table.class_exists(target_name) {
                return Err(TypeError::new(ErrorCode::EUnknownClass, *line, format!("unknown class '{}'", target_name)));
            }
            let typed_operand = check_expr(operand, scope, ctx, table)?;
            let direction = match typed_of(&typed_operand) {
                // A cast applied to null always succeeds; no runtime check needed.
                ExprType::NullLiteral => CastDirection::Upcast,
                ExprType::Concrete(Type::Class(source_name)) => {
                    if table.is_subtype(&source_name, target_name) {
                        CastDirection::Upcast
                    } else if table.is_subtype(target_name, &source_name) {
                        CastDirection::Downcast
                    } else {
                        return Err(TypeError::new(ErrorCode::ECastUnrelatedTypes, *line,
                            format!("cannot cast '{}' to unrelated class '{}'", source_name, target_name)));
                    }
                }
                _ => return Err(TypeError::new(ErrorCode::ECastSourceNotClass, *line, "cast source must be a class-typed expression")),
            };
            TypedExpr::Cast { target: target.clone(), operand: Box::new(typed_operand), direction }
        }

        Expr::InstanceOf(operand, class_name, line) => {
            if !table.class_exists(class_name) {
                return Err(TypeError::new(ErrorCode::EUnknownClass, *line, format!("unknown class '{}'", class_name)));
            }
            let typed_operand = check_expr(operand, scope, ctx, table)?;
            match typed_of(&typed_operand) {
                // null is a legal instanceof source -- always false at runtime.
                ExprType::Concrete(Type::Class(_)) | ExprType::NullLiteral => {}
                _ => return Err(TypeError::new(ErrorCode::EInstanceofSourceNotClass, *line, "instanceof source must be a class-typed expression")),
            }
            TypedExpr::InstanceOf { operand: Box::new(typed_operand), class: class_name.clone() }
        }
    })
}

// ---------------------------------------------------------------------------
// Method calls, receivers, and super
// ---------------------------------------------------------------------------

struct ReceiverResolution {
    typed: TypedReceiver,
    /// Irrelevant/empty when `is_super` is true.
    search_class: String,
    is_super: bool,
}

fn resolve_receiver(obj_name: &ObjName, scope: &Scope, ctx: &BodyCtx, table: &ClassTable) -> Result<ReceiverResolution, TypeError> {
    match obj_name {
        ObjName::This(_) => Ok(ReceiverResolution {
            typed: TypedReceiver::This(ctx.class_name.to_string()),
            search_class: ctx.class_name.to_string(),
            is_super: false,
        }),

        ObjName::Super(line) => {
            // super.method() is a method-body form; constructor delegation has
            // its own dedicated form, super(...) (ConstructorDelegation::SuperCall).
            if ctx.in_constructor {
                return Err(TypeError::new(ErrorCode::EInheritanceCheckOther, *line,
                    "super.method() is a method-body form and may not appear in a constructor body"));
            }
            if table.get(ctx.class_name).and_then(|i| i.parent.as_ref()).is_none() {
                return Err(TypeError::new(ErrorCode::ESuperMethodInRootClass, *line, "'super' used in a class with no parent"));
            }
            Ok(ReceiverResolution { typed: TypedReceiver::Super, search_class: String::new(), is_super: true })
        }

        ObjName::Var(name, line) => {
            let binding = resolve_name(name, scope, ctx.class_name, table)
                .ok_or_else(|| TypeError::new(ErrorCode::EUnknownVariable, *line, format!("unknown variable '{}'", name)))?;
            let Type::Class(c) = binding.ty().clone() else {
                return Err(TypeError::new(ErrorCode::EReceiverNotClassType, *line, "receiver is not class-typed"));
            };
            Ok(ReceiverResolution { typed: TypedReceiver::Var { name: name.clone(), binding }, search_class: c, is_super: false })
        }

        ObjName::Computed(expr, line) => {
            if matches!(**expr, Expr::Null(_)) {
                return Err(TypeError::new(ErrorCode::ENullLiteralReceiver, *line, "receiver cannot be the literal 'null'"));
            }
            let typed_expr = check_expr(expr, scope, ctx, table)?;
            match typed_of(&typed_expr) {
                ExprType::Concrete(Type::Class(c)) => Ok(ReceiverResolution {
                    typed: TypedReceiver::Computed(Box::new(typed_expr)),
                    search_class: c,
                    is_super: false,
                }),
                _ => Err(TypeError::new(ErrorCode::EReceiverNotClassType, *line, "receiver is not class-typed")),
            }
        }
    }
}

fn check_method_call(call: &MethodCall, scope: &Scope, ctx: &BodyCtx, table: &ClassTable) -> Result<TypedMethodCall, TypeError> {
    let r = resolve_receiver(&call.obj_name, scope, ctx, table)?;

    let (owner, sig) = if r.is_super {
        let parent = table.get(ctx.class_name).and_then(|i| i.parent.as_deref()).unwrap();
        table.get(parent).unwrap().effective_methods.get(&call.method_name).cloned()
            .ok_or_else(|| TypeError::new(ErrorCode::ESuperMethodUnresolved, call.line, format!("no ancestor declares method '{}'", call.method_name)))?
    } else {
        let info = table.get(&r.search_class).unwrap();
        info.effective_methods.get(&call.method_name).cloned()
            .ok_or_else(|| TypeError::new(ErrorCode::EUnknownMethod, call.line, format!("unknown method '{}' on class '{}'", call.method_name, r.search_class)))?
    };

    let typed_args = check_actuals(&sig.params, &call.actuals, scope, ctx, table, call.line, &sig.name)?;
    Ok(TypedMethodCall { receiver: r.typed, name: call.method_name.clone(), owner, args: typed_args, return_type: sig.return_type })
}

fn check_actuals(
    formal_types: &[Type], actuals: &[Expr], scope: &Scope, ctx: &BodyCtx, table: &ClassTable, line: u32, what: &str,
) -> Result<Vec<TypedExpr>, TypeError> {
    if formal_types.len() != actuals.len() {
        return Err(TypeError::new(ErrorCode::EArityMismatch, line,
            format!("'{}' expects {} argument(s), got {}", what, formal_types.len(), actuals.len())));
    }
    let mut typed_args = Vec::with_capacity(actuals.len());
    for (formal_ty, actual) in formal_types.iter().zip(actuals) {
        let typed_arg = check_expr(actual, scope, ctx, table)?;
        if !assignment_compatible(&typed_of(&typed_arg), formal_ty, table) {
            return Err(TypeError::new(ErrorCode::EActualTypeMismatch, line, format!("argument type does not match formal type in call to '{}'", what)));
        }
        typed_args.push(typed_arg);
    }
    Ok(typed_args)
}

fn check_constructor_call(
    ctors: &[ConstructorSig], actuals: &[Expr], scope: &Scope, ctx: &BodyCtx, table: &ClassTable, line: u32, class_name: &str,
) -> Result<Vec<TypedExpr>, TypeError> {
    let ctor = ctors.iter().find(|c| c.arity == actuals.len())
        .ok_or_else(|| TypeError::new(ErrorCode::EArityMismatch, line, format!("no constructor of class '{}' takes {} argument(s)", class_name, actuals.len())))?;
    check_actuals(&ctor.params, actuals, scope, ctx, table, line, class_name)
}

// ---------------------------------------------------------------------------
// Constructor delegation
// ---------------------------------------------------------------------------

fn check_delegation_cycle(class_name: &str, table: &ClassTable) -> Result<(), TypeError> {
    let info = table.get(class_name).unwrap();
    let arities: Vec<usize> = info.own_constructors.iter().map(|c| c.arity).collect();
    let this_edges = |arity: &usize| -> Vec<usize> {
        info.own_constructors.iter()
            .find(|c| c.arity == *arity)
            .and_then(|c| c.this_target_arity)
            .into_iter().collect()
    };
    if find_cycle(arities.iter(), this_edges).is_some() {
        let line = info.own_constructors.iter().find(|c| c.this_target_arity.is_some())
            .map(|c| c.line).unwrap_or(info.decl_line);
        return Err(TypeError::new(ErrorCode::EDelegationCycle, line, format!("constructor delegation in class '{}' forms a cycle", class_name)));
    }
    Ok(())
}

fn check_delegation(ctor: &ConstructorDecl, class_name: &str, scope: &Scope, table: &ClassTable) -> Result<Option<TypedDelegation>, TypeError> {
    let info = table.get(class_name).unwrap();
    let inheriting = info.parent.is_some();

    match &ctor.delegation {
        None => {
            if inheriting {
                return Err(TypeError::new(ErrorCode::EInheritanceCheckOther, ctor.line,
                    "constructor of an inheriting class must start with super(...) or this(...)"));
            }
            Ok(None)
        }
        Some(ConstructorDelegation::SuperCall(args, line)) => {
            let parent = info.parent.as_ref().ok_or_else(|| TypeError::new(ErrorCode::ESuperInRootClass, *line, "super(...) used in a class with no parent"))?;
            let parent_info = table.get(parent).unwrap();
            let target = parent_info.own_constructors.iter().find(|c| c.arity == args.len())
                .ok_or_else(|| TypeError::new(ErrorCode::EDelegationArityMismatch, *line, format!("'{}' has no constructor of arity {}", parent, args.len())))?;
            let ctx = BodyCtx { class_name, return_type: &Type::Void, in_loop: false, in_constructor: true };
            let typed_args = check_actuals(&target.params, args, scope, &ctx, table, *line, parent)?;
            Ok(Some(TypedDelegation::Super { args: typed_args }))
        }
        Some(ConstructorDelegation::ThisCall(args, line)) => {
            let target = info.own_constructors.iter().find(|c| c.arity == args.len())
                .ok_or_else(|| TypeError::new(ErrorCode::EDelegationArityMismatch, *line, format!("class '{}' has no constructor of arity {}", class_name, args.len())))?;
            let ctx = BodyCtx { class_name, return_type: &Type::Void, in_loop: false, in_constructor: true };
            let typed_args = check_actuals(&target.params, args, scope, &ctx, table, *line, class_name)?;
            Ok(Some(TypedDelegation::This { args: typed_args }))
        }
    }
}

// ---------------------------------------------------------------------------
// Compatibility and operator typing
// ---------------------------------------------------------------------------

fn assignment_compatible(from: &ExprType, to: &Type, table: &ClassTable) -> bool {
    match from {
        ExprType::NullLiteral => matches!(to, Type::Class(_)),
        ExprType::Concrete(Type::Class(a)) => matches!(to, Type::Class(b) if table.is_subtype(a, b)),
        ExprType::Concrete(t) => t == to,
    }
}

fn combine_ternary_branches(a: &ExprType, b: &ExprType, table: &ClassTable, line: u32) -> Result<Type, TypeError> {
    use ExprType::*;
    let mismatch = || TypeError::new(ErrorCode::EConditionalTypeMismatch, line, "ternary branches have incompatible types");
    match (a, b) {
        (NullLiteral, NullLiteral) => Err(mismatch()),
        (NullLiteral, Concrete(t @ Type::Class(_))) | (Concrete(t @ Type::Class(_)), NullLiteral) => Ok(t.clone()),
        (Concrete(t1), Concrete(t2)) if t1 == t2 => Ok(t1.clone()),
        (Concrete(Type::Class(c1)), Concrete(Type::Class(c2))) => least_common_ancestor(c1, c2, table).map(Type::Class).ok_or_else(mismatch),
        _ => Err(mismatch()),
    }
}

fn check_binop(op: Binop, lhs: &ExprType, rhs: &ExprType, line: u32) -> Result<Type, TypeError> {
    use Binop::*;
    let (ExprType::Concrete(l), ExprType::Concrete(r)) = (lhs, rhs) else {
        return Err(TypeError::new(ErrorCode::EBinopTypeMismatch, line, "null is not a legal operand"));
    };
    let mismatch = || TypeError::new(ErrorCode::EBinopTypeMismatch, line, "operand type mismatch");
    match op {
        Add | Sub | Mul | Div | Mod => {
            if l == &Type::Int && r == &Type::Int { Ok(Type::Int) }
            else if op == Add && l == &Type::String && r == &Type::String { Ok(Type::String) }
            else if op == Mul && l == &Type::String && r == &Type::Int { Ok(Type::String) }
            else { Err(mismatch()) }
        }
        And | Or => if l == &Type::Bool && r == &Type::Bool { Ok(Type::Bool) } else { Err(mismatch()) },
        Lt | Gt | Eq => {
            if l == r && matches!(l, Type::Int | Type::String) { Ok(Type::Bool) } else { Err(mismatch()) }
        }
    }
}

fn check_unop(op: Unop, operand: &ExprType, line: u32) -> Result<Type, TypeError> {
    let ExprType::Concrete(t) = operand else {
        return Err(TypeError::new(ErrorCode::EUnopTypeMismatch, line, "null is not a legal operand"));
    };
    match (op, t) {
        (Unop::Not, Type::Bool) => Ok(Type::Bool),
        (Unop::Neg, Type::Int) => Ok(Type::Int),
        (Unop::Neg, Type::String) => Ok(Type::String), // `~` also means string reversal
        _ => Err(TypeError::new(ErrorCode::EUnopTypeMismatch, line, "operand type mismatch")),
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn check_program(program: Program) -> Result<(TypedProgram, ClassTable), TypeError> {
    let program = crate::add_io_classes::add_io_classes(program);
    let mut table = gather_declarations(&program)?;
    resolve_inheritance(&mut table)?;
    check_entry_point(&table)?;
    let typed = check_bodies(&program, &table)?;
    Ok((typed, table))
}
