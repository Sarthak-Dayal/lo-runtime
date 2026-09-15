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

`ExprType::NullLiteral` keeps null distinct during checking. Reference/null and
null/null equality are legal. Ternaries combine class types through their least
common ancestor; two null branches retain `ty: None`. Casts record `Upcast`,
`Downcast`, or `Null` for downstream handling.

## Backend boundary

The checker supplies bindings, signatures, field order, dispatch categories, and
method slots. Object headers, byte offsets, emitted vtables, allocation, GC, and
runtime abort checks belong to execution/codegen. Static checking does not remove
the need for null-receiver or downcast checks.

See [implementation](type_checker_implementation.md) for the module and pass map.
