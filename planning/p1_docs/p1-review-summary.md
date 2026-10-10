# P1 Review Summary — Design Decisions Refresher

Source: `planning/*_design.md` + `compiler/src/`. Line references are to files
under `compiler/src/` unless noted otherwise. This is a refresher for the team
code review, not a new design document — see the linked `_design.md` files for
full rationale and the `_implementation.md` files for the module map.

Pipeline: **Lexer → Parser → Type Checker → (Interpreter | WASM Emitter)**.
Each phase trusts the one before it completely — the parser assumes valid
tokens, the checker assumes valid syntax, and both backends assume a
type-checked AST is well-formed (states the checker rules out become
`unreachable!()`, not runtime errors).

---

## 1. Lexer (`lexer.rs`, `token.rs`)

- **Whole-file scan into `Vec<Token>`, not streaming.** The parser needs
  LL(2) lookahead; a vector + cursor gives free `peek`/`peek2` with no
  hand-built ring buffer.
- **`Token { kind, line }`** — one line field on the wrapper, not one per
  `TokenKind` variant, since every token needs exactly one line number.

  ```rust
  // token.rs:2-5
  pub struct Token {
      pub kind: TokenKind,
      pub line: u32,
  }
  ```

- **Hand-written scanner, no lexer-generator crate.** Matches the course's
  "build the parser yourself" ethos; LO's lexical grammar is small enough
  that a generator buys little.
- **Keywords are recognized only after maximal-munch identifier scan**, then
  checked against a fixed table — no match falls back to `Ident`. This is
  why `String` is a *keyword*, not an identifier:

  ```rust
  // token.rs:8-30 (TokenKind)
  pub enum TokenKind {
      Ident(String),
      Num(i32),
      Str(String),
      KwInt, KwBool, KwString, KwVoid,
      KwClass, KwExtends, KwThis, KwSuper, KwNull, KwNew,
      KwReturn, KwIf, KwElse, KwWhile, KwBreak,
      KwTrue, KwFalse, KwInstanceof,
      LParen, RParen, LBrace, RBrace, LBracket, RBracket,
      Semicolon, Comma, Dot, Question, Colon, Equals,
      Plus, Minus, /* ... */
  }
  ```

  Side effect: `class C extends String` fails at the **parser**, not the
  type checker, because `String` can never fill the `ClassName` slot.

- **`in`, `out`, `err`, `Main`, `Input`, `Output` are NOT keywords.** They
  lex as ordinary `Ident` and are only flagged as reserved later, during
  type checking (`E_RESERVED_VARIABLE_NAME` / `E_RESERVED_CLASS_NAME`). They
  never appear as literal tokens in the LO-4 grammar, so making them
  keywords would mean lexing something the grammar doesn't define.
- **No `==`, no `&&`/`||`.** `=` is the only equals token (context decides
  assignment vs. equality); `&`/`|` are single-character short-circuit
  operators. LO has no doubled operators at all.
- **String escapes decoded during scanning**, not deferred — `\u{...}`
  range validation (reject surrogates, reject codepoints above U+10FFFF) is
  a lex-phase compile error per spec, so it has to happen while the string
  is being scanned.
- **Line only, no column** — no P1 diagnostic policy calls for column
  tracking, and the course's own P4/DWARF chapter rejected column tracking
  as "cost without P4 payoff," which is decent evidence line-only is enough
  even beyond P1.

---

## 2. Parser (`parser.rs`)

- **Recursive-descent, hand-built, strictly LL(2)** — only `peek1()` /
  `peek2()` (`parser.rs:850`), no backtracking, no generator.
- **Entry point**: `parse_program` (`parser.rs:69`) implements
  `Program -> (ClassDecl)*`; `parse_class_decl` (`parser.rs:89`) implements
  the full class production.
- **Every AST node carries its own `line`.** `Type::String` is its own
  variant, not `Class("String")` — keeps `String` out of the class
  hierarchy at the type level from the start.

### The four documented LL(2) decision points

1. **Ident-led ambiguity** — `VarDecl` vs. assignment vs. call statement all
   start with one `Identifier`. Resolved by `peek2()`: another `Identifier`
   → `VarDecl`; `=` → assignment; anything else → call statement. See
   `parse_stmt` (`parser.rs:350`) and `is_start_of_var_decl`
   (`parser.rs:904`).
2. **`this`-led** — bare `this` vs. `this.method(...)`, resolved by
   checking for `.` right after `this` via `peek2()`.
3. **The hard one: `( (Identifier) ...)` — cast vs. a redundantly
   parenthesized `Var`.** `((Dog)obj)` (a cast) and `((x))` (a parenthesized
   variable) are indistinguishable for the first three tokens
   `( Identifier )`, and because whatever is inside the inner `()` can
   itself be arbitrarily deeply nested (`((((Dog)))obj)`), **no fixed k
   tokens of lookahead resolves this in general** — it is not really an
   LL(2)-vs-LL(k) problem, because LL(2) only promises 2 tokens of
   lookahead *at each decision point*, not from the start of the construct.
   The fix: don't look further ahead — **parse through the ambiguity** as
   an ordinary `Var` (valid either way), then reinterpret based on what
   immediately follows:

   ```rust
   // parser.rs:595-617 (parse_paren_suffix)
   fn parse_paren_suffix(&mut self, primary_expr: Expr, line: u32) -> Result<Expr, ParseError> {
       // P29: Expr -> ( ( Type ) Expr ), Type -> ClassName
       if let Expr::Var(identifier, _) = &primary_expr {
           if self.is_start_of_expr() {
               let value = self.parse_expr()?;
               self.expect(TokenKind::RParen, ErrorCode::EParsePhaseOther)?; // ")"
               return Ok(Expr::Cast(
                   Type::Class(identifier.clone()),
                   Box::new(value),
                   line,
               ));
           }
       }
       // P23: Expr -> ObjName . MethodName ( (Actuals)? ), via P49: ObjName -> ( Expr )
       if self.check(&TokenKind::Dot) {
           let obj_name = ObjName::Computed(Box::new(primary_expr), line);
           let call = Expr::Call(self.parse_method_call_suffix(obj_name, line)?);
           return self.parse_paren_close_or_operator(call, line);
       }
       self.parse_paren_close_or_operator(primary_expr, line)
   }
   ```

   If an expression immediately follows the parsed `Var` with no connector
   token, only a cast produces that shape, so the `Var`'s name is
   reinterpreted as the cast's `Type`. This is the single best "why is your
   parser structured this way" answer to have ready — it demonstrates the
   team understood the LL(k) limits of the grammar, not just implemented
   around them.
4. **Constructor delegation position** — `this(...)`/`super(...)` is not
   actually a second production sharing a prefix; it's a shape that must be
   *actively rejected* anywhere outside the one legal slot at the top of a
   constructor body. Implemented in `misplaced_delegation_error`
   (`parser.rs:819`), called from the top of `parse_stmt` before any `Stmt`
   dispatch.

- **Locals are hoisted at parse time.** Every `VarDecl` found at any nesting
  depth inside a method/constructor body is flattened into the enclosing
  `BodyScope.locals`, and `E_DUPLICATE_LOCAL` is checked right there during
  hoisting — this is why nested blocks in the AST carry no scope of their
  own.

---

## 3. Type Checker (`type_checker.rs`, `type_checker/*.rs`)

- **Four gated, fail-fast passes** — each stops at its *first* error rather
  than accumulating diagnostics, because later passes depend on earlier
  results being valid (no point type-checking bodies against an inheritance
  graph that has a cycle in it):

  ```rust
  // type_checker.rs:23-30
  pub fn check_program(program: Program) -> Result<(TypedProgram, ClassTable), TypeError> {
      validate_user_declared_names(&program)?;
      let program = crate::add_io_classes::add_io_classes(program);
      let mut table = gather_declarations(&program)?;      // Pass 1: declarations
      resolve_inheritance(&mut table)?;                     // Pass 2: inheritance
      check_entry_point(&table)?;                           // Pass 3: entry point
      let typed = check_bodies(&program, &table)?;          // Pass 4: bodies
      Ok((typed, table))
  }
  ```

  Note the reserved-name check (`validate_user_declared_names`) runs
  **before** the synthetic `Input`/`Output` preamble classes are injected
  (`add_io_classes`) — so a user can't accidentally collide with the names
  the preamble is about to add.
- **Separate typed AST (`TypedProgram`)** rather than annotating the
  parser's tree in place. Keeps the parser's output and the checker's
  semantic output decoupled — both the interpreter and the WASM emitter
  consume the same `TypedProgram`/`ClassTable` pair.
- **Vtable slots computed parent-first**, in `resolve_inheritance`
  (`type_checker/inheritance.rs:8`) → `compute_effective`
  (`type_checker/inheritance.rs:88`): an inherited field keeps its parent's
  index, an override reuses its parent's vtable slot, and a genuinely new
  method appends a slot.

  ```rust
  // type_checker/class_table.rs:13-30
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

      // Method names in vtable-slot order: an inherited method keeps its
      // parent's slot, an override reuses that slot, and a genuinely new
      // method is appended. `method_slot[name]` is that name's index here.
      pub vtable: Vec<String>,
      pub method_slot: HashMap<String, usize>,
  }
  ```

  This is the single piece of shared infrastructure both backends rely on
  for identical dispatch behavior — good talking point for "how do the
  interpreter and the WASM backend agree on method dispatch."
- **`String` is never part of class inheritance** — it's primitive-
  equivalent in the type system; `null` is only assignment-compatible with
  class-typed targets, never with `String` (whose default is the empty
  string).
- **Ternary branch types resolve via least common ancestor**: walk one
  branch's ancestor chain fully, then walk the other from most-specific to
  most-general and take the first match. `Cat`/`Dog` under `Animal` →
  `Animal`; disjoint hierarchies are rejected.
- **Rule → pass → diagnostic table** in `planning/type_checker_design.md`
  maps every `E_*` error code to the exact pass that raises it — the fastest
  way to answer "what stage catches X" during review.

---

## 4. Interpreter (`interpreter.rs`, `interpreter/*.rs`)

- **Entry point**: `interpret` (`interpreter.rs:23`), taking the checked
  `TypedProgram` + `ClassTable` — it never re-validates; anything the
  checker made impossible is `unreachable!()`.
- **Values are self-describing** — operator dispatch reads the runtime
  value's own kind (`Value::Int` vs. `Value::Str`), not a static type,
  except for the handful of cases the checker had to pre-resolve anyway
  (`super` dispatch, cast direction) because no runtime information could
  supply them.

  ```rust
  // interpreter/value.rs:6-14
  pub enum Value {
      Int(i32),
      Bool(bool),
      // A string handle — never null (`String` is primitive-equivalent; its
      // type-default is the empty string, and `null` is only class-compatible).
      Str(StrId),
      // A class-typed value; `None` is LO null.
      Obj(Option<ObjId>),
  }
  ```

  The comment on `Str` captures a common gotcha: **`String` locals can
  never hold `null`** — `Obj(None)` is the *only* null representation.
- **Objects/strings live in an arena** addressed by handles
  (`interpreter/heap.rs:32,37` — `Cell`/`Heap`), not native `Rc`/`Box`.
  LO programs can build reference cycles freely (the OOM conformance test
  does this on purpose with a linked list), and `Rc` would leak every
  cycle — so a byte budget on the arena is what gives deterministic OOM
  parity (exit 137) with the real runtime. **No GC in the interpreter** —
  documented as a deliberate fidelity gap, mitigated by setting the budget
  comfortably above what any conformance program needs.
- **Integer arithmetic is total and wrapping**, matching LO semantics
  exactly rather than Rust's panicking defaults:

  ```rust
  // interpreter/eval.rs:671-673
  Binop::Add => Value::Int(a.wrapping_add(b)),
  Binop::Sub => Value::Int(a.wrapping_sub(b)),
  Binop::Mul => Value::Int(a.wrapping_mul(b)),
  ```

  Division/remainder special-case `x/0 = -1`, `x%0 = x`, and
  `INT_MIN / -1 = INT_MIN` (`wrapping_div`) before falling through to
  `wrapping_div`/`wrapping_rem` otherwise (`interpreter/eval.rs:686-701`).
- **One `Signal` enum drives all non-local control flow** (`Break`,
  `Return`, `Abort`) so `?` propagates uniformly through the evaluator
  (`interpreter/eval.rs:22`). Aborts are **returned** as `Outcome::Abort`,
  never `process::exit`'d directly, which is what makes them unit-testable.
- **Seven abort codes**, each with a contractual stderr message prefix the
  conformance harness substring-matches:

  ```rust
  // interpreter/abort.rs:2-31
  pub enum AbortKind {
      CastFailed { from: String, to: String },   // 101
      NullReceiver { method: String },            // 102
      ReadIntMalformed,                           // 110
      ReadIntEof,                                 // 111
      ReadBoolInvalid,                             // 112
      RepeatNegative(i32),                        // 120
      OutOfMemory,                                // 137
  }
  ```

- **Frames are flat, no scope stack.** Because the parser already hoists
  every local and LO forbids both local/formal collisions and field
  shadowing, name resolution is a fixed lookup order —
  locals → formals → fields → `in`/`out`/`err` — decided once per body, not
  pushed/popped per block.

---

## 5. WASM Emitter (`compiler/src/wasm/*.rs`, `planning/wasm-emitter-design.md`)

- **Same input as the interpreter** — `TypedProgram` + `ClassTable` — one
  typed IR feeding two independent backends, which is why the checker's
  `MethodResolution` (`Virtual`/`Super`/`Io`) and vtable-slot layout matter:
  both backends read them directly instead of re-deriving dispatch.
- **All LO values are i32** in linear memory; only `class C` and `String`
  values are GC roots (int/bool/void are not).
- **Explicit shadow-stack root-frame protocol** — every reference-typed
  local gets a slot in a runtime-managed frame; `PUBLISH`/`RELOAD` wrap
  every call so the collector can see and rewrite references even though
  WASM locals and the operand stack themselves aren't scannable by the GC.
- **One uniform `CALL` protocol** shared by user method calls, constructor
  calls, and runtime calls: publish roots → push saved arguments → call →
  save result → reload roots. This mirrors the checker's
  `MethodResolution` directly — `Virtual` becomes a vtable-slot load plus
  `call_indirect`; `Super` becomes a direct, non-virtual call to the
  resolved ancestor implementation (no dispatch, no null-vtable-lookup
  needed since the checker already pinned the target).

---

## Likely review angles

Given how much of the planning docs is "Decisions" and "Alternatives"
tables, the questions most likely to come up:

1. Why hand-written lexer/parser instead of a generator crate.
2. Why LL(2) suffices — especially the cast-vs-parenthesized-`Var`
   ambiguity (decision point 3 above) and why it *isn't* really an LL(k)
   problem.
3. Why the checker uses gated fail-fast passes instead of accumulating all
   diagnostics at once.
4. Why the interpreter uses an arena instead of `Rc`/`Box`, and why it has
   no garbage collector while the WASM backend does.
5. How the interpreter and WASM backend stay consistent on virtual
   dispatch (answer: they share the checker's vtable-slot assignment and
   `MethodResolution`, computed once in Pass 2).
