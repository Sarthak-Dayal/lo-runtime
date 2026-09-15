## Object Layout

On WASM32, every heap object begins with the runtime-required header:

- offset 0: class descriptor pointer
- offset 4: GC bits
- offset 8: reserved flags
- offset 12: first field

All entries are four bytes because pointers and LO values use `i32`. The first three words are fixed by the ABI: virtual dispatch reads the descriptor from offset 0, while the GC owns offsets 4 and 8.

Fields are laid out parent-first, followed by child fields. The type checker records this as the class's effective field order, so inherited fields keep the same offset through both parent and subclass types. Field `i` therefore has offset `12 + 4i`.

`Output` has one compiler-private field at offset 12 containing `0` for stdout or `1` for stderr, allowing the destination to survive aliasing and parameter passing.

## Class Descriptors and Vtables

Each class has a descriptor in read-only linear memory:

- 0: class-name pointer
- 4: class-name length
- 8: parent descriptor or `0`
- 12: object size
- 16: reference-field offset array
- 20: number of reference fields
- 24: number of vtable entries
- 28: vtable address

The descriptor contains everything the runtime needs without accessing the compiler's `ClassTable`. The parent pointer supports casts and `instanceof`; object size is used by `lo_alloc`; reference offsets identify managed object and `String` fields for GC scanning.

Each class also has a complete vtable containing four-byte WebAssembly function-table indices. A subclass copies its parent's table, replaces overridden slots, and appends new methods:

    Animal:                    Dog:
    slot 0 -> Animal.speak     slot 0 -> Dog.speak
    slot 1 -> Animal.age       slot 1 -> Animal.age
                               slot 2 -> Dog.fetch

This keeps method slots stable across an inheritance hierarchy. Ordinary instance methods dispatch through the vtable using `call_indirect`; constructors and `super` calls use direct calls because their targets are statically known.

The type checker determines field order and method slots, and the backend reuses that information rather than recomputing inheritance. Descriptor addresses, vtable addresses, and function-table indices remain symbolic until `wasm-ld` resolves them.

## Declaration Pre-scan and Emission

Before emitting bodies, the backend records constructor and method symbols and their WASM signatures, then constructs descriptors, reference-field arrays, and vtables from the completed `ClassTable`.

This pre-scan allows methods to reference classes or methods declared later in the source. The emission pass then visits each constructor and method once, using checked field offsets, method slots, and descriptor symbols directly.

Scratch locals and GC-root slots are discovered during emission. Instructions are buffered until their counts are known, after which local declarations and frame setup are written before the body. This avoids maintaining a separate counting traversal that could disagree with actual temporary allocation.

## Interpreter

The interpreter is a tree-walking evaluator over the checked AST. It uses the type checker's resolved bindings, class layouts, and method information instead of repeating semantic analysis.

Each call frame stores its receiver, parameters, and locals. Objects and immutable strings live in a shared heap arena addressed by integer handles, preserving identity and allowing cyclic references. Heap exhaustion is modeled using a byte budget rather than garbage collection.

I/O uses injectable buffered streams so the same evaluator supports process I/O and in-memory tests. `return`, `break`, and runtime errors propagate through recursive evaluation until handled by the appropriate method, loop, or program boundary.

| **Typed AST Node** | **Design Rationale** | **Relevant Productions** |
|---|---|---|
| `TypedProgram { classes: Vec<TypedClassDecl> }` | Direct typed representation of the program. | P1 |
| `TypedClassDecl { class_name, kind, extends, fields, constructors, methods, line }` | Mirrors P4. `ClassKind` distinguishes user and preamble classes such as `Input` and `Output`. Fields are flattened from grouped declarations. Missing constructor sections become explicit implicit constructors. | P4, P11 |
| `TypedMethodDecl { method_name, return_type, formals, body, line }` | Mirrors P7. Formals and locals are flattened to simplify scope and binding lookup. | P7, P9 |
| `TypedMethodCall { obj_name, method_name, resolution, actuals, return_type, line }` | Shared by statement and expression calls. `MethodResolution` distinguishes virtual, `super`, and built-in dispatch so later stages do not repeat method lookup. | P10, P19, P23 |

## What We Would Do Differently

We would first introduce a structured WASM instruction representation instead of emitting assembly strings directly. This would make stack effects and branch targets easier to validate before invoking `llvm-mc`.

We would also replace repeated ABI numbers such as `12` and `28` with named constants, making the implementation easier to audit and adapt to Project 2's different pointer size.

Finally, we would test code generation earlier with small inheritance, call, and nested-loop programs. While the type checker was incomplete, several larger tests failed before reaching the backend, hiding an assembly-generation bug until later.
