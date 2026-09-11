# Type Checker Implementation Plan — LO (LiveOak) P1

Implements everything decided in `type_checker_design.md`, against the `compiler/`
crate as it exists on `parser/phase-1-implementation` (`ast.rs`, `lexer.rs`,
`parser.rs`, `token.rs`, `main.rs`). Adds one new file and makes small, explicitly
flagged additions to `ast.rs`.

---

## Files to create / modify

| File | Change |
|---|---|
| `compiler/src/sema.rs` | New. Everything below. |
| `compiler/src/ast.rs` | **Untouched.** An earlier draft of this plan added `Cell`-based annotation fields here for a codegen phase that doesn't exist yet — cut per `type_checker_design.md`'s "What downstream phases get": `check_program` hands back the `ClassTable` instead, and nothing needs to be cached on the AST itself. |
| `compiler/src/main.rs` | Add `mod sema;` and wire `sema::check_program` after parsing. |

---

## Decisions resolved for this implementation

These were open items in `type_checker_design.md`, or surfaced while writing this
plan. Picking something concrete now so there's nothing left for an implementer to
guess.

| Item | Decision |
|---|---|
| `=`/`<`/`>` on class-typed operands (design's Open item #1) | **Not legal**, following LO-2 §3.1's literal "no other operand combination is legal." `(out = err)` is `E_BINOP_TYPE_MISMATCH` under this implementation. If course staff confirms reference equality was intended, this is a single extra match arm to add later (see `check_binop`) — not a structural change. |
| Ternary result type for related-but-unequal class types (design's Open item #2) | Same bidirectional check as casts: if the branch types are equal, that's the result; if primitives, they must match exactly (no widening between primitives); if one class type is a subtype of the other, the result is the wider (supertype) one; otherwise `E_CONDITIONAL_TYPE_MISMATCH`. |
| `null` as an expression's type | `check_expr` returns `ExprType`, not `ast::Type` directly — `ExprType::Concrete(Type)` or `ExprType::NullLiteral`. Every compatibility check (assignment, return, actual-argument, ternary) matches on this instead of comparing `Type` values directly, since `null` has no `Type` of its own but is compatible with any class type. |
| **Gap found, not in `type_checker_design.md`:** no published code for *"first statement of an inheriting class's constructor is neither `super(...)` nor `this(...)`"* (LO-4 §4.1) | Filed under `E_INHERITANCE_CHECK_OTHER` for now, alongside the already-known `E_BREAK_OUTSIDE_LOOP` gap — both go on the same course-staff reconciliation list. |
| Implicit-constructor synthesis | Done once, in `gather_declarations` (Pass 1): if `class_decl.constructors.is_empty()` and `class_decl.extends.is_none()`, synthesize one `ConstructorSig { arity: own_fields.len(), params: own_fields' types, .. }` directly into `ClassInfo.own_constructors`. (If `extends.is_some()` and `constructors.is_empty()`, that's `E_MISSING_CONSTRUCTOR_IN_INHERITING_CLASS` instead — never synthesized.) Body-checking in Pass 4 only ever walks *explicit* `ConstructorDecl`s, since an implicit constructor has no body AST to check — its correctness is definitional (`this.f = f` for each same-named, same-typed formal). |
| Constructor delegation arity failures use `E_DELEGATION_ARITY_MISMATCH`, never the general `E_ARITY_MISMATCH` | The vocabulary's own text for `E_ARITY_MISMATCH` explicitly lists `super(...)`/`this(...)` alongside ordinary calls and `new` — the two codes genuinely overlap for the delegation case (this was missed in the design doc's first pass; now in its Open items). This implementation resolves the overlap by always preferring the more specific code for a delegation failure, matching how every other narrower/general pair in the vocabulary (e.g. the `..._OTHER` sentinels) is meant to be read. |
| `ConstructorSig` carries `this_target_arity: Option<usize>` | A **correction**, not the original plan: an earlier draft had `check_delegation`'s cycle detector call a helper fed `&ClassTable`, while claiming in prose that the helper "needs the raw `ClassDecl`" — those two statements contradict each other, and as drafted the helper had no way to actually work. Fixed by capturing each constructor's `this(...)` target arity (if any) directly in `ConstructorSig` during Pass 1, so Pass 4's cycle check reads it straight off `ClassTable` with no second data source needed. |
| `ClassTable` carries an explicit `order: Vec<String>` (source declaration order) | `resolve_inheritance` originally iterated `table.classes.keys()` — a `HashMap`, unordered. For a program with more than one independent violation, that made *which* error/code came back nondeterministic across runs, which is a real problem when the whole grading strategy is fail-fast + substring-matching a specific code. `gather_declarations` already sees classes in source order (`&program.classes`, a `Vec`); it now also records that order for `resolve_inheritance` to reuse, matching the ordering `gather_declarations`/`check_bodies` already had for free. |
| Reserved-name checks moved earlier where needed | Two related fixes to what an earlier draft got backwards or skipped entirely — see the `gather_declarations` and `Scope::build` code below and the inline comments on each. |

---

## `src/sema.rs`

### Error type

```rust
use crate::ast::*;
use std::collections::HashMap;

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
    EDuplicateConstructorArity,
    EFieldTypedVoid,
    EFormalTypedVoid,
    EReturnInVoidMethod,
    EReturnMissing,
    EReturnInConstructor,
    EBreakOutsideLoop,     // not in the published vocabulary — see design doc Open items
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
            EDuplicateConstructorArity => "E_DUPLICATE_CONSTRUCTOR_ARITY",
            EFieldTypedVoid => "E_FIELD_TYPED_VOID",
            EFormalTypedVoid => "E_FORMAL_TYPED_VOID",
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

/// The internal type of a checked expression. Distinct from `ast::Type` because
/// `null` has no declared type of its own but is compatible with any class type —
/// see "Decisions resolved for this implementation."
#[derive(Debug, Clone, PartialEq)]
enum ExprType {
    Concrete(Type),
    NullLiteral,
}
```

### Class table

Fields and methods below are `pub` (`pub(crate)` at minimum) even though nothing in
*this* implementation plan needs them to be — `check_program` hands `ClassTable` back
to the caller precisely so a future codegen implementation can call these same
lookups instead of rebuilding its own, and a private struct full of private methods
couldn't actually be reused by another module. The exact public surface is provisional
and codegen's own implementation plan can narrow or extend it once it exists; the
point for now is just that "reuse this instead of re-deriving it" is actually possible.

```rust
pub struct ClassTable {
    classes: HashMap<String, ClassInfo>,
    order: Vec<String>, // source declaration order — see "Decisions resolved"
}

pub struct ClassInfo {
    pub decl_line: u32,
    pub parent: Option<String>,
    own_fields: Vec<(String, Type, u32)>,        // (name, type, decl line)
    own_methods: Vec<MethodSig>,
    own_constructors: Vec<ConstructorSig>,

    pub ancestors: Vec<String>,                       // filled by resolve_inheritance; does NOT include self, root-terminated
    pub effective_fields: Vec<(String, Type, String)>, // (name, type, owner), parent-first
    pub effective_methods: HashMap<String, (String, MethodSig)>, // name -> (owner, sig)
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
    line: u32,
    /// `Some(target_arity)` iff this constructor's own delegation is `this(...)`
    /// targeting a constructor of `target_arity` in the same class. `None` for
    /// `super(...)`, no delegation at all, or a `this(...)` whose target doesn't
    /// exist (that failure is reported separately, by arity lookup, not by this
    /// field being absent). Exists purely so delegation-cycle detection (Pass 4)
    /// can read the whole class's `this(...)` graph straight off `ClassTable`,
    /// with no second pass over the AST needed.
    this_target_arity: Option<usize>,
}

impl ClassTable {
    pub fn get(&self, name: &str) -> Option<&ClassInfo> {
        self.classes.get(name)
    }

    pub fn class_exists(&self, name: &str) -> bool {
        self.classes.contains_key(name)
    }

    /// `a <: b` — is `a` `b` itself or a descendant of it, per the precomputed
    /// ancestor chain. Never called with `Type::String` on either side (see design
    /// doc's "Preconditions"); callers guard that before reaching here.
    pub fn is_subtype(&self, a: &str, b: &str) -> bool {
        a == b || self.classes.get(a).is_some_and(|info| info.ancestors.iter().any(|anc| anc == b))
    }

    /// A cast is legal iff one side is a subtype of the other (either direction —
    /// upcast or downcast). This checker only needs legality, not *which* direction:
    /// whether the cast is a no-op (upcast) or needs a runtime `lo_cast_check`
    /// (downcast) is a codegen concern, decided later by codegen re-running this same
    /// `is_subtype` check on its own — see design doc, "What downstream phases get."
    pub fn cast_is_legal(&self, target: &str, source: &str) -> bool {
        self.is_subtype(source, target) || self.is_subtype(target, source)
    }
}
```

`ConstructorSig`/`own_fields`/`own_methods`/`own_constructors` stay private: they're
Pass-1-internal representations (arity/param lists without names, `this_target_arity`
existing purely for the cycle check) that a consumer would want restated more usefully
anyway — `effective_fields`/`effective_methods` are the actually-useful, already-merged
view of a class, and those are the fields made `pub`.

### Generic cycle detector (design doc, Algorithms §4)

```rust
/// DFS with a "currently on this path" set. `edges(node)` returns every node `node`
/// points to. Returns the first back-edge found, as `(from, to)`, or `None` if
/// acyclic. Used for both `extends` cycles and `this(...)` delegation cycles.
fn find_cycle<'a, N, F>(nodes: impl Iterator<Item = &'a N>, edges: F) -> Option<(N, N)>
where
    N: Eq + std::hash::Hash + Clone + 'a,
    F: Fn(&N) -> Vec<N>,
{
    let mut visited: std::collections::HashSet<N> = std::collections::HashSet::new();
    let mut on_path: Vec<N> = Vec::new();

    fn visit<N, F>(
        node: &N,
        edges: &F,
        visited: &mut std::collections::HashSet<N>,
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

```rust
/// Synthesizes `Input`/`Output` as ordinary `ClassDecl`s with `MethodBody::Io`
/// bodies, and prepends them to `program.classes`. Called once, before
/// `check_program`. Line `0` marks a synthesized declaration — never emitted by the
/// parser, so it can't collide with a real diagnostic's line number in practice, but
/// diagnostics about the preamble itself (there should never be any) would read `@0`.
pub fn inject_preamble(program: &mut Program) {
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
/// after locals/formals/fields all miss — see design doc, Algorithms §5.
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

pub fn gather_declarations(program: &Program) -> Result<ClassTable, TypeError> {
    let mut classes = HashMap::new();
    let mut order = Vec::new();

    for class in &program.classes {
        // Reserved-name check MUST run before the duplicate-name check below. Both
        // Input and Output are already present in `classes` by the time any user
        // class is visited here (inject_preamble always runs first), so a user's
        // `class Input() {}` would otherwise collide with EDuplicateClassName before
        // this arm is ever reached — the wrong code for what the vocabulary
        // specifically calls out as E_RESERVED_CLASS_NAME. `class.line != 0` exempts
        // the two synthesized classes themselves (see inject_preamble) — no real
        // source line is ever 0, since the lexer starts counting at 1.
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
            for p in &method.params {
                if p.declared_type == Type::Void {
                    return Err(TypeError::new(ErrorCode::EFormalTypedVoid, p.line,
                        format!("formal '{}' cannot have type void", p.name)));
                }
                check_not_reserved_var_name(&p.name, p.line)?;
            }
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
            for p in &ctor.params {
                if p.declared_type == Type::Void {
                    return Err(TypeError::new(ErrorCode::EFormalTypedVoid, p.line,
                        format!("formal '{}' cannot have type void", p.name)));
                }
                check_not_reserved_var_name(&p.name, p.line)?;
            }
            let this_target_arity = match &ctor.delegation {
                Some(ConstructorDelegation::ThisCall(args, _)) => Some(args.len()),
                _ => None,
            };
            own_constructors.push(ConstructorSig {
                arity,
                params: ctor.params.iter().map(|p| p.declared_type.clone()).collect(),
                line: ctor.line,
                this_target_arity,
            });
        }
        // Implicit constructor: only for a root (non-extending) class with no
        // explicit constructor section at all. An inheriting class with none is
        // E_MISSING_CONSTRUCTOR_IN_INHERITING_CLASS, checked in resolve_inheritance
        // once `extends` targets are known to resolve — deliberately not re-derived
        // here from `class.extends.is_some()` alone, to keep "is this class's
        // hierarchy well-formed at all" answered in one place.
        if own_constructors.is_empty() && class.extends.is_none() {
            own_constructors.push(ConstructorSig {
                arity: own_fields.len(),
                params: own_fields.iter().map(|(_, t, _)| t.clone()).collect(),
                line: class.line,
                this_target_arity: None, // implicit constructors never delegate
            });
        }

        order.push(class.name.clone());
        classes.insert(class.name.clone(), ClassInfo {
            decl_line: class.line,
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
reserved-name check happens in `Scope::build` (Pass 4) instead — see below.

### Pass 2 — `resolve_inheritance`

```rust
pub fn resolve_inheritance(table: &mut ClassTable) -> Result<(), TypeError> {
    // 2a: every type reference (extends target, field/formal/return types) resolves.
    // Iterates `table.order` (source declaration order), NOT `table.classes.keys()`
    // — a `HashMap`'s key order is unspecified, which would make *which* error comes
    // back nondeterministic across runs for a program with more than one violation.
    // `gather_declarations` already had this order for free; `resolve_inheritance`
    // just reuses it instead of re-deriving something weaker from the map.
    let names: Vec<String> = table.order.clone();
    for name in &names {
        let info = table.classes.get(name).unwrap();
        if let Some(parent) = &info.parent {
            if !table.class_exists(parent) {
                return Err(TypeError::new(ErrorCode::EUnknownClass, info.decl_line,
                    format!("class '{}' extends unknown class '{}'", name, parent)));
            }
        }
        if info.parent.is_some() && info.own_constructors.is_empty() {
            // Can only be empty here if it was never synthesized (extends.is_some())
            // — see gather_declarations.
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

    // 2b: no extends cycle.
    if let Some((from, _to)) = find_cycle(names.iter(), |n| {
        table.classes.get(n).and_then(|i| i.parent.clone()).into_iter().collect()
    }) {
        let line = table.classes[&from].decl_line;
        return Err(TypeError::new(ErrorCode::EInheritanceCycle, line,
            format!("inheritance cycle involving class '{}'", from)));
    }

    // 2c: effective fields/methods, parent-first, memoized post-order over the forest.
    let mut done: std::collections::HashSet<String> = std::collections::HashSet::new();
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

fn compute_effective(
    name: &str,
    table: &mut ClassTable,
    done: &mut std::collections::HashSet<String>,
) -> Result<(), TypeError> {
    if done.contains(name) {
        return Ok(());
    }
    let parent = table.classes[name].parent.clone();

    let (mut ancestors, mut effective_fields, mut effective_methods) = match &parent {
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
        match effective_methods.get(&m.name) {
            Some((_, existing)) if existing.params != m.params || existing.return_type != m.return_type => {
                return Err(TypeError::new(ErrorCode::EOverrideSignatureMismatch, m.line,
                    format!("'{}' overrides an ancestor method with a different signature", m.name)));
            }
            _ => {}
        }
        effective_methods.insert(m.name.clone(), (name.to_string(), m.clone()));
    }

    let info = table.classes.get_mut(name).unwrap();
    info.ancestors = ancestors.clone();
    info.effective_fields = effective_fields;
    info.effective_methods = effective_methods;
    done.insert(name.to_string());
    Ok(())
}
```

Note the borrow-checker-driven shape: `compute_effective` reads the parent's already-
computed data by *cloning* it into locals before taking a fresh mutable borrow of the
child's slot — Rust won't allow holding an immutable borrow of `table.classes[parent]`
across the recursive call that also needs `&mut table`. This is the natural
consequence of "memoized clone-then-extend" (design doc, Algorithms §1) rather than a
workaround for it — the clone was already the intended operation, not incidental.

### Pass 3 — `check_entry_point`

```rust
pub fn check_entry_point(table: &ClassTable) -> Result<(), TypeError> {
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

### `Scope` and name resolution (design doc, Algorithms §5)

```rust
struct Scope {
    bindings: HashMap<String, Type>,
    formal_names: std::collections::HashSet<String>,
}

impl Scope {
    fn build(params: &[Param], locals: &[VarDecl]) -> Result<Scope, TypeError> {
        let mut bindings = HashMap::new();
        let mut formal_names = std::collections::HashSet::new();
        for p in params {
            bindings.insert(p.name.clone(), p.declared_type.clone());
            formal_names.insert(p.name.clone());
        }
        // BodyScope.locals is already pairwise-distinct within itself (parser-level
        // E_DUPLICATE_LOCAL) and each VarDecl may name several identifiers sharing
        // one type ("int a, b;") — flatten per name.
        for decl in locals {
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

/// Locals/formals → fields → `in`/`out`/`err`, first match wins (design doc,
/// Algorithms §5). Returns only the resolved `Type` — nothing needs to know *which*
/// tier resolved it; a future codegen pass that does care can ask `scope` and
/// `table.get(class_name)` the same two questions itself when it gets there.
fn resolve_name(name: &str, scope: &Scope, class_name: &str, table: &ClassTable) -> Option<Type> {
    if let Some(t) = scope.bindings.get(name) {
        return Some(t.clone());
    }
    let info = table.get(class_name)?;
    if let Some((_, t, _)) = info.effective_fields.iter().find(|(n, ..)| n == name) {
        return Some(t.clone());
    }
    preamble_binding(name)
}
```

### Pass 4 — body checking

```rust
struct BodyCtx<'a> {
    class_name: &'a str,
    return_type: &'a Type,
    in_loop: bool,
    in_constructor: bool,
}

pub fn check_bodies(program: &Program, table: &ClassTable) -> Result<(), TypeError> {
    for class in &program.classes {
        for method in &class.methods {
            let body = match &method.body {
                MethodBody::UserDefined(b) => b,
                MethodBody::Io(_) => continue, // preamble methods have no body to check
            };
            let scope = Scope::build(&method.params, &body.locals)?;
            let ctx = BodyCtx {
                class_name: &class.name,
                return_type: &method.return_type,
                in_loop: false,
                in_constructor: false,
            };
            for stmt in &body.stmts {
                check_stmt(stmt, &scope, &ctx, table)?;
            }
            if method.return_type != Type::Void && !definitely_returns(&body.stmts) {
                return Err(TypeError::new(ErrorCode::EReturnMissing, method.line,
                    format!("'{}' does not return on every path", method.name)));
            }
        }

        check_delegation_cycle(&class.name, table)?; // once per class, not once per constructor
        for ctor in &class.constructors {
            let scope = Scope::build(&ctor.params, &ctor.body.locals)?;
            check_delegation(ctor, &class.name, &scope, table)?;
            let ctx = BodyCtx {
                class_name: &class.name,
                return_type: &Type::Void, // unused: constructors can't `return <Expr>;`
                in_loop: false,
                in_constructor: true,
            };
            for stmt in &ctor.body.stmts {
                check_stmt(stmt, &scope, &ctx, table)?;
            }
        }
    }
    Ok(())
}

fn definitely_returns(stmts: &[Stmt]) -> bool {
    match stmts.last() {
        Some(Stmt::Return(..)) => true,
        Some(Stmt::If(_, then_b, else_b, _)) => definitely_returns(then_b) && definitely_returns(else_b),
        _ => false,
    }
}
```

### Statement checking

```rust
fn check_stmt(stmt: &Stmt, scope: &Scope, ctx: &BodyCtx, table: &ClassTable) -> Result<(), TypeError> {
    match stmt {
        Stmt::Empty(_) => Ok(()),

        Stmt::Assign(name, expr, line) => {
            let target_ty = resolve_name(name, scope, ctx.class_name, table)
                .ok_or_else(|| TypeError::new(ErrorCode::EUnknownVariable, *line,
                    format!("unknown variable '{}'", name)))?;
            let value_ty = check_expr(expr, scope, ctx, table)?;
            if !assignment_compatible(&value_ty, &target_ty, table) {
                return Err(TypeError::new(ErrorCode::EAssignTypeMismatch, *line,
                    format!("cannot assign to '{}'", name)));
            }
            Ok(())
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
            let value_ty = check_expr(expr, scope, ctx, table)?;
            if !assignment_compatible(&value_ty, ctx.return_type, table) {
                return Err(TypeError::new(ErrorCode::EReturnTypeMismatch, *line,
                    "returned expression's type does not match the declared return type"));
            }
            Ok(())
        }

        Stmt::If(cond, then_b, else_b, line) => {
            expect_bool(cond, scope, ctx, table, *line)?;
            for s in then_b { check_stmt(s, scope, ctx, table)?; }
            for s in else_b { check_stmt(s, scope, ctx, table)?; }
            Ok(())
        }

        Stmt::While(cond, body, line) => {
            expect_bool(cond, scope, ctx, table, *line)?;
            let inner_ctx = BodyCtx { in_loop: true, ..*ctx };
            for s in body { check_stmt(s, scope, &inner_ctx, table)?; }
            Ok(())
        }

        Stmt::Break(line) => {
            if !ctx.in_loop {
                return Err(TypeError::new(ErrorCode::EBreakOutsideLoop, *line,
                    "'break' outside an enclosing while loop"));
            }
            Ok(())
        }

        Stmt::CallStmt(call) => {
            let ret = check_method_call(call, scope, ctx, table)?;
            if ret != Type::Void {
                return Err(TypeError::new(ErrorCode::ENonvoidCallAsStatement, call.line,
                    format!("result of non-void call to '{}' is discarded", call.name)));
            }
            Ok(())
        }
    }
}

fn expect_bool(expr: &Expr, scope: &Scope, ctx: &BodyCtx, table: &ClassTable, line: u32) -> Result<(), TypeError> {
    match check_expr(expr, scope, ctx, table)? {
        ExprType::Concrete(Type::Bool) => Ok(()),
        _ => Err(TypeError::new(ErrorCode::ETypeMismatch, line, "condition must be bool")),
    }
}
```

`BodyCtx { in_loop: true, ..*ctx }` requires `BodyCtx: Copy` or an explicit field-copy
constructor; given it holds a `&str`/`&Type` (both `Copy`) plus two `bool`s, deriving
`Copy` is free and avoids writing that constructor by hand.

### Expression checking

```rust
fn check_expr(expr: &Expr, scope: &Scope, ctx: &BodyCtx, table: &ClassTable) -> Result<ExprType, TypeError> {
    Ok(match expr {
        Expr::Num(..) => ExprType::Concrete(Type::Int),
        Expr::Bool(..) => ExprType::Concrete(Type::Bool),
        Expr::Str(..) => ExprType::Concrete(Type::String),
        Expr::Null(_) => ExprType::NullLiteral,

        // No "this outside a method/constructor" guard here: `Expr::This` only ever
        // parses inside a method or constructor body (there is no other place a
        // `Stmt`/`Expr` exists in the grammar), and `check_bodies` only ever builds
        // `BodyCtx` with a real class name — E_THIS_OUTSIDE_INSTANCE has no reachable
        // trigger given this AST, so it's omitted from `ErrorCode` entirely rather
        // than kept as untestable dead code with a borrowed sentinel.
        Expr::This(_) => ExprType::Concrete(Type::Class(ctx.class_name.to_string())),

        Expr::Var(name, line) => {
            let ty = resolve_name(name, scope, ctx.class_name, table)
                .ok_or_else(|| TypeError::new(ErrorCode::EUnknownVariable, *line,
                    format!("unknown variable '{}'", name)))?;
            ExprType::Concrete(ty)
        }

        Expr::New(class_name, args, line) => {
            let info = table.get(class_name).ok_or_else(|| TypeError::new(ErrorCode::EUnknownClass, *line,
                format!("unknown class '{}'", class_name)))?;
            check_constructor_call(&info.own_constructors, args, scope, ctx, table, *line, class_name)?;
            ExprType::Concrete(Type::Class(class_name.clone()))
        }

        Expr::Call(call) => ExprType::Concrete(check_method_call(call, scope, ctx, table)?),

        Expr::Ternary(cond, then_e, else_e, line) => {
            expect_bool(cond, scope, ctx, table, *line)?;
            let then_ty = check_expr(then_e, scope, ctx, table)?;
            let else_ty = check_expr(else_e, scope, ctx, table)?;
            combine_ternary_branches(then_ty, else_ty, table, *line)?
        }

        Expr::Binary(lhs, op, rhs, line) => {
            let lhs_ty = check_expr(lhs, scope, ctx, table)?;
            let rhs_ty = check_expr(rhs, scope, ctx, table)?;
            ExprType::Concrete(check_binop(*op, &lhs_ty, &rhs_ty, *line)?)
        }

        Expr::Unary(op, operand, line) => {
            let operand_ty = check_expr(operand, scope, ctx, table)?;
            ExprType::Concrete(check_unop(*op, &operand_ty, *line)?)
        }

        Expr::Cast(target, operand, line) => {
            let Type::Class(target_name) = target else {
                return Err(TypeError::new(ErrorCode::ECastTargetNotClass, *line, "cast target must be a class type"));
            };
            if !table.class_exists(target_name) {
                return Err(TypeError::new(ErrorCode::EUnknownClass, *line, format!("unknown class '{}'", target_name)));
            }
            let source_ty = check_expr(operand, scope, ctx, table)?;
            let ExprType::Concrete(Type::Class(source_name)) = &source_ty else {
                return Err(TypeError::new(ErrorCode::ECastSourceNotClass, *line, "cast source must be a class-typed expression"));
                // A NullLiteral source is also rejected here: (T) null is not a form
                // the grammar produces meaningfully differently from just `null`, and
                // the reference never discusses casting a literal null — treated as
                // ECastSourceNotClass rather than inventing a silent no-op.
            };
            if !table.cast_is_legal(target_name, source_name) {
                return Err(TypeError::new(ErrorCode::ECastUnrelatedTypes, *line,
                    format!("cannot cast '{}' to unrelated class '{}'", source_name, target_name)));
            }
            ExprType::Concrete(target.clone())
        }

        Expr::InstanceOf(operand, class_name, line) => {
            if !table.class_exists(class_name) {
                return Err(TypeError::new(ErrorCode::EUnknownClass, *line, format!("unknown class '{}'", class_name)));
            }
            match check_expr(operand, scope, ctx, table)? {
                ExprType::Concrete(Type::Class(_)) => {}
                _ => return Err(TypeError::new(ErrorCode::EInstanceofSourceNotClass, *line,
                    "instanceof source must be a class-typed expression")),
            }
            ExprType::Concrete(Type::Bool)
        }
    })
}
```

### Method calls, receivers, and `super`

```rust
fn check_method_call(call: &MethodCall, scope: &Scope, ctx: &BodyCtx, table: &ClassTable) -> Result<Type, TypeError> {
    let (search_class, is_super) = resolve_receiver(&call.receiver, scope, ctx, table)?;

    // resolve_receiver already rejected `super` in a root class (E_SUPER_METHOD_IN_ROOT_CLASS)
    // before returning `is_super = true`, so `ctx.class_name`'s parent is guaranteed
    // to exist here — no need to re-check it.
    let (_owner, sig) = if is_super {
        // super.m(...): the STATIC PARENT's effective methods, never the current
        // class's own override — deliberately a different lookup path from the
        // ordinary case below (design doc, Algorithms §6).
        let parent = table.get(ctx.class_name).and_then(|i| i.parent.as_deref()).unwrap();
        table.get(parent).unwrap().effective_methods.get(&call.name).cloned()
            .ok_or_else(|| TypeError::new(ErrorCode::ESuperMethodUnresolved, call.line,
                format!("no ancestor declares method '{}'", call.name)))?
    } else {
        // `search_class` is always a name already validated to exist in `table` —
        // every `Type::Class(name)` value that can flow into it (a formal/local/field
        // type, `Expr::This`, or a `New` expression's own result) was checked against
        // the table before it could ever reach here, so `table.get` can't miss.
        let info = table.get(&search_class).unwrap();
        info.effective_methods.get(&call.name).cloned()
            .ok_or_else(|| TypeError::new(ErrorCode::EUnknownMethod, call.line,
                format!("unknown method '{}' on class '{}'", call.name, search_class)))?
    };

    check_actuals(&sig.params, &call.args, scope, ctx, table, call.line, &sig.name)?;
    Ok(sig.return_type)
}

/// Returns (the class to search for the method, whether this was `super`).
fn resolve_receiver(receiver: &Receiver, scope: &Scope, ctx: &BodyCtx, table: &ClassTable) -> Result<(String, bool), TypeError> {
    match receiver {
        Receiver::This(_) => Ok((ctx.class_name.to_string(), false)),
        Receiver::Super(line) => {
            // `super.m(...)` — the METHOD-BODY form. Distinct from `super(...)`
            // constructor delegation, which is a different AST node entirely
            // (`ConstructorDelegation::SuperCall`, handled in `check_delegation`) and
            // correctly gets the *other* code, E_SUPER_IN_ROOT_CLASS. An earlier draft
            // used E_SUPER_IN_ROOT_CLASS here too, which is wrong per the vocabulary's
            // own distinction between the two contexts, and additionally made
            // check_method_call's (correct) E_SUPER_METHOD_IN_ROOT_CLASS check
            // permanently unreachable, since this function runs first.
            if table.get(ctx.class_name).and_then(|i| i.parent.as_ref()).is_none() {
                return Err(TypeError::new(ErrorCode::ESuperMethodInRootClass, *line,
                    "'super' used in a class with no parent"));
            }
            Ok((String::new(), true)) // class name unused on the super path
        }
        Receiver::Var(name, line) => {
            let ty = resolve_name(name, scope, ctx.class_name, table)
                .ok_or_else(|| TypeError::new(ErrorCode::EUnknownVariable, *line,
                    format!("unknown variable '{}'", name)))?;
            match ty {
                Type::Class(c) => Ok((c, false)),
                _ => Err(TypeError::new(ErrorCode::EReceiverNotClassType, *line, "receiver is not class-typed")),
            }
        }
        Receiver::Computed(expr, line) => {
            if matches!(**expr, Expr::Null(_)) {
                return Err(TypeError::new(ErrorCode::ENullLiteralReceiver, *line, "receiver cannot be the literal 'null'"));
            }
            let ctx_for_expr = BodyCtx { ..*ctx };
            match check_expr(expr, scope, &ctx_for_expr, table)? {
                ExprType::Concrete(Type::Class(c)) => Ok((c, false)),
                _ => Err(TypeError::new(ErrorCode::EReceiverNotClassType, *line, "receiver is not class-typed")),
            }
        }
    }
}

fn check_actuals(
    formals: &[Type], args: &[Expr], scope: &Scope, ctx: &BodyCtx, table: &ClassTable, line: u32, what: &str,
) -> Result<(), TypeError> {
    if formals.len() != args.len() {
        return Err(TypeError::new(ErrorCode::EArityMismatch, line,
            format!("'{}' expects {} argument(s), got {}", what, formals.len(), args.len())));
    }
    for (formal_ty, arg) in formals.iter().zip(args) {
        let arg_ty = check_expr(arg, scope, ctx, table)?;
        if !assignment_compatible(&arg_ty, formal_ty, table) {
            return Err(TypeError::new(ErrorCode::EActualTypeMismatch, line,
                format!("argument type does not match formal type in call to '{}'", what)));
        }
    }
    Ok(())
}

fn check_constructor_call(
    ctors: &[ConstructorSig], args: &[Expr], scope: &Scope, ctx: &BodyCtx, table: &ClassTable, line: u32, class_name: &str,
) -> Result<(), TypeError> {
    let ctor = ctors.iter().find(|c| c.arity == args.len())
        .ok_or_else(|| TypeError::new(ErrorCode::EArityMismatch, line,
            format!("no constructor of class '{}' takes {} argument(s)", class_name, args.len())))?;
    check_actuals(&ctor.params, args, scope, ctx, table, line, class_name)
}
```

### Constructor delegation

Two functions, deliberately separated: one scans a whole class's `this(...)` graph
for cycles, run once per class; the other resolves a single constructor's own
delegation target. An earlier draft tried to do both from inside the second function,
reaching for a helper that needed data (`ConstructorDecl.delegation`, from every
sibling constructor) that `ClassTable` didn't actually store anywhere — that's fixed
now by capturing `this_target_arity` on `ConstructorSig` itself (see "Class table"),
so the cycle scan reads `ClassTable` alone and needs no second pass over the AST.

```rust
/// Runs once per class (not once per constructor — `find_cycle` reports the same
/// cycle every time it's asked, so re-running it per constructor would just repeat
/// the same answer). Cycle check across ALL this(...) edges in the class at once —
/// A -> B -> A is a cycle even though neither edge alone is a self-loop (design doc,
/// Algorithms §4), and a constructor delegating to itself (a one-node cycle) is
/// caught by the same traversal, per LO-3 §3.3.4.
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

/// Resolves and type-checks THIS constructor's own delegation target. Assumes
/// `check_delegation_cycle` already ran for the enclosing class and found nothing —
/// so any `this(...)` target found here is guaranteed to terminate, not loop.
fn check_delegation(ctor: &ConstructorDecl, class_name: &str, scope: &Scope, table: &ClassTable) -> Result<(), TypeError> {
    let info = table.get(class_name).unwrap();
    let inheriting = info.parent.is_some();

    match &ctor.delegation {
        None => {
            if inheriting {
                // Gap in the published vocabulary — see "Decisions resolved for this
                // implementation." LO-4 §4.1 requires super(...)/this(...) as the
                // first statement of every constructor in an inheriting class.
                return Err(TypeError::new(ErrorCode::EInheritanceCheckOther, ctor.line,
                    "constructor of an inheriting class must start with super(...) or this(...)"));
            }
            Ok(())
        }
        Some(ConstructorDelegation::SuperCall(args, line)) => {
            // Nothing upstream (Pass 1/2) ever looks inside `ctor.delegation` — this
            // is the first and only place a root-class `super(...)` gets caught.
            let parent = info.parent.as_ref().ok_or_else(|| TypeError::new(ErrorCode::ESuperInRootClass, *line,
                "super(...) used in a class with no parent"))?;
            let parent_info = table.get(parent).unwrap();
            let target = parent_info.own_constructors.iter().find(|c| c.arity == args.len())
                .ok_or_else(|| TypeError::new(ErrorCode::EDelegationArityMismatch, *line,
                    format!("'{}' has no constructor of arity {}", parent, args.len())))?;
            let ctx = BodyCtx { class_name, return_type: &Type::Void, in_loop: false, in_constructor: true };
            check_actuals(&target.params, args, scope, &ctx, table, *line, parent)
        }
        Some(ConstructorDelegation::ThisCall(args, line)) => {
            let target = info.own_constructors.iter().find(|c| c.arity == args.len())
                .ok_or_else(|| TypeError::new(ErrorCode::EDelegationArityMismatch, *line,
                    format!("class '{}' has no constructor of arity {}", class_name, args.len())))?;
            let ctx = BodyCtx { class_name, return_type: &Type::Void, in_loop: false, in_constructor: true };
            check_actuals(&target.params, args, scope, &ctx, table, *line, class_name)
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

fn combine_ternary_branches(a: ExprType, b: ExprType, table: &ClassTable, line: u32) -> Result<ExprType, TypeError> {
    use ExprType::*;
    Ok(match (a, b) {
        (NullLiteral, NullLiteral) => NullLiteral,
        (NullLiteral, Concrete(t @ Type::Class(_))) | (Concrete(t @ Type::Class(_)), NullLiteral) => Concrete(t),
        (Concrete(t1), Concrete(t2)) if t1 == t2 => Concrete(t1),
        (Concrete(Type::Class(c1)), Concrete(Type::Class(c2))) => {
            if table.is_subtype(&c1, &c2) { Concrete(Type::Class(c2)) }
            else if table.is_subtype(&c2, &c1) { Concrete(Type::Class(c1)) }
            else {
                return Err(TypeError::new(ErrorCode::EConditionalTypeMismatch, line,
                    "ternary branches have unrelated class types"));
            }
        }
        _ => return Err(TypeError::new(ErrorCode::EConditionalTypeMismatch, line,
            "ternary branches have incompatible types")),
    })
}

/// `=`/`<`/`>` on class-typed (or null) operands is rejected — see "Decisions
/// resolved for this implementation." This is the single arm to relax if course
/// staff confirms reference equality was intended.
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

`UnaryOp::Neg` covers both `~` uses (integer negation and string reversal per LO-2
§3.1) since the AST doesn't split them into separate operators — the grammar's `~`
token is one `Unop` regardless of operand type, and Rust's own overload-by-return-type
dispatch (matching the *result*, not the token) makes this a natural single-function
fit rather than something needing a synthetic split.

### Entry point

```rust
/// Proof, at the type level, that a `Program` has been through `check_program`
/// successfully. Not a restructured tree — a one-line wrapper around the exact same
/// `Program` the parser produced (see design doc, "What downstream phases get").
pub struct Checked(pub Program);

pub fn check_program(mut program: Program) -> Result<(Checked, ClassTable), TypeError> {
    inject_preamble(&mut program);
    let mut table = gather_declarations(&program)?;
    resolve_inheritance(&mut table)?;
    check_entry_point(&table)?;
    check_bodies(&program, &table)?;
    Ok((Checked(program), table))
}
```

`check_program` takes ownership of `program` rather than a `&mut Program`, since it
now hands the (preamble-injected) tree back out wrapped in `Checked` — there's nothing
left for the caller to do with the original binding once checking succeeds, and taking
ownership makes that explicit instead of leaving a `&mut Program` the caller might be
tempted to keep using unchecked.

---

## `src/main.rs` (add to the existing file)

```rust
mod sema;

fn main() {
    // CLI entry point comes later — wiring shown for reference:
    //
    // let program = parser::parse_program(&tokens)?;
    // let (checked, class_table) = sema::check_program(program)?;
    // interpret/codegen consume `checked.0` and, if they need it, `class_table`.
}
```

---

## Acceptance tests

Given as source-level `.lo` fragments (parse them, run `check_program`, check the
result) rather than hand-built AST, since that's what actually exercises the parser →
checker boundary.

| Input | Expected |
|---|---|
| `class Main() { int main() { return 0; } }` | `Ok(())` |
| `class Main() { int main() { } }` | `Err(EReturnMissing)` |
| `class Main() { void main() { return 0; } }` | `Err(EMainMethodSignature)` |
| `class Foo() {} class Foo() {}` (plus a valid `Main`) | `Err(EDuplicateClassName)` |
| `class A(int x; int x;) {}` | `Err(EDuplicateField)` |
| `class A extends B () {} ` — `B` never declared | `Err(EUnknownClass)` |
| `class A extends B (){} class B extends A (){}` | `Err(EInheritanceCycle)` |
| `class A(int x;)[A(int v){x=v;}]{} class B extends A(int x;)[B(int v){super(v);}]{}` | `Err(EFieldShadowing)` |
| `class A(){int f(){return 1;}} class B extends A(){[B(){super();}]{bool f(){return true;}}}` | `Err(EOverrideSignatureMismatch)` |
| `class A extends B(){}` where `B` is a valid root class, no `[ ]` section on `A` | `Err(EMissingConstructorInInheritingClass)` |
| `class Main() { int main() { return (1 + true); } }` | `Err(EBinopTypeMismatch)` |
| `class Main() { int main() { out.print_int(1); return out.print_int(2); } }` (void call in return position) | `Err(EVoidCallInExpression)` |
| `class Main() { int main() { out.println(); return 0; } }` (call statement, void method — legal) | `Ok(())` |
| `class Main() { int main() { return this.helper(); } int helper() { return 1; } }` | `Ok(())` — forward reference within the same class |
| `class Animal(){String describe(){return "a";}} class Dog extends Animal()[Dog(){super();}]{String describe(){return ((((Dog)this) instanceof Dog) ? super.describe() : "?");}}` | `Ok(())` — exercises cast, `instanceof`, and `super.m()` together. LO requires full parenthesization of every compound expression (P25/P29/P30 each carry their own mandatory wrapping parens) — an earlier draft of this row omitted the ternary's and the `instanceof`'s own wrapping parens and would have failed to parse, let alone reach the checker. |
| `class Main() { int main() { break; return 0; } }` | `Err(EBreakOutsideLoop)` |
| `class Animal(){} class Dog extends Animal()[Dog(){out.println();}]{}` (no super/this as first statement) | `Err(EInheritanceCheckOther)` — see the noted vocabulary gap |
| `class C(){[C(){this(1);} C(int x){this();}]{}}` | `Err(EDelegationCycle)` |
| `class A(int x;)[A(int x){x=x;}]{int get(){return x;}}` — `new A(5).get()` at runtime returns `0`, not `5` | `Ok(())` — **not** a checker bug; see "Notes" in the design doc. LO has no `this.field = value` assignment form, so a formal sharing a field's name makes that field unassignable by bare identifier inside its own constructor (`x = x` resolves both sides to the formal, per the local→formal→field priority) — nothing in the well-formedness rules forbids this shape, so it type-checks fine and just behaves unexpectedly. Recorded here as a real language-level sharp edge, not something this pass should (or structurally could) reject. |

Every row above was traced through the pass structure by hand while writing this plan,
the same discipline `lexer_implementation.md` held itself to — not a "should probably
work" list. (An earlier draft of this table failed that same standard on two rows —
the field-shadowing test used a `this.x = x` assignment form that doesn't exist
anywhere in the grammar, and the cast/`instanceof`/ternary combo test was missing three
mandatory wrapping parens — both are fixed above.)

---

## Explicitly out of scope for this step

- Wiring a CLI mode (`check`/`interpret`/`compile`) — `main.rs` stays a stub beyond the
  module wiring shown above.
- The interpreter and codegen themselves — both consume the `Checked(Program)` and
  `ClassTable` that `check_program` returns, but building either is a separate
  implementation plan. In particular, exactly how codegen re-derives cast direction
  and variable-binding kind from `ClassTable` (rather than reading a cache, per "What
  downstream phases get" in the design doc) is that plan's problem to work out, not
  this one's.
- Unifying `sema::ErrorCode` with `parser::ErrorCode` into one shared diagnostic type
  — deferred per `type_checker_design.md`'s Open items.
- Actually reconciling `E_DUPLICATE_LOCAL`, `E_BREAK_OUTSIDE_LOOP`, and the
  inheriting-constructor-delegation gap with course staff's published vocabulary —
  this plan implements against the vocabulary as it stands today, with the two
  documented workarounds, and should be revisited if staff respond.
