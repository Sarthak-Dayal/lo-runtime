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
