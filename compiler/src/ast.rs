#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Int,
    Bool,
    String,
    Void,
    Class(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub classes: Vec<ClassDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub declared_type: Type,
    pub name: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassDecl {
    pub name: String,
    pub extends: Option<String>,
    pub fields: Vec<Param>,
    pub constructors: Vec<ConstructorDecl>,
    pub methods: Vec<MethodDecl>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstructorDecl {
    pub params: Vec<Param>,
    pub other_constructor_call: Option<OtherConstructorCall>,
    pub body: BodyScope,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodDecl {
    pub return_type: Type,
    pub name: String,
    pub params: Vec<Param>,
    pub body: MethodBody,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtherConstructorCall {
    ThisCall(Vec<Expr>, u32),
    SuperCall(Vec<Expr>, u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MethodBody {
    // Real, source-written method bodies.
    UserDefined(BodyScope),
    // Never produced by the parser — injected later via the synthetic
    // Input/Output preamble classes.
    Io(IoOp),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IoOp {
    ReadInt,
    ReadBool,
    ReadString,
    Eof,
    PrintInt,
    PrintBool,
    PrintString,
    Println,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarDecl {
    pub declared_type: Type,
    pub names: Vec<String>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyScope {
    pub locals: Vec<VarDecl>,
    pub stmts: Vec<Stmt>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum Stmt {
    Assign(String, Expr, u32),
    Return(Expr, u32),
    If(Expr, Vec<Stmt>, Vec<Stmt>, u32),
    While(Expr, Vec<Stmt>, u32),
    Break(u32),
    Empty(u32),
    CallStmt(MethodCall),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodCall {
    pub receiver: Receiver,
    pub name: String,
    pub args: Vec<Expr>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Num(i32, u32),
    Bool(bool, u32),
    Str(String, u32),
    Var(String, u32),
    This(u32),
    Null(u32),
    New(String, Vec<Expr>, u32),
    Call(MethodCall),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>, u32),
    Binary(Box<Expr>, BinaryOp, Box<Expr>, u32),
    Unary(UnaryOp, Box<Expr>, u32),
    Cast(Type, Box<Expr>, u32),
    InstanceOf(Box<Expr>, String, u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Receiver {
    Var(String, u32),
    This(u32),
    Super(u32),
    Computed(Box<Expr>, u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    And,
    Or,
    Lt,
    Gt,
    Eq,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnaryOp {
    Not,
    Neg,
}
