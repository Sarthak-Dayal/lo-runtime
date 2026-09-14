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
