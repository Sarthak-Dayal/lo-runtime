# Type Checker Implementation Plan — LO (LiveOak) P1

Implements everything decided in `type_checker_design.md`, against the `compiler/`
crate as it exists on `parser/phase-1-implementation` (`ast.rs`, `lexer.rs`,
`parser.rs`, `token.rs`, `main.rs`). Adds one new file. `ast.rs` stays untouched —
the typed AST introduced this revision is a wholly separate set of types living in
`sema.rs`, not a modification of the parser's output type.

**Revision note:** this supersedes the previous draft after a 23-comment team review
across two reviewers. The biggest change is architectural (the checker now produces
a genuine `TypedProgram`, not the same untyped tree wrapped in a marker); the rest are
concrete, confirmed bugs the review found, plus a real algorithmic fix to the ternary
type rule (least common ancestor, not a bidirectional subtype check). Every fix below
is cross-referenced to what was wrong before

---

## Files to create / modify

| File | Change |
|---|---|
| `compiler/src/sema.rs` | New. Everything below. |
| `compiler/src/ast.rs` | **Untouched**, still. The typed-AST decision resurrects some of what an even earlier draft tried to do via `Cell` fields on `ast.rs` — but as a *separate* type family in `sema.rs`, not by mutating the parser's types. See design doc's "Alternate designs" for why those are two different objections and only one of them still applies. |
| `compiler/src/main.rs` | Add `mod sema;` and wire `sema::check_program` after parsing. |

---

## Decisions resolved for this implementation

| Item | Decision |
|---|---|
| Typed AST vs. `Checked(Program)` | **Reversed from the previous draft** — see design doc. `check_program(Program) -> Result<(TypedProgram, ClassTable), TypeError>`. |
| Preamble injection: inside or outside `check_program` | **Reversed from the previous draft's design doc** (the implementation already did this; the design doc said otherwise, and that inconsistency itself was one of the review comments). Settled: inside. There is exactly one way to obtain a `TypedProgram`, and it always includes the preamble. |
| Ternary result type | **Corrected, not just resolved.** Least common ancestor over the precomputed ancestor chains, not a bidirectional `is_subtype` check. The old check was a confirmed bug: `Cat`/`Dog` siblings under `Animal` would incorrectly fail with `E_CONDITIONAL_TYPE_MISMATCH` instead of resolving to `Animal`. See `least_common_ancestor` below. |
| Cast applied to `null` | **Fixed — confirmed bug.** §4.4.4 says this always succeeds, typed as the target, no runtime check. The previous draft's pattern match on `ExprType::Concrete(Type::Class(_))` for cast sources rejected `NullLiteral` outright. Now accepted, classified `CastDirection::Upcast` (see design doc for why that's the right direction to report for a null source). |
| `instanceof` with a `null` source | **Fixed — confirmed bug.** §4.3.6 says this is legal, always `false` at runtime. Same missing-`NullLiteral`-arm bug as the cast case. |
| `E_VOID_CALL_IN_EXPRESSION` | **Fixed — a claimed check that was never actually implemented.** The design doc's Algorithm 9 always described both directions of the void/non-void duality as checked at their respective call sites; the code only ever implemented the statement side (`E_NONVOID_CALL_AS_STATEMENT`). `check_expr`'s `Expr::Call` handling now checks the other direction explicitly. |
| Return-path completeness | **Corrected.** Rule is "does *any* statement in the sequence definitely return," not "does the *last* one." See `definitely_returns` below — this was a real algorithmic error, not a style choice, per course lecture material. |
| Local variable declared types | **Fixed — confirmed gap.** `Scope::build` never validated a local's declared type at all — an unknown class (`Foo x;` for nonexistent `Foo`) or `void` (`void x;`) both passed silently. `Scope::build` now takes `&ClassTable` and runs the same `check_type_reference` Pass 2a uses, plus rejects `Type::Void` before inserting a name. `E_LOCAL_TYPED_VOID` is invented (see design doc Notes) since no dedicated code exists. |
| Duplicate formal parameter names | **Fixed — confirmed gap, and a real correctness bug, not just a missing diagnostic.** Formals were inserted into `Scope`'s `HashMap` by name with no duplicate check; `void foo(int x, int x)` silently kept only the second `x`'s binding. Fixed in Pass 1 (`gather_declarations`), consolidated into one `check_formals_well_formed` helper shared by method and constructor parameter lists — Pass 1 already walks every parameter list once for void/reserved-name checks, so the duplicate check belongs there, not in a second walk in `Scope::build`. `E_DUPLICATE_FORMAL` is invented (matches a gap `parser_design.md` independently flagged). |
| Empty explicit constructor bodies | **Fixed — confirmed gap.** LO-3 §3.3.4: a constructor body with no delegation and no statements is a compile error, even though the grammar's `(Stmt)*` permits it syntactically. Checked once per explicit constructor in `check_bodies`, filed under `E_WELL_FORMEDNESS_OTHER` (no dedicated code exists). |
| `new Input()` / `new Output()` | **Fixed — confirmed gap.** §4.6: only the synthesized wrapper instantiates the preamble classes. Every declared class now carries a `ClassKind` (`User`/`Preamble`); `Expr::New` rejects a `Preamble`-kind target, filed under `E_TYPE_CHECK_OTHER`. |
| `extends Input`/`extends Output` | **Fixed — confirmed gap.** Checked in `resolve_inheritance` alongside the existing extends-target-resolves check, filed under `E_INHERITANCE_CHECK_OTHER`. |
| `extends Main` | **Fixed — confirmed gap.** The prior draft only checked that *Main itself* has no `extends` clause (`E_MAIN_CLASS_EXTENDS`); §3.4.6 states the rule in both directions — nothing may extend Main either. Checked in the same place as the Preamble-extends check, filed under `E_ENTRY_POINT_OTHER` since it's about protecting Main's role specifically. |
| `super.method(...)` inside a constructor body | **Fixed — confirmed gap.** §4.1 introduces `super.method(...)` specifically as a method-body form; constructor-body delegation has its own dedicated form, `super(...)` (no dot). Nothing in the grammar stops a constructor's own statement list from containing an ordinary `MethodCall` with `Receiver::Super`, so this needed an explicit `ctx.in_constructor` check inside `resolve_receiver`'s `Super` arm — it does not fall out of anything else. Filed under `E_INHERITANCE_CHECK_OTHER`. |
| Pass structure vs. the vocabulary's own §5 phase taxonomy | **Considered, not adopted** — see design doc Open item 6 for the full reasoning (data-dependency order vs. error-kind categorization genuinely don't coincide). The rule-mapping table in the design doc is the cross-reference the suggestion was really asking for. |
| Struct field ordering (`line` last) | Minor, but fixed: `ConstructorSig` had `line` in the middle; every `ast.rs` struct puts `line` last, and this document's own structs now match. |

---

## `src/sema.rs`

### Error type

```rust
use crate::ast::*;
use std::collections::{HashMap, HashSet};

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
    EDuplicateFormal,        // invented — see design doc Notes
    EDuplicateConstructorArity,
    EFieldTypedVoid,
    EFormalTypedVoid,
    ELocalTypedVoid,         // invented — see design doc Notes
    EReturnInVoidMethod,
    EReturnMissing,
    EReturnInConstructor,
    EBreakOutsideLoop,       // invented — see design doc Notes
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

/// Internal comparison currency during checking — never part of the typed AST
/// itself. `null` has no `Type` of its own but is compatible with any class type;
/// every compatibility check (assignment, return, actual-argument, ternary) matches
/// on this instead of comparing `Type` values directly.
#[derive(Debug, Clone, PartialEq)]
enum ExprType {
    Concrete(Type),
    NullLiteral,
}
```

### The typed AST

New this revision, replacing the previous draft's `Checked(Program)` entirely — see
design doc for the full rationale. Every field here is non-optional by construction:
a node only exists in this tree once whatever it represents has already been fully
checked.

```rust
pub struct TypedProgram {
    pub classes: Vec<TypedClassDecl>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ClassKind { User, Preamble }

pub struct TypedClassDecl {
    pub name: String,
    pub kind: ClassKind,
    pub extends: Option<String>,
    pub fields: Vec<Param>,
    pub constructors: Vec<TypedConstructor>,
    pub methods: Vec<TypedMethodDecl>,
}

pub enum TypedConstructor {
    Explicit {
        params: Vec<Param>,
        delegation: Option<TypedDelegation>,
        locals: Vec<(String, Type)>,
        stmts: Vec<TypedStmt>,
    },
    Implicit {
        fields: Vec<(String, Type)>, // this.field_i = formal_i, in field order
    },
}

pub enum TypedDelegation {
    This { args: Vec<TypedExpr> },
    Super { args: Vec<TypedExpr> },
}

pub struct TypedMethodDecl {
    pub name: String,
    pub return_type: Type,
    pub params: Vec<Param>,
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
    pub owner: String,           // the class whose effective_methods entry resolved this
    pub args: Vec<TypedExpr>,
    pub return_type: Type,
}

pub enum TypedReceiver {
    This(String),                              // enclosing class name
    Super,                                     // owner lives on the enclosing TypedMethodCall
    Var { name: String, binding: BindingInfo },
    Computed(Box<TypedExpr>),
}

pub enum TypedExpr {
    Num(i32),
    Bool(bool),
    Str(String),
    Null,                                       // deliberately no Type — see design doc
    This(String),
    Var { name: String, binding: BindingInfo },
    New { class: String, args: Vec<TypedExpr> },
    Call(TypedMethodCall),
    Ternary { cond: Box<TypedExpr>, then_branch: Box<TypedExpr>, else_branch: Box<TypedExpr>, ty: Type },
    Binary { lhs: Box<TypedExpr>, op: BinaryOp, rhs: Box<TypedExpr>, ty: Type },
    Unary { op: UnaryOp, operand: Box<TypedExpr>, ty: Type },
    Cast { target: Type, operand: Box<TypedExpr>, direction: CastDirection },
    InstanceOf { operand: Box<TypedExpr>, class: String },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CastDirection { Upcast, Downcast }

/// The type of a checked `TypedExpr`, as `ExprType` (never `Option`-wrapped on the
/// node itself — this is purely a read-back helper for compatibility checks).
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
```

### Class table

`ClassTable`'s useful surface stays `pub` so a future codegen phase can reuse
`is_subtype`, `least_common_ancestor`-shaped lookups, and `effective_methods` for
vtable slot assignment, without re-deriving them from scratch.

```rust
pub struct ClassTable {
    classes: HashMap<String, ClassInfo>,
    order: Vec<String>, // source declaration order
}

pub struct ClassInfo {
    pub decl_line: u32,
    pub kind: ClassKind,
    pub parent: Option<String>,
    own_fields: Vec<(String, Type, u32)>,
    own_methods: Vec<MethodSig>,
    own_constructors: Vec<ConstructorSig>,

    pub ancestors: Vec<String>,                       // does NOT include self, root-terminated
    pub effective_fields: Vec<(String, Type, String)>, // (name, type, owner), parent-first
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
    this_target_arity: Option<usize>, // Some(n) iff this ctor's own delegation is
                                       // this(...) targeting the n-arity constructor
                                       // of the same class — see check_delegation_cycle.
    line: u32,                        // last, matching every ast.rs struct's convention
}

impl ClassTable {
    pub fn get(&self, name: &str) -> Option<&ClassInfo> {
        self.classes.get(name)
    }

    pub fn class_exists(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    /// `a <: b` — is `a` `b` itself or a descendant of it. Never called with
    /// `Type::String` on either side; callers guard that before reaching here.
    pub fn is_subtype(&self, a: &str, b: &str) -> bool {
        a == b || self.classes.get(a).is_some_and(|info| info.ancestors.iter().any(|anc| anc == b))
    }
}
```

### Least common ancestor (design doc, Algorithms §7)

```rust
/// `b`'s own chain, walked from `b` upward, is monotonically "more general" moving
/// away from `b` — so the first class in that walk that also appears anywhere in
/// `a`'s chain is the *closest* common ancestor, not just *a* common one. Correct
/// because LO-4 inheritance is a forest of simple upward chains (single inheritance,
/// no diamonds). `None` means `a` and `b` are in genuinely disjoint hierarchies.
fn least_common_ancestor(a: &str, b: &str, table: &ClassTable) -> Option<String> {
    let chain_a: HashSet<&str> = std::iter::once(a)
        .chain(table.get(a)?.ancestors.iter().map(String::as_str))
        .collect();
    std::iter::once(b)
        .chain(table.get(b)?.ancestors.iter().map(String::as_str))
        .find(|c| chain_a.contains(c))
        .map(String::from)
}
```

### Generic cycle detector (unchanged from the previous draft — design doc, Algorithms §4)

```rust
/// DFS with a "currently on this path" set. Returns the first back-edge found, as
/// `(from, to)`, or `None` if acyclic. Used for both `extends` cycles and `this(...)`
/// delegation cycles.
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
            return None; // already fully explored via another path, known acyclic
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

### Preamble injection

Unchanged in mechanism from the previous draft — the change is that `check_program`
now calls this itself (see "Entry point" below) rather than requiring a caller to
remember to.

```rust
/// Synthesizes `Input`/`Output` as ordinary `ClassDecl`s with `MethodBody::Io`
/// bodies, and prepends them to `program.classes`. Line `0` marks a synthesized
/// declaration — never emitted by the parser (the lexer starts counting at line 1) —
/// which `gather_declarations` uses both to exempt these two from the reserved-name
/// check and to mark them `ClassKind::Preamble`.
fn inject_preamble(program: &mut Program) {
    let io_method = |name: &str, return_type: Type, params: Vec<Param>, op: IoOp| MethodDecl {
        return_type,
        name: name.to_string(),
        params,
        body: MethodBody::Io(op),
        line: 0,
    };

    let input = ClassDecl {
        name: "Input".to_string(),
        extends: None,
        fields: vec![],
        constructors: vec![],
        methods: vec![
            io_method("read_int", Type::Int, vec![], IoOp::ReadInt),
            io_method("read_bool", Type::Bool, vec![], IoOp::ReadBool),
            io_method("read_string", Type::String, vec![], IoOp::ReadString),
            io_method("eof", Type::Bool, vec![], IoOp::Eof),
        ],
        line: 0,
    };

    let string_param = |name: &str| Param { declared_type: Type::String, name: name.to_string(), line: 0 };
    let output = ClassDecl {
        name: "Output".to_string(),
        extends: None,
        fields: vec![],
        constructors: vec![],
        methods: vec![
            io_method("print_int", Type::Void, vec![Param { declared_type: Type::Int, name: "n".into(), line: 0 }], IoOp::PrintInt),
            io_method("print_bool", Type::Void, vec![Param { declared_type: Type::Bool, name: "b".into(), line: 0 }], IoOp::PrintBool),
            io_method("print_string", Type::Void, vec![string_param("s")], IoOp::PrintString),
            io_method("println", Type::Void, vec![], IoOp::Println),
        ],
        line: 0,
    };

    program.classes.insert(0, output);
    program.classes.insert(0, input);
}

/// The three pre-bound program-scope names. Checked as a final resolution tier,
/// after locals/formals/fields all miss.
fn preamble_binding(name: &str) -> Option<Type> {
    match name {
        "in" => Some(Type::Class("Input".to_string())),
        "out" | "err" => Some(Type::Class("Output".to_string())),
        _ => None,
    }
}
```

### Pass 1 — `gather_declarations`

```rust
const RESERVED_CLASS_NAMES: &[&str] = &["Input", "Output"]; // Main is permitted, shape-checked separately
const RESERVED_VAR_NAMES: &[&str] = &["in", "out", "err"];

fn check_not_reserved_var_name(name: &str, line: u32) -> Result<(), TypeError> {
    if RESERVED_VAR_NAMES.contains(&name) {
        return Err(TypeError::new(ErrorCode::EReservedVariableName, line,
            format!("'{}' is a reserved name", name)));
    }
    Ok(())
}

/// Shared by method and constructor parameter lists: no void-typed formal, no
/// reserved name, no duplicate name within this one list. Consolidated into one
/// helper (this revision) rather than two copy-pasted loops, which is also what
/// let the missing duplicate-formal check (`E_DUPLICATE_FORMAL`, invented — see
/// design doc Notes) get added in one place instead of two.
fn check_formals_well_formed(params: &[Param]) -> Result<(), TypeError> {
    let mut seen = HashSet::new();
    for p in params {
        if p.declared_type == Type::Void {
            return Err(TypeError::new(ErrorCode::EFormalTypedVoid, p.line,
                format!("formal '{}' cannot have type void", p.name)));
        }
        check_not_reserved_var_name(&p.name, p.line)?;
        if !seen.insert(p.name.clone()) {
            return Err(TypeError::new(ErrorCode::EDuplicateFormal, p.line,
                format!("duplicate formal parameter '{}'", p.name)));
        }
    }
    Ok(())
}

fn gather_declarations(program: &Program) -> Result<ClassTable, TypeError> {
    let mut classes = HashMap::new();
    let mut order = Vec::new();

    for class in &program.classes {
        // Reserved-name check MUST run before the duplicate-name check: both Input
        // and Output are already present in `classes` by the time any user class is
        // visited (inject_preamble always runs first), so a user's `class Input(){}`
        // would otherwise collide with EDuplicateClassName before this arm is ever
        // reached — the wrong code for what the vocabulary specifically names
        // E_RESERVED_CLASS_NAME.
        if RESERVED_CLASS_NAMES.contains(&class.name.as_str()) && class.line != 0 {
            return Err(TypeError::new(ErrorCode::EReservedClassName, class.line,
                format!("class name '{}' is reserved", class.name)));
        }
        if classes.contains_key(&class.name) {
            return Err(TypeError::new(ErrorCode::EDuplicateClassName, class.line,
                format!("class '{}' declared more than once", class.name)));
        }

        let mut own_fields = Vec::new();
        for field in &class.fields {
            if field.declared_type == Type::Void {
                return Err(TypeError::new(ErrorCode::EFieldTypedVoid, field.line,
                    format!("field '{}' cannot have type void", field.name)));
            }
            check_not_reserved_var_name(&field.name, field.line)?;
            if own_fields.iter().any(|(n, ..): &(String, Type, u32)| n == &field.name) {
                return Err(TypeError::new(ErrorCode::EDuplicateField, field.line,
                    format!("duplicate field '{}'", field.name)));
            }
            own_fields.push((field.name.clone(), field.declared_type.clone(), field.line));
        }

        let mut own_methods: Vec<MethodSig> = Vec::new();
        for method in &class.methods {
            if own_methods.iter().any(|m| m.name == method.name) {
                return Err(TypeError::new(ErrorCode::EDuplicateMethod, method.line,
                    format!("duplicate method '{}'", method.name)));
            }
            check_formals_well_formed(&method.params)?;
            own_methods.push(MethodSig {
                name: method.name.clone(),
                params: method.params.iter().map(|p| p.declared_type.clone()).collect(),
                return_type: method.return_type.clone(),
                line: method.line,
            });
        }

        let mut own_constructors: Vec<ConstructorSig> = Vec::new();
        for ctor in &class.constructors {
            let arity = ctor.params.len();
            if own_constructors.iter().any(|c| c.arity == arity) {
                return Err(TypeError::new(ErrorCode::EDuplicateConstructorArity, ctor.line,
                    format!("class '{}' already has a constructor of arity {}", class.name, arity)));
            }
            check_formals_well_formed(&ctor.params)?;
            let this_target_arity = match &ctor.delegation {
                Some(ConstructorDelegation::ThisCall(args, _)) => Some(args.len()),
                _ => None,
            };
            own_constructors.push(ConstructorSig {
                arity,
                params: ctor.params.iter().map(|p| p.declared_type.clone()).collect(),
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
                this_target_arity: None, // implicit constructors never delegate
                line: class.line,
            });
        }

        order.push(class.name.clone());
        classes.insert(class.name.clone(), ClassInfo {
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
```

Locals aren't reachable from `gather_declarations` at all — `BodyScope` lives inside
`MethodDecl.body`/`ConstructorDecl.body`, which this pass never opens. Their
reserved-name, void, and type-reference checks all happen in `Scope::build` (Pass 4)
instead — see below.

### Pass 2 — `resolve_inheritance`

```rust
fn resolve_inheritance(table: &mut ClassTable) -> Result<(), TypeError> {
    // Iterates `table.order` (source declaration order), NOT `table.classes.keys()`
    // — a `HashMap`'s key order is unspecified, which would make *which* error comes
    // back nondeterministic across runs for a program with more than one violation.
    let names: Vec<String> = table.order.clone();
    for name in &names {
        let info = table.classes.get(name).unwrap();
        if let Some(parent) = &info.parent {
            if !table.class_exists(parent) {
                return Err(TypeError::new(ErrorCode::EUnknownClass, info.decl_line,
                    format!("class '{}' extends unknown class '{}'", name, parent)));
            }
            // Nothing may extend Main (§3.4.6) or a non-extensible preamble class
            // (§4.6) — both gaps found in review, neither checked before this pass.
            if parent == "Main" {
                return Err(TypeError::new(ErrorCode::EEntryPointOther, info.decl_line,
                    format!("class '{}' may not extend 'Main'", name)));
            }
            if table.classes[parent].kind == ClassKind::Preamble {
                return Err(TypeError::new(ErrorCode::EInheritanceCheckOther, info.decl_line,
                    format!("class '{}' may not extend the non-extensible class '{}'", name, parent)));
            }
        }
        if info.parent.is_some() && info.own_constructors.is_empty() {
            return Err(TypeError::new(ErrorCode::EMissingConstructorInInheritingClass,
                info.decl_line,
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
        return Err(TypeError::new(ErrorCode::EInheritanceCycle, line,
            format!("inheritance cycle involving class '{}'", from)));
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
            return Err(TypeError::new(ErrorCode::EUnknownClass, line,
                format!("unknown class '{}' referenced in '{}'", name, ctx)));
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
            return Err(TypeError::new(ErrorCode::EFieldShadowing, *fline,
                format!("field '{}' shadows an inherited field", fname)));
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
```

### Pass 3 — `check_entry_point`

Unchanged from the previous draft.

```rust
fn check_entry_point(table: &ClassTable) -> Result<(), TypeError> {
    let main = table.get("Main").ok_or_else(|| {
        TypeError::new(ErrorCode::ENoMainClass, 0, "no class named 'Main' declared")
    })?;

    if main.parent.is_some() {
        return Err(TypeError::new(ErrorCode::EMainClassExtends, main.decl_line,
            "'Main' must not have an extends clause"));
    }

    match main.own_methods.iter().find(|m| m.name == "main") {
        None => return Err(TypeError::new(ErrorCode::ENoMainMethod, main.decl_line,
            "'Main' must declare 'int main()'")),
        Some(m) if m.return_type != Type::Int || !m.params.is_empty() => {
            return Err(TypeError::new(ErrorCode::EMainMethodSignature, m.line,
                "'main' must return int and take no formals"));
        }
        _ => {}
    }

    if !main.own_constructors.iter().any(|c| c.arity == 0) {
        return Err(TypeError::new(ErrorCode::EMainNoZeroArgConstructor, main.decl_line,
            "'Main' must have a zero-arg constructor"));
    }

    Ok(())
}
```

### `Scope` and name resolution

```rust
struct Scope {
    bindings: HashMap<String, Type>,
    formal_names: HashSet<String>,
}

impl Scope {
    /// Now takes `&ClassTable` — a confirmed gap in the previous draft, which
    /// inserted `decl.declared_type` straight into the map with no validation at
    /// all, so an unknown class (`Foo x;`, `Foo` never declared) or `void` (`void
    /// x;`) both passed silently. Duplicate FORMAL names are Pass 1's job
    /// (`check_formals_well_formed`), not rechecked here.
    fn build(params: &[Param], locals: &[VarDecl], table: &ClassTable) -> Result<Scope, TypeError> {
        let mut bindings = HashMap::new();
        let mut formal_names = HashSet::new();
        for p in params {
            bindings.insert(p.name.clone(), p.declared_type.clone());
            formal_names.insert(p.name.clone());
        }
        // BodyScope.locals is already pairwise-distinct within itself (parser-level
        // E_DUPLICATE_LOCAL) and each VarDecl may name several identifiers sharing
        // one type ("int a, b;") — flatten per name.
        for decl in locals {
            if decl.declared_type == Type::Void {
                return Err(TypeError::new(ErrorCode::ELocalTypedVoid, decl.line,
                    "a local variable cannot have type void"));
            }
            check_type_reference(&decl.declared_type, table, decl.line, "local declaration")?;
            for name in &decl.names {
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

/// Locals/formals → fields → `in`/`out`/`err`, first match wins. Returns
/// `BindingInfo` (type *and* kind — this revision's change) so callers can put the
/// binding kind directly on the typed node without a second lookup.
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
```

### Pass 4 — body checking

```rust
#[derive(Clone, Copy)]
struct BodyCtx<'a> {
    class_name: &'a str,
    return_type: &'a Type,
    in_loop: bool,
    in_constructor: bool,
}

fn check_bodies(program: &Program, table: &ClassTable) -> Result<TypedProgram, TypeError> {
    let mut typed_classes = Vec::with_capacity(program.classes.len());

    for class in &program.classes {
        let info = table.get(&class.name).unwrap();

        let mut typed_methods = Vec::with_capacity(class.methods.len());
        for method in &class.methods {
            let body = match &method.body {
                MethodBody::Io(op) => {
                    typed_methods.push(TypedMethodDecl {
                        name: method.name.clone(),
                        return_type: method.return_type.clone(),
                        params: method.params.clone(),
                        body: TypedMethodBody::Io(op.clone()),
                    });
                    continue;
                }
                MethodBody::UserDefined(b) => b,
            };
            let scope = Scope::build(&method.params, &body.locals, table)?;
            let ctx = BodyCtx { class_name: &class.name, return_type: &method.return_type, in_loop: false, in_constructor: false };
            let mut typed_stmts = Vec::with_capacity(body.stmts.len());
            for stmt in &body.stmts {
                typed_stmts.push(check_stmt(stmt, &scope, &ctx, table)?);
            }
            if method.return_type != Type::Void && !definitely_returns(&body.stmts) {
                return Err(TypeError::new(ErrorCode::EReturnMissing, method.line,
                    format!("'{}' does not return on every path", method.name)));
            }
            typed_methods.push(TypedMethodDecl {
                name: method.name.clone(),
                return_type: method.return_type.clone(),
                params: method.params.clone(),
                body: TypedMethodBody::UserDefined { locals: flatten_locals(&body.locals), stmts: typed_stmts },
            });
        }

        let typed_constructors = if class.constructors.is_empty() {
            // Only reachable when extends.is_none() — resolve_inheritance already
            // rejected an inheriting class with no explicit constructor section.
            // Synthesizing the full field-assignment form here (this revision),
            // not just a bare signature, so the interpreter/codegen don't have to
            // separately "know" what an implicit constructor does.
            let fields = info.own_fields.iter().map(|(n, t, _)| (n.clone(), t.clone())).collect();
            vec![TypedConstructor::Implicit { fields }]
        } else {
            check_delegation_cycle(&class.name, table)?; // once per class
            let mut typed_ctors = Vec::with_capacity(class.constructors.len());
            for ctor in &class.constructors {
                // LO-3 §3.3.4: a constructor with no delegation and no statements is
                // a compile error, even though the grammar's (Stmt)* permits it
                // syntactically. Confirmed gap — nothing checked this before.
                if ctor.delegation.is_none() && ctor.body.stmts.is_empty() {
                    return Err(TypeError::new(ErrorCode::EWellFormednessOther, ctor.line,
                        "constructor body must contain a delegation or at least one statement"));
                }
                let scope = Scope::build(&ctor.params, &ctor.body.locals, table)?;
                let typed_delegation = check_delegation(ctor, &class.name, &scope, table)?;
                let ctx = BodyCtx { class_name: &class.name, return_type: &Type::Void, in_loop: false, in_constructor: true };
                let mut typed_stmts = Vec::with_capacity(ctor.body.stmts.len());
                for stmt in &ctor.body.stmts {
                    typed_stmts.push(check_stmt(stmt, &scope, &ctx, table)?);
                }
                typed_ctors.push(TypedConstructor::Explicit {
                    params: ctor.params.clone(),
                    delegation: typed_delegation,
                    locals: flatten_locals(&ctor.body.locals),
                    stmts: typed_stmts,
                });
            }
            typed_ctors
        };

        typed_classes.push(TypedClassDecl {
            name: class.name.clone(),
            kind: info.kind,
            extends: class.extends.clone(),
            fields: class.fields.clone(),
            constructors: typed_constructors,
            methods: typed_methods,
        });
    }

    Ok(TypedProgram { classes: typed_classes })
}

fn flatten_locals(locals: &[VarDecl]) -> Vec<(String, Type)> {
    locals.iter()
        .flat_map(|d| d.names.iter().map(move |n| (n.clone(), d.declared_type.clone())))
        .collect()
}

/// **Corrected this revision.** The rule is "does *any* statement in the sequence
/// definitely return," not "does the *last* one" — per course lecture material, an
/// unconditional return followed by (unreachable) further statements still makes the
/// enclosing sequence return. The previous draft only checked `stmts.last()`.
fn definitely_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_returns)
}

fn stmt_returns(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return(..) => true,
        Stmt::If(_, then_b, else_b, _) => definitely_returns(then_b) && definitely_returns(else_b),
        Stmt::While(..) => false, // can't prove the loop body runs at all
        _ => false,
    }
}
```

### Statement checking

```rust
fn check_stmt(stmt: &Stmt, scope: &Scope, ctx: &BodyCtx, table: &ClassTable) -> Result<TypedStmt, TypeError> {
    Ok(match stmt {
        Stmt::Empty(_) => TypedStmt::Empty,

        Stmt::Assign(name, expr, line) => {
            let binding = resolve_name(name, scope, ctx.class_name, table)
                .ok_or_else(|| TypeError::new(ErrorCode::EUnknownVariable, *line,
                    format!("unknown variable '{}'", name)))?;
            let typed_value = check_expr(expr, scope, ctx, table)?;
            if !assignment_compatible(&typed_of(&typed_value), binding.ty(), table) {
                return Err(TypeError::new(ErrorCode::EAssignTypeMismatch, *line,
                    format!("cannot assign to '{}'", name)));
            }
            TypedStmt::Assign { target: name.clone(), binding, value: typed_value }
        }

        Stmt::Return(expr, line) => {
            if ctx.in_constructor {
                return Err(TypeError::new(ErrorCode::EReturnInConstructor, *line,
                    "constructors may not contain a return statement"));
            }
            if *ctx.return_type == Type::Void {
                return Err(TypeError::new(ErrorCode::EReturnInVoidMethod, *line,
                    "a void method may not contain a return statement"));
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
                return Err(TypeError::new(ErrorCode::EBreakOutsideLoop, *line,
                    "'break' outside an enclosing while loop"));
            }
            TypedStmt::Break
        }

        Stmt::CallStmt(call) => {
            let typed_call = check_method_call(call, scope, ctx, table)?;
            if typed_call.return_type != Type::Void {
                return Err(TypeError::new(ErrorCode::ENonvoidCallAsStatement, call.line,
                    format!("result of non-void call to '{}' is discarded", call.name)));
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
```

### Expression checking

```rust
fn check_expr(expr: &Expr, scope: &Scope, ctx: &BodyCtx, table: &ClassTable) -> Result<TypedExpr, TypeError> {
    Ok(match expr {
        Expr::Num(n, _) => TypedExpr::Num(*n),
        Expr::Bool(b, _) => TypedExpr::Bool(*b),
        Expr::Str(s, _) => TypedExpr::Str(s.clone()),
        Expr::Null(_) => TypedExpr::Null,
        Expr::This(_) => TypedExpr::This(ctx.class_name.to_string()),

        Expr::Var(name, line) => {
            let binding = resolve_name(name, scope, ctx.class_name, table)
                .ok_or_else(|| TypeError::new(ErrorCode::EUnknownVariable, *line,
                    format!("unknown variable '{}'", name)))?;
            TypedExpr::Var { name: name.clone(), binding }
        }

        Expr::New(class_name, args, line) => {
            let info = table.get(class_name).ok_or_else(|| TypeError::new(ErrorCode::EUnknownClass, *line,
                format!("unknown class '{}'", class_name)))?;
            // §4.6: only the synthesized wrapper instantiates Input/Output. Confirmed
            // gap — nothing rejected this before.
            if info.kind == ClassKind::Preamble {
                return Err(TypeError::new(ErrorCode::ETypeCheckOther, *line,
                    format!("'{}' cannot be instantiated directly", class_name)));
            }
            let typed_args = check_constructor_call(&info.own_constructors, args, scope, ctx, table, *line, class_name)?;
            TypedExpr::New { class: class_name.clone(), args: typed_args }
        }

        Expr::Call(call) => {
            let typed_call = check_method_call(call, scope, ctx, table)?;
            // Confirmed gap: this check was claimed as covered in the design doc but
            // never actually implemented — Expr::Call previously just wrapped
            // whatever return type came back, Void included.
            if typed_call.return_type == Type::Void {
                return Err(TypeError::new(ErrorCode::EVoidCallInExpression, call.line,
                    format!("void call to '{}' used in expression position", call.name)));
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

        Expr::Binary(lhs, op, rhs, line) => {
            let typed_lhs = check_expr(lhs, scope, ctx, table)?;
            let typed_rhs = check_expr(rhs, scope, ctx, table)?;
            let ty = check_binop(*op, &typed_of(&typed_lhs), &typed_of(&typed_rhs), *line)?;
            TypedExpr::Binary { lhs: Box::new(typed_lhs), op: *op, rhs: Box::new(typed_rhs), ty }
        }

        Expr::Unary(op, operand, line) => {
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
                // §4.4.4: a cast applied to null always succeeds, no runtime check.
                // Confirmed bug — the previous draft rejected this outright.
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
                _ => return Err(TypeError::new(ErrorCode::ECastSourceNotClass, *line,
                    "cast source must be a class-typed expression")),
            };
            TypedExpr::Cast { target: target.clone(), operand: Box::new(typed_operand), direction }
        }

        Expr::InstanceOf(operand, class_name, line) => {
            if !table.class_exists(class_name) {
                return Err(TypeError::new(ErrorCode::EUnknownClass, *line, format!("unknown class '{}'", class_name)));
            }
            let typed_operand = check_expr(operand, scope, ctx, table)?;
            match typed_of(&typed_operand) {
                // §4.3.6: (null instanceof T) is legal, evaluates to false. Confirmed
                // bug — the previous draft rejected a null source outright.
                ExprType::Concrete(Type::Class(_)) | ExprType::NullLiteral => {}
                _ => return Err(TypeError::new(ErrorCode::EInstanceofSourceNotClass, *line,
                    "instanceof source must be a class-typed expression")),
            }
            TypedExpr::InstanceOf { operand: Box::new(typed_operand), class: class_name.clone() }
        }
    })
}
```

### Method calls, receivers, and `super`

```rust
struct ReceiverResolution {
    typed: TypedReceiver,
    search_class: String, // irrelevant/empty when is_super is true
    is_super: bool,
}

fn resolve_receiver(receiver: &Receiver, scope: &Scope, ctx: &BodyCtx, table: &ClassTable) -> Result<ReceiverResolution, TypeError> {
    match receiver {
        Receiver::This(_) => Ok(ReceiverResolution {
            typed: TypedReceiver::This(ctx.class_name.to_string()),
            search_class: ctx.class_name.to_string(),
            is_super: false,
        }),

        Receiver::Super(line) => {
            // §4.1 introduces super.method(...) as a method-body form specifically;
            // constructor-body delegation has its own dedicated form, super(...)
            // (ConstructorDelegation::SuperCall, handled in check_delegation).
            // Nothing about the grammar stops an ordinary MethodCall with
            // Receiver::Super from appearing in a constructor's own statement list,
            // so this needs an explicit check — confirmed gap, found in review.
            if ctx.in_constructor {
                return Err(TypeError::new(ErrorCode::EInheritanceCheckOther, *line,
                    "super.method() is a method-body form and may not appear in a constructor body"));
            }
            // This is the method-body form specifically, so it gets
            // E_SUPER_METHOD_IN_ROOT_CLASS — distinct from E_SUPER_IN_ROOT_CLASS,
            // which is the constructor-delegation form's code (check_delegation).
            if table.get(ctx.class_name).and_then(|i| i.parent.as_ref()).is_none() {
                return Err(TypeError::new(ErrorCode::ESuperMethodInRootClass, *line,
                    "'super' used in a class with no parent"));
            }
            Ok(ReceiverResolution { typed: TypedReceiver::Super, search_class: String::new(), is_super: true })
        }

        Receiver::Var(name, line) => {
            let binding = resolve_name(name, scope, ctx.class_name, table)
                .ok_or_else(|| TypeError::new(ErrorCode::EUnknownVariable, *line,
                    format!("unknown variable '{}'", name)))?;
            let Type::Class(c) = binding.ty().clone() else {
                return Err(TypeError::new(ErrorCode::EReceiverNotClassType, *line, "receiver is not class-typed"));
            };
            Ok(ReceiverResolution { typed: TypedReceiver::Var { name: name.clone(), binding }, search_class: c, is_super: false })
        }

        Receiver::Computed(expr, line) => {
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
    let r = resolve_receiver(&call.receiver, scope, ctx, table)?;

    // resolve_receiver already rejected super-in-a-constructor and super-in-a-root-
    // class before returning is_super = true, so ctx.class_name's parent is
    // guaranteed to exist here.
    let (owner, sig) = if r.is_super {
        // super.m(...): the STATIC PARENT's effective methods, never the current
        // class's own override.
        let parent = table.get(ctx.class_name).and_then(|i| i.parent.as_deref()).unwrap();
        table.get(parent).unwrap().effective_methods.get(&call.name).cloned()
            .ok_or_else(|| TypeError::new(ErrorCode::ESuperMethodUnresolved, call.line,
                format!("no ancestor declares method '{}'", call.name)))?
    } else {
        // r.search_class is always a name already validated to exist in `table` —
        // every Type::Class(name) that can flow into it was checked before reaching
        // here — so table.get can't miss.
        let info = table.get(&r.search_class).unwrap();
        info.effective_methods.get(&call.name).cloned()
            .ok_or_else(|| TypeError::new(ErrorCode::EUnknownMethod, call.line,
                format!("unknown method '{}' on class '{}'", call.name, r.search_class)))?
    };

    // owner is now genuinely used (this revision) — retained on the typed call for
    // codegen/interpreter, not discarded as it was in the previous draft.
    let typed_args = check_actuals(&sig.params, &call.args, scope, ctx, table, call.line, &sig.name)?;
    Ok(TypedMethodCall { receiver: r.typed, name: call.name.clone(), owner, args: typed_args, return_type: sig.return_type })
}

fn check_actuals(
    formals: &[Type], args: &[Expr], scope: &Scope, ctx: &BodyCtx, table: &ClassTable, line: u32, what: &str,
) -> Result<Vec<TypedExpr>, TypeError> {
    if formals.len() != args.len() {
        return Err(TypeError::new(ErrorCode::EArityMismatch, line,
            format!("'{}' expects {} argument(s), got {}", what, formals.len(), args.len())));
    }
    let mut typed_args = Vec::with_capacity(args.len());
    for (formal_ty, arg) in formals.iter().zip(args) {
        let typed_arg = check_expr(arg, scope, ctx, table)?;
        if !assignment_compatible(&typed_of(&typed_arg), formal_ty, table) {
            return Err(TypeError::new(ErrorCode::EActualTypeMismatch, line,
                format!("argument type does not match formal type in call to '{}'", what)));
        }
        typed_args.push(typed_arg);
    }
    Ok(typed_args)
}

fn check_constructor_call(
    ctors: &[ConstructorSig], args: &[Expr], scope: &Scope, ctx: &BodyCtx, table: &ClassTable, line: u32, class_name: &str,
) -> Result<Vec<TypedExpr>, TypeError> {
    let ctor = ctors.iter().find(|c| c.arity == args.len())
        .ok_or_else(|| TypeError::new(ErrorCode::EArityMismatch, line,
            format!("no constructor of class '{}' takes {} argument(s)", class_name, args.len())))?;
    check_actuals(&ctor.params, args, scope, ctx, table, line, class_name)
}
```

### Constructor delegation

Two functions, as in the previous draft: one scans a whole class's `this(...)` graph
for cycles, run once per class; the other resolves a single constructor's own
delegation target and now returns the typed form.

```rust
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
        return Err(TypeError::new(ErrorCode::EDelegationCycle, line,
            format!("constructor delegation in class '{}' forms a cycle", class_name)));
    }
    Ok(())
}

/// Assumes check_delegation_cycle already ran for the enclosing class and found
/// nothing, so any this(...) target found here is guaranteed to terminate.
fn check_delegation(ctor: &ConstructorDecl, class_name: &str, scope: &Scope, table: &ClassTable) -> Result<Option<TypedDelegation>, TypeError> {
    let info = table.get(class_name).unwrap();
    let inheriting = info.parent.is_some();

    match &ctor.delegation {
        None => {
            if inheriting {
                // Gap in the published vocabulary — filed under E_INHERITANCE_CHECK_OTHER.
                // LO-4 §4.1 requires super(...)/this(...) as the first statement of
                // every constructor in an inheriting class.
                return Err(TypeError::new(ErrorCode::EInheritanceCheckOther, ctor.line,
                    "constructor of an inheriting class must start with super(...) or this(...)"));
            }
            Ok(None)
        }
        Some(ConstructorDelegation::SuperCall(args, line)) => {
            let parent = info.parent.as_ref().ok_or_else(|| TypeError::new(ErrorCode::ESuperInRootClass, *line,
                "super(...) used in a class with no parent"))?;
            let parent_info = table.get(parent).unwrap();
            let target = parent_info.own_constructors.iter().find(|c| c.arity == args.len())
                .ok_or_else(|| TypeError::new(ErrorCode::EDelegationArityMismatch, *line,
                    format!("'{}' has no constructor of arity {}", parent, args.len())))?;
            let ctx = BodyCtx { class_name, return_type: &Type::Void, in_loop: false, in_constructor: true };
            let typed_args = check_actuals(&target.params, args, scope, &ctx, table, *line, parent)?;
            Ok(Some(TypedDelegation::Super { args: typed_args }))
        }
        Some(ConstructorDelegation::ThisCall(args, line)) => {
            let target = info.own_constructors.iter().find(|c| c.arity == args.len())
                .ok_or_else(|| TypeError::new(ErrorCode::EDelegationArityMismatch, *line,
                    format!("class '{}' has no constructor of arity {}", class_name, args.len())))?;
            let ctx = BodyCtx { class_name, return_type: &Type::Void, in_loop: false, in_constructor: true };
            let typed_args = check_actuals(&target.params, args, scope, &ctx, table, *line, class_name)?;
            Ok(Some(TypedDelegation::This { args: typed_args }))
        }
    }
}
```

### Compatibility and operator typing

```rust
fn assignment_compatible(from: &ExprType, to: &Type, table: &ClassTable) -> bool {
    match from {
        ExprType::NullLiteral => matches!(to, Type::Class(_)),
        ExprType::Concrete(Type::Class(a)) => matches!(to, Type::Class(b) if table.is_subtype(a, b)),
        ExprType::Concrete(t) => t == to,
    }
}

/// **Corrected this revision** — least common ancestor, not a bidirectional
/// is_subtype check. See design doc, Algorithms §7, for why the old check was a
/// confirmed bug (Cat/Dog siblings under Animal would incorrectly fail).
fn combine_ternary_branches(a: &ExprType, b: &ExprType, table: &ClassTable, line: u32) -> Result<Type, TypeError> {
    use ExprType::*;
    let mismatch = || TypeError::new(ErrorCode::EConditionalTypeMismatch, line, "ternary branches have incompatible types");
    match (a, b) {
        // Both branches null: no concrete Type exists to put on TypedExpr::Ternary's
        // `ty` field (the typed AST has no Option-typed fields to fall back on) — a
        // deliberately accepted edge case, not a gap. See design doc.
        (NullLiteral, NullLiteral) => Err(mismatch()),
        (NullLiteral, Concrete(t @ Type::Class(_))) | (Concrete(t @ Type::Class(_)), NullLiteral) => Ok(t.clone()),
        (Concrete(t1), Concrete(t2)) if t1 == t2 => Ok(t1.clone()),
        (Concrete(Type::Class(c1)), Concrete(Type::Class(c2))) =>
            least_common_ancestor(c1, c2, table).map(Type::Class).ok_or_else(mismatch),
        _ => Err(mismatch()),
    }
}

/// `=`/`<`/`>` on class-typed (or null) operands is rejected, confirmed correct
/// against the canonical language reference (design doc, Open item 1 — now resolved).
fn check_binop(op: BinaryOp, lhs: &ExprType, rhs: &ExprType, line: u32) -> Result<Type, TypeError> {
    use BinaryOp::*;
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
            if l == r && matches!(l, Type::Int | Type::String) { Ok(Type::Bool) }
            else { Err(mismatch()) }
        }
    }
}

fn check_unop(op: UnaryOp, operand: &ExprType, line: u32) -> Result<Type, TypeError> {
    let ExprType::Concrete(t) = operand else {
        return Err(TypeError::new(ErrorCode::EUnopTypeMismatch, line, "null is not a legal operand"));
    };
    match (op, t) {
        (UnaryOp::Not, Type::Bool) => Ok(Type::Bool),
        (UnaryOp::Neg, Type::Int) => Ok(Type::Int),
        (UnaryOp::Neg, Type::String) => Ok(Type::String), // `~` also means string reversal
        _ => Err(TypeError::new(ErrorCode::EUnopTypeMismatch, line, "operand type mismatch")),
    }
}
```

### Entry point

```rust
fn check_program(program: Program) -> Result<(TypedProgram, ClassTable), TypeError> {
    let mut program = program;
    inject_preamble(&mut program);
    let mut table = gather_declarations(&program)?;
    resolve_inheritance(&mut table)?;
    check_entry_point(&table)?;
    let typed = check_bodies(&program, &table)?;
    Ok((typed, table))
}
```

`check_program` takes ownership of `Program` and consumes it entirely — nothing
downstream needs the original untyped tree back, only `TypedProgram` and
`ClassTable`. Preamble injection lives inside, per this revision's decision — there
is exactly one way to obtain a `TypedProgram`, and it always includes `Input`/`Output`.

---

## `src/main.rs` (add to the existing file)

```rust
mod sema;

fn main() {
    // CLI entry point comes later — wiring shown for reference:
    //
    // let program = parser::parse_program(&tokens)?;
    // let (typed_program, class_table) = sema::check_program(program)?;
    // interpret/codegen consume `typed_program` and, if needed, `class_table`.
}
```

---

## Acceptance tests

Given as source-level `.lo` fragments (parse them, run `check_program`, check the
result). Rows carried over from the previous draft are unaffected by this revision's
changes unless noted; new rows specifically exercise this revision's fixes.

| Input | Expected |
|---|---|
| `class Main() { int main() { return 0; } }` | `Ok` |
| `class Main() { int main() { } }` | `Err(EReturnMissing)` |
| `class Main() { void main() { return 0; } }` | `Err(EMainMethodSignature)` |
| `class Foo() {} class Foo() {}` (plus a valid `Main`) | `Err(EDuplicateClassName)` |
| `class A(int x; int x;) {}` | `Err(EDuplicateField)` |
| `class Main(){int f(int x, int x){return x;} int main(){return 0;}}` | `Err(EDuplicateFormal)` — new; a confirmed gap where a duplicate formal previously silently overwrote the first binding with no error at all |
| `class A extends B () {}` — `B` never declared | `Err(EUnknownClass)` |
| `class A extends B (){} class B extends A (){}` | `Err(EInheritanceCycle)` |
| `class Main() { int main() { return 0; } } class Foo extends Main() [Foo(){super();}] {}` | `Err(EEntryPointOther)` — new; nothing may extend `Main` |
| `class Foo extends Input() [Foo(){super();}] {} class Main() { int main() { return 0; } }` | `Err(EInheritanceCheckOther)` — new; `Input`/`Output` are non-extensible |
| `class Main() { int main() { Input i; i = new Input(); return 0; } }` | `Err(ETypeCheckOther)` — new; only the synthesized wrapper instantiates `Input`/`Output` |
| `class A(int x;)[A(int v){x=v;}]{} class B extends A(int x;)[B(int v){super(v);}]{}` | `Err(EFieldShadowing)` |
| `class A(){int f(){return 1;}} class B extends A(){[B(){super();}]{bool f(){return true;}}}` | `Err(EOverrideSignatureMismatch)` |
| `class A extends B(){}` where `B` is a valid root class, no `[ ]` section on `A` | `Err(EMissingConstructorInInheritingClass)` |
| `class Foo() [Foo(){}] {} class Main() { int main() { return 0; } }` | `Err(EWellFormednessOther)` — new; an explicit constructor with no delegation and no statements is a compile error even though `(Stmt)*` allows it syntactically |
| `class Main() { int main() { return (1 + true); } }` | `Err(EBinopTypeMismatch)` |
| `class Main() { int main() { out.print_int(1); return out.print_int(2); } }` | `Err(EVoidCallInExpression)` — **now actually caught**; the previous draft claimed this check existed but never implemented it |
| `class Main() { int main() { out.println(); return 0; } }` | `Ok` |
| `class Main() { int main() { return this.helper(); } int helper() { return 1; } }` | `Ok` — forward reference within the same class |
| `class Animal(){String describe(){return "a";}} class Dog extends Animal()[Dog(){super();}]{String describe(){return ((((Dog)this) instanceof Dog) ? super.describe() : "?");}}` | `Ok` — exercises cast, `instanceof`, and `super.m()` together, fully parenthesized per P25/P29/P30 |
| `class Animal(){String describe(){return "a";}} class Dog extends Animal()[Dog(){super();super.describe();}]{}` | `Err(EInheritanceCheckOther)` — new; `super.method()` is a method-body form and may not appear in a constructor body |
| `class Foo(){} class Main() { int main() { Foo f; f = ((Foo) null); return 0; } }` | `Ok` — new; a cast applied to `null` always succeeds per §4.4.4 (previously incorrectly rejected) |
| `class Foo(){} class Main() { int main() { return ((null instanceof Foo) ? 1 : 0); } }` | `Ok`, returns `0` — new; `null instanceof T` is legal and always `false` per §4.3.6 (previously incorrectly rejected) |
| `class Animal(){} class Cat extends Animal()[Cat(){super();}]{} class Dog extends Animal()[Dog(){super();}]{} class Main(){int main(){bool b;b=true;Animal a;a=(b ? new Cat() : new Dog());return 0;}}` | `Ok` — new, the case the whole LCA fix is for: `Cat`/`Dog` share no direct subtype relation, so this would incorrectly fail `E_CONDITIONAL_TYPE_MISMATCH` under the old bidirectional check; the correct result type is `Animal`, their least common ancestor |
| `class Main() { int main() { break; return 0; } }` | `Err(EBreakOutsideLoop)` |
| `class Animal(){} class Dog extends Animal()[Dog(){out.println();}]{}` (no `super`/`this` as first statement) | `Err(EInheritanceCheckOther)` |
| `class C(){[C(){this(1);} C(int x){this();}]{}}` | `Err(EDelegationCycle)` |
| `class Main() { int main() { void x; return 0; } }` | `Err(ELocalTypedVoid)` — new; confirmed gap, local declared types were never validated at all |
| `class Main() { int main() { Bogus x; return 0; } }` — `Bogus` never declared | `Err(EUnknownClass)` — new; same gap, this half of it is an unknown-class reference inside a local declaration |
| `class A(int x;)[A(int x){x=x;}]{int get(){return x;}}` — `new A(5).get()` at runtime returns `0`, not `5` | `Ok` — **not** a checker bug; see design doc Notes. A formal sharing a field's name makes the field unassignable by bare identifier, a real language-level sharp edge, not something this pass should or structurally could reject. |

---

## Explicitly out of scope for this step

- Wiring a CLI mode (`check`/`interpret`/`compile`) — `main.rs` stays a stub beyond
  the module wiring shown above.
- The interpreter and codegen themselves — both consume the `TypedProgram` and
  `ClassTable` that `check_program` returns, but building either is a separate
  implementation plan.
- Unifying `sema::ErrorCode` with `parser::ErrorCode` into one shared diagnostic type
  — deferred per `type_checker_design.md`'s Open items.
- Actually reconciling the full list of invented/reused codes with course staff's
  published vocabulary (see design doc Notes for the complete inventory as of this
  revision) — this plan implements against the vocabulary as it stands today, with
  the documented workarounds, and should be revisited if staff respond.
- Vtable slot assignment, and anything else codegen-specific — `ClassTable`'s
  `effective_methods` has what a future codegen pass needs to assign slots itself.
