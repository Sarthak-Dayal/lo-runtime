# LO Interpreter: Structure and Detailed Design

## Overview

The interpreter executes a checked abstract syntax tree (AST): the structured representation of a program after parsing and type checking. It recursively visits expressions and statements, stores objects and strings in its own heap, and returns either `Main.main()`'s integer result or a structured runtime abort. This is a tree-walking interpreter and there is no interpreter bytecode or instruction loop.

The interpreter executes the checked abstract syntax tree through a recursive tree walk, using the type checker’s resolved bindings, class layouts, and method information. The interpreter's evaluator coordinates expressions, statements, method dispatch, and constructor execution, while per-call frames hold the receiver, parameters, and local variables. Objects and immutable strings live in a shared area addressed by integer handles, preserving object identity and supporting cyclic references; it uses a byte budget to model heap exhaustion without implementing garbage collection. Separate modules define runtime values and defaults, string operations, and runtime aborts. I/O uses injectable buffered input and output streams and supports process I/O and in-memory tests; object identity distinguishes out from err. The evaluator passes returns, breaks, and runtime errors back through nested execution until they reach the method, loop, or program boundary that handles them. The CLI converts the final outcome into an exit status and any required diagnostic.


The interpreter consists of a tree-walking evaluator, per-call frames, and a shared heap, supported by modules for runtime values, string operations, I/O, and runtime aborts. The evaluator recursively executes the checked abstract syntax tree using the type checker’s resolved bindings, class layouts, and method information. It coordinates expressions, statements, method dispatch, and constructor execution, while frames hold each call’s receiver, parameters, and local variables. Objects and immutable strings live in a shared heap arena addressed by integer handles, preserving object identity and supporting cyclic references; a byte budget models heap exhaustion without implementing garbage collection. I/O uses injectable buffered input and output streams to support both process I/O and in-memory tests, with object identity distinguishing out from err. The evaluator passes returns, breaks, and runtime errors back through nested execution until they reach the method, loop, or program boundary that handles them. The CLI converts the final outcome into an exit status and any required diagnostic.


## Source map


| Source | Responsibility | Principal definitions |
|---|---|---|
| [interpreter.rs](../compiler/src/interpreter.rs) | Module declarations, callable entry, outcome, whole-program tests | `interpret`, `Outcome`; re-exports `Io`, `AbortKind`, `DEFAULT_HEAP_LIMIT` |
| [eval.rs](../compiler/src/interpreter/eval.rs) | Runtime orchestration and all AST execution | `Interp`, `Signal`, `Exec`, expression/statement evaluation, calls, constructors, bindings |
| [value.rs](../compiler/src/interpreter/value.rs) | Runtime value representation and initialization defaults | `Value`, `type_default` |
| [heap.rs](../compiler/src/interpreter/heap.rs) | Object/string storage, handles, allocation accounting | `Heap`, `Cell`, `ObjId`, `StrId` |
| [env.rs](../compiler/src/interpreter/env.rs) | Receiver, parameters, and locals for one invocation | `Frame` |
| [strings.rs](../compiler/src/interpreter/strings.rs) | Immutable string operations | `concat`, `repeat`, `reverse`, `compare` |
| [io.rs](../compiler/src/interpreter/io.rs) | Buffered input, formatted output, test streams | `Io`, `SharedBuf` |
| [abort.rs](../compiler/src/interpreter/abort.rs) | Runtime failure categories and external diagnostics | `AbortKind::code`, `AbortKind::message` |
| [main.rs](../compiler/src/main.rs) | Source loading, front-end errors, process I/O and exit status | `main`, `run_interpreter` |
| [type_checker.rs](../compiler/src/type_checker.rs) | Checked input and precomputed semantic information | `TypedProgram`, `Typed*`, `ClassTable`, `BindingInfo`, `MethodResolution`, `CastDirection` |
| [add_io_classes.rs](../compiler/src/add_io_classes.rs) | Front-end definitions of the built-in methods | `add_io_classes`, `CLASS_NAMES` |


## 3. Contract with the front end

### 3.1 Inputs

The actual callable interface is:

```rust
pub fn interpret(
    program: &TypedProgram,
    table: &ClassTable,
    heap_limit: usize,
    io: Io,
) -> Outcome
```

The function borrows the program and class table, takes ownership of the I/O bundle, and creates a fresh interpreter. Although the function is `pub`, the current Cargo package is a binary crate rooted at `main.rs`; this is an internal callable interface, not a separately packaged Rust library.

`type_checker::check_program` injects `Input` and `Output` exactly once and returns the checked tree and class table together. The interpreter expects those outputs to correspond to the same successfully checked program. It does not re-run parsing, name resolution, overload checking, or inheritance validation.

### 3.2 Information already resolved

| Checked information | How execution uses it |
|---|---|
| `BindingInfo::{Local, Formal, Field, Prebound}` | Chooses a frame slot, object field, or built-in singleton without repeating lexical resolution |
| `MethodResolution::Virtual { static_class }` | Indicates virtual dispatch; execution finds the override using the receiver's runtime class |
| `MethodResolution::Super { declaring_class }` | Identifies the exact ancestor method body to invoke |
| `MethodResolution::Io { op }` | Selects a built-in operation directly |
| `CastDirection::{Upcast, Downcast, Null}` | Determines whether a runtime subtype check is necessary |
| `TypedMethodBody::UserDefined { locals, stmts }` | Supplies a flat local list and executable statements |
| `TypedConstructor` and `TypedDelegation` | Supplies explicit bodies, synthesized implicit initialization, and delegation arguments |
| `ClassInfo::effective_fields` | Supplies all inherited and own fields in parent-first order |
| `ClassInfo::effective_methods` | Supplies the winning declaration owner for each method name |
| `ClassInfo::parent`, `ancestors` | Supports constructor delegation and subtype checks |

The checker guarantees valid argument counts and types, valid declaration references, legal constructor delegation, and the required `Main` entry point. Runtime checks remain necessary for null receivers, downcasts, input contents, negative string repetition, and heap exhaustion.

Impossible internal states use `expect`, `unreachable!`, indexing, or debug assertions. For example, a Boolean condition represented by an integer is an interpreter/front-end invariant violation. These checks are not LO runtime diagnostics. Runtime values select overloaded operators, but correctness still depends on the checker's binding, dispatch, and cast annotations.

### 3.3 Class metadata versus executable bodies

The class table describes methods through signatures and declaring owners; it does not hold their executable statement lists. Consequently, `Interp` builds its own body index while reusing the checker's inheritance information.

An effective method entry for `Dog.kind` might name `Dog` as owner if it overrides `Animal.kind`, or `Animal` if it inherits the implementation. After finding that owner, the interpreter looks up the corresponding `TypedMethodDecl`. The class table also contains vtable-related information for code generation, but the interpreter does not use numeric vtable slots.

## 4. Interpreter state and startup

Source: [eval.rs](../compiler/src/interpreter/eval.rs), `Interp`, `with_io`, `run`.

### 4.1 Owned and borrowed state

| `Interp` field | Meaning and lifetime |
|---|---|
| `table: &ClassTable` | Immutable shared semantic metadata for the run |
| `classes` | Class name → borrowed `TypedClassDecl`, used primarily for constructors |
| `method_bodies` | Declaring class → method name → borrowed `TypedMethodDecl` |
| `heap` | All runtime object and string cells |
| `literals` | Literal text → static string handle, filled lazily |
| `io` | Owned input/output/error streams |
| `in_id`, `out_id`, `err_id` | Identities of the three built-in objects |

`Interp<'p>` borrows declarations for the program lifetime. Helpers such as `body` return references with that lifetime, allowing execution to hold a declaration reference while mutating the interpreter's heap and I/O. The interpreter does not clone method bodies for each call.

`with_io` scans classes and methods once. Only `UserDefined` methods enter `method_bodies`; I/O methods are executed through a separate branch. `Interp::new` is a convenience constructor using `Io::sink`, which supplies immediate EOF and discards output.

### 4.2 Startup sequence

1. Build the class and method-body maps.
2. Create the heap and its empty string.
3. Allocate one `Input` and two distinct `Output` singleton objects without charging the program heap budget.
4. `run` calls `construct("Main", vec![])`, allocating and initializing the entry object.
5. Look up the body declared specifically as `Main.main` and invoke it with that object and no arguments.
6. Convert an integer return to `Outcome::Exit(i32)`. Convert an abort during construction or method execution to `Outcome::Abort`.

The checker requires `Main` to declare the entry method and have a zero-argument constructor, so entry lookup is direct. `Main` itself is a normal, charged object allocation. A budget smaller than its allocation cost can abort before `main` runs.

Every call to `interpret` starts with fresh state. Once that call returns, its heap and owned streams are dropped; returned outcomes do not contain heap handles.

## 5. Runtime values and defaults

Source: [value.rs](../compiler/src/interpreter/value.rs).

```rust
pub enum Value {
    Int(i32),
    Bool(bool),
    Str(StrId),
    Obj(Option<ObjId>),
}
```

`Value` is small and `Copy`. Assigning an integer or Boolean copies its scalar value. Assigning an object or string copies its handle. Object aliases therefore see the same mutable fields; assignment does not duplicate the object. Strings are immutable and safely share their stored contents.

Only `Obj(None)` represents LO null. `Str` always has a valid string handle because the language treats `String` separately from nullable class types.

| Declared type | `type_default` result |
|---|---|
| `int` | `Value::Int(0)` |
| `bool` | `Value::Bool(false)` |
| `String` | `Value::Str(heap.empty())` |
| User or preamble class | `Value::Obj(None)` |
| `void` | No default; requesting one is an invariant failure |

Fields receive defaults when their object is allocated. Locals receive defaults when their invocation frame is created. Parameters receive evaluated arguments instead.

There is no `Value::Void`. A completed void method or output operation returns `Obj(None)` as an internal sentinel. Checked programs cannot consume a void call as an expression value; statement execution discards the sentinel.

## 6. Heap, handles, and memory accounting

Source: [heap.rs](../compiler/src/interpreter/heap.rs).

### 6.1 Arena representation

`Heap` stores one `Vec<Cell>` containing both kinds of allocation:

```rust
enum Cell {
    Obj { class: Rc<str>, fields: Vec<Value> },
    Str(Rc<str>),
}
```

`ObjId(u32)` and `StrId(u32)` wrap vector indices. Separate handle types distinguish objects from strings at Rust call sites, while both use the same index space. Vector growth may move cells in host memory but does not change their indices. No cells are removed or reused during execution.

An object records its most-derived runtime class and a vector of field values. That class identity remains unchanged while ancestor constructors and methods execute. `class_of`, `field`, and `set_field` provide the corresponding access operations.

`str_value` returns a cheap `Rc<str>` clone. A string operation can hold the source text while allocating its result through a mutable heap reference. Object relationships use integer handles, so cyclic LO graphs do not create cycles of Rust reference-counted object pointers.

### 6.2 Allocation rules

The default budget is `64 * 1024 * 1024` bytes, or 64 MiB. Accounting approximates wasm32 data sizes rather than actual Rust memory consumption:

| Allocation path | Charged bytes | Reuse |
|---|---|---|
| `alloc_obj(class, fields)` | `12 + 4 * fields.len()` | Fresh object |
| `alloc_str(nonempty)` | `12 + 4 + UTF-8 byte length` | Fresh string even if equal text already exists |
| `alloc_str("")` | Zero | Canonical empty string |
| `intern_static(nonempty)` | Zero | Fresh cell; caller handles deduplication |
| `alloc_singleton(class)` | Zero | Fresh infrastructure object with no fields |

The constants represent a 12-byte object header, one 4-byte slot per field, and a 4-byte string length. The budget excludes host `Vec`, `HashMap`, `Rc`, frame, and AST overhead.

For ordinary allocations, `charge` computes the prospective total using saturating addition and checks `next > limit`. Exactly filling the budget is allowed. A rejected allocation neither increments the accounting total nor pushes a cell.

`intern_literal` in `eval.rs` caches literal contents across expressions. The first evaluation adds an uncharged static cell; later equal literals reuse its handle. `intern_static` itself does not deduplicate nonempty strings. Dynamically computed equal strings remain separate allocations, but comparisons use contents.

### 6.3 Collection and host allocation limits

There is no garbage collector. `charged` is cumulative allocation, not live reachable memory, and unreachable objects and strings remain until the run ends. This makes memory behavior different from a collecting compiled runtime: allocation-heavy programs can exhaust this interpreter's budget after dropping references.

The budget is also not a complete host-memory safety mechanism. String helpers build temporary Rust strings before `alloc_str` checks the budget, and input reads build host buffers before their result is allocated. A sufficiently large request can fail in host allocation before becoming `AbortKind::OutOfMemory`. These are current implementation limits, not additional implemented LO abort categories.

## 7. Frames, variable access, and field layout

Sources: [env.rs](../compiler/src/interpreter/env.rs), `Frame`; [eval.rs](../compiler/src/interpreter/eval.rs), `read_var`, `write_var`, `field_slot`.

### 7.1 Frame construction

A frame contains `this: Option<ObjId>`, a `Vec<Value>` of slots, and a `HashMap<String, usize>` mapping names to slots. `Frame::new`:

1. Uses the evaluated argument vector as the initial slot vector.
2. Maps each parameter name to its positional argument slot.
3. Appends each local's type-default value and records its slot.

The frame's layout is therefore `parameters ++ locals`. The map is built for each invocation; it is not cached once per method. `get` and `set` use that map for reads and writes.

The parser hoists nested declarations into the enclosing body, and the checker supplies the flat local list. Entering an `if` or loop body does not create a new scope or reset locals. All locals exist from frame setup, even if the block containing their declaration never executes.

### 7.2 Binding-directed access

`read_var` and `write_var` inspect the attached `BindingInfo`:

- **Local or formal:** access `Frame` by name.
- **Field:** get `frame.this`, find the field's slot in that object's runtime layout, and read/write the heap cell.
- **Prebound:** return the stored `in`, `out`, or `err` handle. Assigning these bindings is unreachable because checking rejects it.

Field access currently searches `effective_fields` by name on every access. It does not use a precomputed field offset from `BindingInfo::Field.owner`. Inherited field names are unique, and parent-first layout ensures an ancestor field remains at the same index in descendants.

Field hiding across the hierarchy is forbidden, but a formal can shadow a field. In `C(int x) { x = x; }`, both names resolve to the formal, leaving the field at its default. The interpreter follows the recorded binding rather than inferring constructor initialization from matching names.

### 7.3 Calls and the host stack

Each user method and explicit constructor creates a fresh frame as a Rust local variable. Recursive evaluator calls provide the call stack; `Interp` has no explicit stack of frames. Returning drops the callee frame while retaining heap objects referenced by handles elsewhere. Deep recursion is consequently limited by the host stack, with no defined LO stack-overflow abort.

## 8. Control flow and statements

Source: [eval.rs](../compiler/src/interpreter/eval.rs), `Signal`, `Exec`, `exec_block`, `exec_stmt`, `invoke`.

```rust
pub enum Signal {
    Break,
    Return(Value),
    Abort(AbortKind),
}
pub type Exec<T> = Result<T, Signal>;
```

`Ok` means normal completion. `Err` carries a nonlocal transfer of control, including ordinary language constructs such as `return`. Rust's `?` propagates a signal through nested expressions and blocks until the correct boundary handles it.

| Signal | Handler | Effect |
|---|---|---|
| `Break` | Nearest executing `while` body | Stops that loop and resumes after it |
| `Return(value)` | Current method's `invoke` | Becomes the method call's result |
| `Abort(kind)` | Top-level `run` | Becomes `Outcome::Abort(kind)` |

`exec_block` executes statements in order and stops on the first signal. It reuses the current frame.

| `TypedStmt` variant | Execution |
|---|---|
| `Assign` | Evaluate the right side, then write through the target's binding |
| `Return` | Evaluate its expression and produce `Signal::Return` |
| `If` | Evaluate the Boolean condition and execute exactly one statement list |
| `While` | Re-evaluate the condition each iteration; catch `Break` from its body; propagate return and abort |
| `Break` | Produce `Signal::Break` |
| `Empty` | Complete immediately |
| `CallStmt` | Execute the call and discard its result |

A user method's `invoke` builds a frame and executes its body. A returned value is unwrapped into `Ok(value)`, while fallthrough produces the void sentinel. A break escaping a method is an invariant failure. Constructor execution likewise treats escaping returns and breaks as impossible; a break handled inside a constructor's loop does not escape.

## 9. Expression evaluation and order

Source: [eval.rs](../compiler/src/interpreter/eval.rs), `eval_expr`, `eval_args`, `eval_obj_name`.

`eval_expr` covers the entire `TypedExpr` enum:

| Expression | Behavior |
|---|---|
| `Num`, `Bool` | Return the corresponding scalar |
| `Str` | Intern literal text lazily and return its string handle |
| `Null` | Return `Obj(None)` |
| `This` | Return the current receiver handle |
| `Var` | Read through the annotated binding |
| `New` | Evaluate arguments, allocate an object, execute its constructor, return its handle |
| `Call` | Delegate to call resolution and invocation |
| `Unop` | Evaluate the operand, then apply its operator |
| `Binop` | Handle short-circuit logic or evaluate both operands and apply the operator |
| `Ternary` | Evaluate the condition and only the selected branch |
| `Cast` | Evaluate the operand, then apply the classified cast |
| `InstanceOf` | Evaluate the operand and test its runtime class |

Order is observable because expressions can read input, mutate objects through calls, allocate, or abort:

1. Ordinary binary operands evaluate left to right.
2. Argument lists evaluate left to right through `eval_args`.
3. Virtual and I/O calls evaluate and null-check the receiver **before** evaluating arguments. A null receiver prevents argument side effects.
4. `new C(...)` evaluates all arguments **before** allocating `C`.
5. Assignment evaluates its right side before changing the target.
6. `&`, `|`, and ternary expressions skip branches whose results are unnecessary.

Receiver syntax has its own `TypedObjName`: `This` and `Super` use the current receiver; `Var` uses binding-directed access; `Computed` evaluates an arbitrary contained receiver expression. Resolving the receiver value and selecting the method implementation are separate steps.

## 10. Operators and strings

Sources: [eval.rs](../compiler/src/interpreter/eval.rs), `eval_unary`, `eval_binary`, `eval_int_binop`, `lo_div`, `lo_rem`; [strings.rs](../compiler/src/interpreter/strings.rs).

### 10.1 Integer and Boolean behavior

| Operator/case | Implemented result |
|---|---|
| Integer `+`, `-`, `*` | Signed 32-bit wrapping arithmetic |
| Integer unary `~` | Wrapping negation |
| `a / b`, nonzero divisor | Division truncating toward zero; wrapping overflow behavior |
| `a / 0` | `-1` |
| `i32::MIN / -1` | `i32::MIN` |
| `a % b`, nonzero divisor | Remainder with dividend's sign when nonzero |
| `a % 0` | `a` |
| `i32::MIN % -1` | `0` |
| Integer `<`, `>`, `=` | Signed comparisons producing `Bool` |
| Boolean `!` | Logical negation |
| Boolean `a & b` | Skip `b` if `a` is false |
| Boolean `a | b` | Skip `b` if `a` is true |

The wrapping functions preserve LO's specified arithmetic in both debug and release builds. Division and remainder explicitly handle zero before calling Rust's wrapping operations. These arithmetic cases do not produce runtime aborts.

The evaluator dispatches overloaded operators from `Value` variants. It supports string multiplication only as `String * int`, not the reversed operand order. Boolean equality is not a supported checked operator.

### 10.2 Null equality

The checker permits equality between a class value and a null-typed operand, or between two null-typed operands. The evaluator implements the object branch by comparing `Option<ObjId>`: two nulls are equal, and a nonnull object differs from null. The branch could compare any two object handles internally, but ordinary checked programs cannot use unrestricted object-to-object equality. A both-branches-null ternary also has the checker's null type.

### 10.3 String operations

| Helper | Mechanics | Allocation |
|---|---|---|
| `concat` | Obtain both source buffers, create a capacity-sized host string, append both texts | Allocate the result in the heap |
| `repeat` | Reject negative count; return empty for zero; otherwise repeat source text | Allocate a nonempty result; empty results reuse the singleton |
| `reverse` | Collect `chars().rev()` into a host string | Allocate the result |
| `compare` | Compare source UTF-8 byte slices lexicographically | No new heap string |

String `+` uses `concat`, string unary `~` uses `reverse`, and string `<`, `>`, `=` translate `Ordering` into a Boolean. Equality compares contents even when handles differ.

Reversal operates on Unicode scalar values, not bytes or user-perceived grapheme clusters. For example, `aé` becomes `éa` without corrupting UTF-8; combining marks are independently reversed. Comparison uses byte ordering, with no locale or normalization step.

Any negative repeat count aborts with code 120, including when the input string is empty. Result allocation can abort with code 137. Empty results reuse the uncharged canonical string.

## 11. Method dispatch

Source: [eval.rs](../compiler/src/interpreter/eval.rs), `eval_call`, `body`, `invoke`.

### 11.1 Virtual calls

For `MethodResolution::Virtual`:

1. Evaluate the receiver and reject null with `NullReceiver { method }`.
2. Evaluate arguments from left to right in the caller frame.
3. Read the receiver object's runtime class from the heap.
4. Find `effective_methods[method_name]` for that class.
5. Read the winning entry's declaring `owner`.
6. Find `method_bodies[owner][method_name]`.
7. Invoke that declaration with the original object handle and a fresh frame.

For example, an `Animal` variable containing a `Dog` object calls the `Dog` override if one exists. The call's `static_class` records checking context; runtime selection uses the object's actual class.

### 11.2 `super` calls

`MethodResolution::Super { declaring_class }` uses the exact owner selected by the checker. It evaluates arguments and invokes that owner's body with the current `this`. It bypasses virtual selection for this call, but does not change the object's runtime class. A virtual call made inside the ancestor body can therefore still select a descendant override.

### 11.3 Built-in calls

`MethodResolution::Io` evaluates and null-checks its receiver, evaluates arguments, and calls `run_io`. It does not create a user-method frame or consult the method-body index. Null `Input` and `Output` variables abort just like other null receivers; the prebound singleton names themselves are always nonnull.

## 12. Object construction and delegation

Source: [eval.rs](../compiler/src/interpreter/eval.rs), `construct`, `select_ctor`, `ctor_arity`, `run_ctor`, `run_delegation`.

### 12.1 Allocation and constructor selection

After the caller evaluates arguments, `construct` obtains the target class's `effective_fields`, generates each field's type-default, and allocates one object containing the full inherited layout. It then selects a constructor and runs it on that same handle.

`select_ctor` scans the class's constructor list for the requested arity. An explicit constructor's arity is `formals.len()`; an implicit constructor's arity is `fields.len()`. The checker ensures there is a unique valid target. Constructors are selected from the specified class, not inherited as methods are.

### 12.2 Implicit constructors

The checker synthesizes `TypedConstructor::Implicit { fields }` for eligible root classes without explicit constructors. Execution directly assigns each argument to its corresponding field using `field_slot`. This path has no explicit-constructor frame or source statement list. It initializes fields positionally rather than performing bare-name resolution.

### 12.3 Explicit constructors

An explicit constructor creates a frame with the same receiver, its own parameters, and its own defaulted locals. It executes its delegation prefix completely, if present, then executes its statements.

- `ThisCall`: evaluate arguments in the current constructor frame, select a same-class constructor by arity, and recursively run it on the same object.
- `SuperCall`: evaluate arguments in the current frame, find the parent of the class whose constructor is executing, and run the parent's selected constructor on the same object.

No delegation step allocates a second object. The class parameter tracks the currently executing constructor's declaring class; the object's stored runtime class continues to identify the most-derived allocation.

The checker rejects cyclic `this(...)` delegation and requires an inheriting constructor to begin with `this(...)` or `super(...)`. Root constructors may have no delegation; a root `this(...)` chain terminates at such a constructor.

### 12.4 Body execution order

Delegated constructor bodies finish before the delegating body's statements begin. For example, if `Child()` delegates to `Child(1)`, and `Child(int)` delegates to `Parent()`, execution is:

```text
allocate one Child, default every field
  enter Child()
    evaluate this(1)
    enter Child(int)
      evaluate super()
      run Parent() delegation/body
      run Child(int) statements
    run Child() statements
return the original Child handle
```

Multiple constructor bodies from one class can execute in a `this(...)` chain. Describing this as exactly one body per hierarchy level is incorrect. Also, although allocation initially defaults all fields, delegation argument expressions and virtual method calls can have effects; parent-first execution alone does not guarantee descendant fields remain untouched until their constructor body starts.

If construction aborts, the signal propagates and no object result is delivered to the caller. Earlier heap mutations and I/O are not rolled back. The allocation remains in the arena until the overall run ends.

## 13. Casts and `instanceof`

Sources: [eval.rs](../compiler/src/interpreter/eval.rs), `eval_cast`, `is_instance`; [type_checker.rs](../compiler/src/type_checker.rs), `ClassTable::is_subtype`.

An upcast or a cast classified as `Null` returns the evaluated value unchanged. A downcast first accepts null; otherwise it retrieves the actual class and checks whether it equals or descends from the target class. Success preserves the same handle. Failure creates `CastFailed { from, to }` with the runtime source class and target class names.

`instanceof` returns false for null. For a nonnull object it performs the same subtype query and returns a Boolean. The test itself does not abort, although evaluating its operand can.

`ClassTable::is_subtype(a, b)` tests equality and then scans `a`'s precomputed ancestor list. It does not reconstruct the hierarchy or walk parent pointers afresh. Unrelated casts rejected statically never reach these helpers.

## 14. Input and output

Sources: [io.rs](../compiler/src/interpreter/io.rs), `Io`; [eval.rs](../compiler/src/interpreter/eval.rs), `run_io`; [add_io_classes.rs](../compiler/src/add_io_classes.rs).

### 14.1 Stream ownership and routing

`Io` owns `Box<dyn BufRead>` for input and two `Box<dyn Write>` streams for output and error output. This permits real process streams and in-memory test streams behind the same interface. The reader's buffering supports peeking without consumption for `eof`.

Both `out` and `err` have class `Output` and no fields. `run_io` selects stderr exactly when the receiver's handle equals `err_id`; otherwise output goes to stdout. Routing is based on identity, so passing `err` into an `Output` parameter preserves the selected stream without depending on the parameter's name.

### 14.2 Eight built-in operations

| Operation | Result and stream behavior |
|---|---|
| `print_int(n)` | Writes signed decimal digits; returns the void sentinel |
| `print_bool(b)` | Writes exactly `true` or `false`; returns the sentinel |
| `print_string(s)` | Writes the string's UTF-8 bytes; returns the sentinel |
| `println()` | Writes one newline; returns the sentinel |
| `read_int()` | Reads a token and parses an `i32`; malformed/out-of-range → 110, no token → 111 |
| `read_bool()` | Accepts exactly `true` or `false`; invalid token or EOF → 112 |
| `read_string()` | Reads through the next newline, removes that newline, and heap-allocates the result |
| `eof()` | Returns whether the input buffer is at end-of-input without consuming bytes |

Print operations append no newline except `println`. Each write flushes its selected stream, including stderr. `print_string` requires `Value::Str`; the interpreter does not accept null string arguments.

### 14.3 Token and line details

`read_token` first skips ASCII whitespace. It then repeatedly reads buffered chunks until whitespace or EOF, consuming token bytes but leaving the terminating whitespace unread. It decodes collected bytes using `String::from_utf8_lossy`.

`read_int` parses the whole token with Rust's `parse::<i32>()`. Optional signs are accepted; overflow and tokens such as `12x` are malformed. It does not consume just an integer prefix of a larger token.

`read_string` uses `read_until(b'\n')`. It removes only a trailing LF, so CRLF input retains the CR in the returned string. Immediate EOF returns the empty string; an empty input line also returns empty. Invalid UTF-8 becomes replacement characters. Reading a nonempty line is a charged string allocation.

The token delimiter rule matters when mixing input operations. After reading `42` from `42\n`, the newline remains: `eof()` is false, and `read_string()` returns an empty line. A loop guarded only by `!eof()` can attempt an extra integer read after the last token and abort with 111. Line reads consume their newline, avoiding that particular issue.

### 14.4 Host I/O errors and test capture

The current code ignores write/flush errors and does not define a separate LO abort for host I/O failures. `eof` treats a buffer error as EOF; token readers stop on buffer errors; line reads ignore the `read_until` result and use any collected bytes. This behavior limits what the interpreter can diagnose about failed host streams.

`SharedBuf` implements `Write` over `Rc<RefCell<Vec<u8>>>`. Tests retain one clone and pass another into `Io`, allowing them to inspect emitted output after execution. `contents()` returns a lossy UTF-8 string view of captured bytes.

## 15. Runtime aborts and CLI behavior

Sources: [abort.rs](../compiler/src/interpreter/abort.rs), [interpreter.rs](../compiler/src/interpreter.rs), [main.rs](../compiler/src/main.rs).

### 15.1 Defined aborts

| Kind | Producer | Code | Message |
|---|---|---|---|
| `CastFailed { from, to }` | Failed nonnull downcast | 101 | `lo_cast_check: cannot cast <from> to <to>` |
| `NullReceiver { method }` | Null virtual or I/O receiver | 102 | `lo_abort_null_receiver: cannot dispatch <method>` |
| `ReadIntMalformed` | Invalid or out-of-range integer token | 110 | `lo_read_int: malformed token` |
| `ReadIntEof` | No integer token remains | 111 | `lo_read_int: end of input` |
| `ReadBoolInvalid` | Invalid Boolean token, including EOF | 112 | `lo_read_bool: invalid token` |
| `RepeatNegative(n)` | Negative string repetition | 120 | `lo_string_repeat: negative count <n>` |
| `OutOfMemory` | Arena allocation exceeds its budget | 137 | `lo_alloc: out of memory` |

Low-level helpers return `Result<_, AbortKind>`, and the evaluator converts failures with `map_err(Signal::Abort)`. Aborts then propagate without requiring every statement to duplicate failure handling.

The interpreter returns the abort object; it does not print its diagnostic into injected stderr. The CLI renders the message. This separates LO's program-generated `err` output from the host driver's reporting responsibility and lets tests inspect outcomes directly. Earlier side effects remain observable after an abort.

### 15.2 Command-line entry

The implemented command is:

```sh
cargo run --manifest-path compiler/Cargo.toml -- --run path/to/program.lo
```

Run this from the `lo-runtime` directory. `main` recognizes exactly `--run FILE.lo` before its ordinary check/compile argument path. `run_interpreter` reads the file, tokenizes, parses, checks, and constructs process-backed `Io`.

File and front-end errors are printed to stderr and return failure status 1. A clean `Outcome::Exit(code)` becomes `ExitCode::from(code as u8)`, preserving the low eight bits of the LO integer result. An abort prints `abort.message()` with a newline and returns `abort.code()` as the process status.

The CLI always uses `DEFAULT_HEAP_LIMIT`. The callable interface supports custom budgets, but `--heap-limit` is not implemented. The general usage text also omits `--run`, even though the dedicated argument branch handles it.

## 16. Worked execution trace

The whole-program tests include this object pattern:

```text
class Counter (int n;) [ Counter(int start) { n = start; } ] {
    int get() { return n; }
    void bump() { n = (n + 1); }
}
class Main () {
    int main() {
        Counter c;
        c = new Counter(10);
        c.bump();
        c.bump();
        return c.get();
    }
}
```

1. The checker records `c` as a local, `start` as a formal, and `n` in these method bodies as a field. It supplies the `Counter` method entries and constructor declaration.
2. Startup creates the empty string and I/O singletons, then allocates and constructs `Main`.
3. Invoking `main` creates a frame whose `c` slot starts as null.
4. Evaluating `new Counter(10)` first evaluates `10`, then allocates a `Counter` with `n = 0`. Its one-field allocation costs 16 modeled bytes.
5. The constructor frame binds `start = 10`. Assignment reads that formal and writes the object's `n` slot.
6. The returned object handle is assigned to `c`.
7. Each `bump` call reads `c`, checks it is nonnull, resolves the `Counter` body, creates a receiver frame, and increments the shared object's field. Its void result is discarded.
8. `get` reads `n = 12` and produces a return signal, which its invocation converts to an expression value.
9. `main` returns 12; `run` produces `Outcome::Exit(12)`, and the CLI returns process status 12.

This example connects the front-end annotations, frame slots, heap identity, constructor initialization, dispatch, arithmetic, and control-flow signals without any generated machine code.

## 17. Reconciliation with earlier planning

The planning files remain useful rationale, but the following details should not be copied into a final design unchanged:

| Planning statement or sketch | Current implementation |
|---|---|
| CLI wiring is future/out of scope | `main.rs` implements `--run FILE.lo` |
| Heap budget can be overridden with `--heap-limit` | Only the internal `interpret` argument supports a custom budget |
| Generic `Io<R, W, E>` or borrowed `&mut Io` | `Io` holds boxed trait objects and is passed by value |
| Orchestration/startup indices live in `interpreter.rs` | They live in `eval.rs`; the root is a thin wrapper and test module |
| A per-method slot map is built once | `Frame::new` builds a new map per invocation |
| One constructor body runs per hierarchy level | `this(...)` runs the target body and then the caller body, potentially several bodies in one class |
| Every `this(...)` chain reaches `super(...)` | Root-class chains can end at a constructor without delegation |
| All class operands are rejected by comparison | Null equality is supported by both checker and evaluator |
| Null `print_string` arguments print nothing | Checked `String` values are nonnull; `run_io` accepts only `Value::Str` |
| Strings are generically described as interned | Literal text is cached; nonempty dynamic results are fresh charged cells |
| Subtype tests walk parents at runtime | The current query scans the precomputed ancestor vector |
| Simplified object class IDs or boxed names | Heap objects hold runtime class names as `Rc<str>` |
| Budget guarantees an OOM outcome for allocation | Arena accounting is checked, but temporary host allocations occur before some checks |

The plans also discuss compiled stderr support and runtime fidelity. Those are cross-backend concerns, not unresolved implementation steps inside the interpreter's existing identity-based stderr routing. This document does not treat historical claims about the emitter as a fresh audit of that backend.

## 18. Validation, coverage, and implementation limits

### 18.1 Existing tests

The interpreter tests reside beside their implementations. They combine direct helper tests, manually constructed typed expressions, and programs compiled through the real lexer/parser/checker.

| Test location | Main coverage |
|---|---|
| `interpreter.rs::tests` | Entry return, printing, factorial loop, object mutation, cast abort outcome, input computation |
| `value.rs::tests` | Scalar, string, and class defaults |
| `heap.rs::tests` | Allocation formulas, uncharged empty string, rejected allocation state, field access |
| `strings.rs::tests` | Concatenation, repetition edges, Unicode reversal, content comparison |
| `abort.rs::tests` | Exit codes and exact diagnostic formats |
| `eval.rs::tests` | Arithmetic edges, short-circuiting, ternary selection, literals, assignments, loops/breaks, dispatch, constructors, shadowing, casts, `instanceof`, I/O and null I/O receivers |

Validation while preparing this document:

```text
Command (from compiler/): cargo test interpreter::
Result: 54 passed; 0 failed; 91 filtered out
```

This run validates the existing interpreter suite, not all compiler tests or full interpreter-versus-WASM equivalence. The repository's [test-conformance.py](../scripts/test-conformance.py) exercises checking, WASM emission, assembly, linking, and WASM execution; it does not invoke `--run` or provide differential interpreter comparison.

### 18.2 Boundaries to retain in a shorter design

- **Trusted checked input:** invalid annotations can produce internal panics rather than LO aborts.
- **No collection:** the heap budget measures cumulative modeled allocations, not live memory.
- **Host resource limits:** recursion uses the Rust stack, and temporary allocations can precede budget checks.
- **I/O error handling:** host stream failures are ignored or treated like EOF; no dedicated error outcome exists.
- **Diagnostic scope:** runtime abort messages have categories and selected names/values, but no LO stack trace or source-line reporting.
- **Lookup costs:** frame maps are rebuilt per invocation; fields and constructor overloads are linearly searched; subtype queries scan ancestor vectors. This favors straightforward implementation at course-program scale.
- **Parity evidence:** passing unit tests establishes covered behavior; it does not establish identical memory exhaustion or all input behavior across backends.

Useful further coverage would include multi-step `this(...)` delegation, argument side-effect order around null receivers, mixed token/line reads, CRLF and invalid UTF-8 input, and passing `err` through an `Output` formal. These are review/testing opportunities, not features claimed to have been added by this documentation pass.

## 19. Design rationale in one place

The architecture keeps execution small by reusing semantic decisions from the checker. Binding annotations eliminate repeated lexical resolution; flattened method ownership avoids repeated ancestor searches for calls; constructor and cast annotations separate static legality from runtime effects.

The arena makes object identity and mutation explicit, supports cyclic object graphs through handles, and provides one accounting point for objects and strings. It leaves a possible future collector a stable handle model, although collection would still require tracking roots in currently active frames and temporary values.

Separate frames express per-call state; a unified signal type handles early exits through deeply nested AST execution. Injectable I/O and returned outcomes make complete programs testable without terminating the test process. Small string and abort modules concentrate behavior whose exact bytes or edge cases matter to external observations.

Together, these choices produce a direct executable interpretation of the checked program, with the same semantic metadata available to the separate compiler backend and clear locations for investigating disagreements.
