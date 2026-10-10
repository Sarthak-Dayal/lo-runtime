# Type Checker Design — LO (LiveOak) P1

Scope: parser `Program` in, `Result<(TypedProgram, ClassTable), TypeError>` out.
The checker validates declarations and bodies, resolves names and calls, and
supplies the semantic information used by the interpreter and WASM emitter.

## Decisions

| Decision | Why |
|---|---|
| Four gated passes: declarations → inheritance → entry point → bodies | Forward references need complete signatures; body checks need valid inheritance and effective members. Each pass stops on its first error. |
| Separate typed AST | Records resolved facts without changing the parser's tree. Both execution paths reuse these decisions. |
| Preamble injection inside `check_program` | Every caller receives the same synthetic `Input`/`Output` declarations. User-name restrictions run before injection. |
| Effective members and vtable slots computed parent first | Inherited fields keep their order; overrides reuse slots; new methods append slots. The backend can use these decisions directly. |
| String-keyed class table with explicit declaration order | Simple lookup for course-sized programs, with deterministic diagnostic order. |
| Private implementation modules, public re-exports in `type_checker.rs` | Splitting the checker keeps existing consumer imports and the `check_program` interface intact. |

## Naming and parser preconditions

| Name | Meaning |
|---|---|
| `ClassTable` | Whole-program class registry, separate from body scopes. |
| `ClassInfo` | Own declarations and the derived inheritance information for one class. |
| `Scope` | Flat name-to-type map for one body's formals and hoisted locals, plus a formal-name set. |
| `BodyCtx` | Current class, declared return type, loop status, and constructor status. |
| `ExprType` | Internal comparison type: `Concrete(Type)` or `NullLiteral`. |
| `TypedObjName` | Checked receiver: `this`, `super`, a resolved variable, or a computed expression. |

The parser has already checked syntax, constructor names, duplicate locals, and
constructor-delegation position/exclusivity. It hoists nested local declarations
into `BodyScope.locals`; nested statements contain no separate declaration scope.
The checker still validates formal uniqueness, local/formal collisions, and class
types used in declarations. `String` is a keyword and a distinct `Type` variant,
so it does not enter the ordinary class hierarchy.

## Shared representations

| Type | Contents |
|---|---|
| `TypedProgram` / `TypedClassDecl` | Classes, fields, constructors, and methods; each class records its parent and `ClassKind`. Fields and formals use flattened `(String, Type)` pairs. |
| `TypedConstructor` | Explicit delegation, locals, and statements, or the fields initialized by an implicit constructor. |
| `TypedStmt` / `TypedExpr` | Checked structure, source lines, resolved bindings, and expression types where needed. |
| `BindingInfo` | Local, formal, field with declaring owner, or prebound I/O name. |
| `TypedMethodCall` | Receiver, actuals, return type, and `MethodResolution`: virtual, `super`, or built-in I/O. |
| `ClassTable` / `ClassInfo` | Own signatures, ancestor chains, effective fields/methods, and stable vtable slots. |
| `TypeError` | Error code, source line, and message; `ErrorCode::as_str()` supplies the diagnostic vocabulary. |

## Typed AST and class-table definitions

The typed tree preserves the program's structure while attaching resolved facts.
`line` stays on source-backed nodes; implicit constructors have no source line,
and call expressions/statements reuse the line on `TypedMethodCall`.

```rust
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

A null expression has no independent concrete type. A ternary with two null
branches therefore stores `ty: None`; this is valid, not an incomplete annotation.
`CastDirection::Null` distinguishes a literal-null source from an ordinary upcast.

The table separates own declarations from effective members. `ancestors` contains
strict ancestors, nearest parent first; `is_subtype(a, a)` handles equality
separately. Constructor signatures are not inherited. Internal fields remain
visible only within the checker.

```rust
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
```

## Name resolution and calls

Each body has one flat scope for formals and hoisted locals. Lookup checks that
scope, then effective fields, then `in`/`out`/`err`. The parser rejects duplicate
locals; the checker rejects locals that shadow formals and validates declared types.

For virtual calls, the receiver's static class determines legal method lookup;
its runtime class determines the overriding implementation. `super` selects the
parent's effective method directly. Every call retains its receiver, including
I/O calls, so execution can check for null receivers.

## Type rules

Assignments, returns, and actual arguments share compatibility rules: identical
types, class subtyping, or `null` assigned to a class type. `String` remains
separate from class inheritance.

`ExprType::NullLiteral` keeps null distinct during checking. Reference/reference, reference/null, and
null/null equality are legal. Ternaries combine class types through their least
common ancestor; two null branches retain `ty: None`. Casts record `Upcast`,
`Downcast`, or `Null` for downstream handling.

## Rule → pass → diagnostic

These are checker diagnostics. A malformed source program may fail in the lexer
or parser before reaching the corresponding semantic check.

| Rule | Stage | Diagnostic |
|---|---|---|
| User class named `Input` or `Output` | Before injection | `E_RESERVED_CLASS_NAME` |
| User method starts with `lo_` | Before injection | `E_RESERVED_VARIABLE_NAME` |
| Unique class / field / method names | 1 | `E_DUPLICATE_CLASS_NAME`, `E_DUPLICATE_FIELD`, `E_DUPLICATE_METHOD` |
| Unique constructor arities | 1 | `E_DUPLICATE_CONSTRUCTOR_ARITY` |
| Unique formal names | 1 | `E_WELL_FORMEDNESS_OTHER` |
| No void field / formal | 1 | `E_FIELD_TYPED_VOID`, `E_FORMAL_TYPED_VOID` |
| No redeclaration of `in`/`out`/`err` | 1; 4 for locals | `E_RESERVED_VARIABLE_NAME` |
| Declared class types exist | 2; 4 for locals/expressions | `E_UNKNOWN_CLASS` |
| No inheritance cycle | 2 | `E_INHERITANCE_CYCLE` |
| Nothing extends `Main` / a preamble class | 2 | `E_ENTRY_POINT_OTHER`, `E_INHERITANCE_CHECK_OTHER` |
| Inheriting classes declare constructors | 2 | `E_MISSING_CONSTRUCTOR_IN_INHERITING_CLASS` |
| No inherited field shadowing | 2 | `E_FIELD_SHADOWING` |
| Overrides preserve parameter and return types | 2 | `E_OVERRIDE_SIGNATURE_MISMATCH` |
| Required root `Main`, `int main()`, and zero-argument constructor | 3 | `E_NO_MAIN_CLASS`, `E_MAIN_CLASS_EXTENDS`, `E_NO_MAIN_METHOD`, `E_MAIN_METHOD_SIGNATURE`, `E_MAIN_NO_ZERO_ARG_CONSTRUCTOR` |
| No void local; explicit constructor contains delegation, locals, or statements | 4 | `E_WELL_FORMEDNESS_OTHER` |
| Local does not shadow a formal | 4 | `E_LOCAL_SHADOWS_FORMAL` |
| Variable / method lookup succeeds | 4 | `E_UNKNOWN_VARIABLE`, `E_UNKNOWN_METHOD` |
| Assignment / return / actual types are compatible | 4 | `E_ASSIGN_TYPE_MISMATCH`, `E_RETURN_TYPE_MISMATCH`, `E_ACTUAL_TYPE_MISMATCH` |
| Operator and ternary operands are compatible | 4 | `E_BINOP_TYPE_MISMATCH`, `E_UNOP_TYPE_MISMATCH`, `E_CONDITIONAL_TYPE_MISMATCH` |
| Conditions have type bool | 4 | `E_TYPE_MISMATCH` |
| Ordinary call / construction arity matches | 4 | `E_ARITY_MISMATCH` |
| Receiver has class type and is not literal null | 4 | `E_RECEIVER_NOT_CLASS_TYPE`, `E_NULL_LITERAL_RECEIVER` |
| Calls occur in the correct value/statement context | 4 | `E_NONVOID_CALL_AS_STATEMENT`, `E_VOID_CALL_IN_EXPRESSION` |
| Required return paths; no return in void methods or constructors | 4 | `E_RETURN_MISSING`, `E_RETURN_IN_VOID_METHOD`, `E_RETURN_IN_CONSTRUCTOR` |
| Break is inside a loop | 4 | `E_BREAK_OUTSIDE_LOOP` |
| Constructor `super(...)` has a parent | 4 | `E_SUPER_IN_ROOT_CLASS` |
| Method `super.m(...)` has a parent and resolves | 4 | `E_SUPER_METHOD_IN_ROOT_CLASS`, `E_SUPER_METHOD_UNRESOLVED` |
| No `super.m(...)` in constructors; inheriting constructors delegate | 4 | `E_INHERITANCE_CHECK_OTHER` |
| Constructor delegation has a matching arity and no cycle | 4 | `E_DELEGATION_ARITY_MISMATCH`, `E_DELEGATION_CYCLE` |
| User code does not instantiate preamble classes | 4 | `E_TYPE_CHECK_OTHER` |
| Cast target/source are classes and related, except null source | 4 | `E_CAST_TARGET_NOT_CLASS`, `E_CAST_SOURCE_NOT_CLASS`, `E_CAST_UNRELATED_TYPES` |
| `instanceof` source is class-typed or null | 4 | `E_INSTANCEOF_SOURCE_NOT_CLASS` |

`Main` is legal as a user class name and is checked by the entry-point rules.
`E_THIS_OUTSIDE_INSTANCE` remains in the vocabulary, but the parser's AST has no
place for `this` outside a method or constructor body.

## Algorithms

### Effective members and stable slots

`resolve_inheritance` validates parent references and declaration types, detects
cycles, then calls `compute_effective` in declaration order. A `done` set prevents
recomputing a class. Each recursive call finishes the parent first, copies its
effective members and slots, then applies the child's declarations.

Because the parent's effective set already includes all ancestors, checking it
catches shadowing and overrides beyond the immediate parent. Overrides compare
parameter vectors and return types exactly. The inherited slot stays fixed;
a method absent from the inherited slot map gets the next slot.

For example, if `Animal.speak` occupies slot 0, `Dog.speak` replaces its effective
method entry but still occupies slot 0. A new `Dog.fetch` appends a slot. Codegen
uses the static slot and the object's runtime vtable to dispatch virtually.

### Cycle detection

`find_cycle` performs DFS with a visited set and an active-path vector. Reaching
a node already on the active path identifies a cycle. The same helper checks
class-parent edges and constructor-arity edges for `this(...)` delegation.
The latter uses `ConstructorSig.this_target_arity`, collected in Pass 1.

### Scope and binding lookup

`Scope::build` inserts formals, then validates and inserts hoisted locals.
A separate formal-name set lets `resolve_name` distinguish the two binding kinds.
Locals cannot duplicate formals, so these entries cannot overwrite one another.
Effective fields and prebound I/O names are later lookup tiers.

A formal may share a field's name. Bare-name lookup then selects the formal;
the checker records that fact rather than treating the reference as a field access.

### Receiver and argument checking

`resolve_receiver` determines a receiver's static class. Computed receivers are
checked recursively; primitive receivers and literal-null receivers are rejected.
`check_method_call` looks up the effective method before checking argument arity
and compatibility. It records virtual, direct `super`, or I/O resolution.

`super.m(...)` starts from the static parent's effective methods, bypassing the
current class's override. This form requires a method body and a parent class.
Constructor `super(...)` is handled separately as delegation.

### Constructor selection and delegation

Ordinary construction selects a signature by argument count, then validates
argument types. Root classes without explicit constructors receive a constructor
whose parameters match their fields in order. Inheriting classes must declare
constructors and begin each with `this(...)` or `super(...)`.

`this(...)` selects within the current class; `super(...)` selects among the
parent's own constructors. Constructors are not inherited. Delegation cycles
are checked before individual explicit constructors are transformed.

### Null, compatibility, and ternaries

`typed_of` extracts a checked expression's comparison type. Compatibility accepts
identical concrete types, a class subtype flowing to its ancestor type, or null
flowing to a class type. It does not treat null as a String value.

For ternaries, identical types stay unchanged, null plus a class becomes that
class, and two null branches remain null. Otherwise, two class branches use their
least common ancestor. The algorithm collects one class's ancestor chain, then
walks the other from most specific to most general and takes the first match.
`Cat` and `Dog` under `Animal` produce `Animal`; disjoint hierarchies are rejected.

### Operators and casts

| Operation | Accepted operands | Result |
|---|---|---|
| `+`, `-`, `*`, `/`, `%` | int, int | int |
| `+` | String, String | String |
| `*` | String, int | String |
| `&`, `\|` | bool, bool | bool |
| `<`, `>`, `=` | int/int or String/String | bool |
| `=` | class/class, class/null, null/class, null/null | bool |
| `!` | bool | bool |
| `~` | int or String | Same type; integer negation or string reversal |

Class equality compares object identity, not field contents. Class references
remain class-typed even when their runtime value is null. Bool equality and
ordering comparisons on class references remain rejected.

The original comparison arm grouped `<`, `>`, and `=` under the primitive rules.
The first null fix added reference/null cases but missed reference/reference
comparisons. Handling reference equality in the `Eq`-only branch fixes that gap
without permitting reference ordering or changing primitive operator rules.

A class cast requires a subtype relation in one direction: widening records
`Upcast`, narrowing records `Downcast`, and a null source records `Null`.
A sibling-to-sibling cast is rejected even when the classes share an ancestor.
`instanceof` accepts unrelated target classes and null sources; runtime decides
the result, with null producing false.

### Return completeness and call position

`definitely_returns` accepts a statement sequence when any statement definitely
returns. A return qualifies directly; an `if` qualifies only if both branches do.
A loop does not establish a return because its body might never execute.
Statements are still checked even when an earlier statement returns.

`check_method_call` is shared by statement and expression contexts. Its callers
apply the position rule: `check_stmt` requires a void result, while `check_expr`
requires a non-void result. `BodyCtx` separately rejects returns in constructors,
returns in void methods, and breaks outside loops.

## Alternatives and tradeoffs

| Alternative | Reason for the current choice |
|---|---|
| Return the raw AST with a checked marker | Consumers would have to repeat binding, call, and cast resolution. |
| Annotate parser nodes in place | A separate typed tree keeps parser ownership and semantic output distinct, at the cost of two related node definitions. |
| Assign slots independently in each backend | Shared slots keep override decisions consistent; the backend still owns concrete ABI layout. |
| Re-walk ancestors at every use site | Effective members are computed once and reused. |
| Check only direct subtyping for ternary branches | That would incorrectly reject sibling branches sharing a common ancestor. |
| Introduce interned class IDs or a general symbol-table framework | String-keyed tables and flat scopes are sufficient for the current program sizes. |
| Accumulate errors across passes | Later passes depend on valid earlier results; fail-fast checking avoids cascading diagnostics. |

## Backend boundary

The checker supplies bindings, signatures, field order, dispatch categories, and
method slots. Object headers, byte offsets, emitted vtables, allocation, GC, and
runtime abort checks belong to execution/codegen. Static checking does not remove
the need for null-receiver or downcast checks.

See [implementation](type_checker_implementation.md) for the module and pass map.
