# P4 — Compiler changes for the `-g` build

This doc lists everything the compiler (`compiler/`) needs so a `-g` build can be debugged. It covers the compiler half of P4 only; the debugger is planned separately.

- Based on `origin/main` (e96e41e).
- Reviewed in two rounds of four verification passes each, then reconciled against Chapter 9 (the DWARF Subset Reference). Round 1 covered spec coverage, front end and emission, named variables and GC, and an end-to-end trace. Round 2 covered a whole-doc consistency check, a line-table simulation on real compiler output, an implementability check of the allocator, CFI and DWARF, and toolchain runs (LLVM only).
- Paths are relative to `compiler/src` unless noted otherwise.

## Ground rules

1. **Nothing changes unless `-g` is set.** That covers the `.loc`/CFI/`.type`/`.size` directives, the `.Lfunc_end` labels, root-slot numbering, prologue order, allocation, dead-copy removal, and the `--dump-ir` text.
   - Data that is carried in both modes (side tables, new IR fields) must not change default output.
   - The interpreter and wasm backends must not change at all.
   - **One deliberate exception: G13** strips debug info from the runtime archive, so default-mode *linked binaries* change. Their code doesn't. The default `.s` stays byte-identical.
2. **Regression gate (G17).** In default mode, all of these must be byte-identical before and after the change:
   - `--emit-asm` over the whole corpus;
   - `--emit-asm --spill-all`;
   - `--dump-ir`.

   Also, `cargo test` must pass. `ir/tests.rs` checks exact dump text and allocations.
3. **Assemble `-g` output with `as --gdwarf-5`; keep everything else in the `.s`.** Ch. 9 §9.1 says: "The -g debug object is assembled with `as --gdwarf-5`". That flag is the only one `-g` may rely on; every other piece of debug info must be in the `.s`. The default build keeps today's `as` invocation. The P3 grading harness assembles our `.s` itself (`as --64`, then `cc -no-pie`), and our own driver links with `gcc -static`, so test `-g` with both link paths.
4. **The spec allows `-g` to be slower:** "the register allocator's clever output is for the default build; the -g build pins named values to the stack".

**The DWARF Subset Reference is Chapter 9 of `LO-language-reference.pdf`** (pp. 65–73), cited below as §9.x. It is the binding producer/consumer contract. Where it settles a choice, this doc follows it. The few remaining **[triples]** marks are behavior that only the published conformance triples can settle.

---

## 0. Spike first: does the toolchain accept the line table? (do this before G10)

- [x] **G0 Toolchain confirmed in `lo-verify`** (GNU as 2.40, gcc 12.2, Debian; run 2026-11-26 via `scripts/in-container.sh`). A hand-written sample `.s` had a recursive `fact`, a `main` wrapper, `.file 0`/`.file 1`, `.loc` rows, the three-state CFI, and a v5 `.debug_info` in this doc's forms. Results:
  - **Plain `as --64` and `as --64 --gdwarf-5` both work.** `.file 0` + `.file 1 "t.lo"` (same name) is accepted, and `.file 0` alone makes gas emit a **v5** line table. We still pass `--gdwarf-5` under `-g`, per §9.1.
  - **gas adds no CU of its own** when `.debug_info` is hand-written: exactly 1 CU in the `.o` and in both linked binaries. glibc's crt files add none to a static link either.
  - **`stmt_list` via the `.Ldebug_line0` label works.** `is_stmt`/`prologue_end` come out exactly as written, as one sequence with one `end_sequence`.
  - **`llvm-dwarfdump --verify`: no errors** on the `.o`, on the `cc -no-pie` binary and on the `gcc -static` binary. Both binaries run and exit 24.
  - **`gcc -static` drops `.eh_frame_hdr`; `cc -no-pie` keeps it.** Under `-g`, link with `-Wl,--eh-frame-hdr` (verified: it restores the section).
  - **gas silently drops every `.loc` with line 0.** No row is emitted, so code after it inherits the previous row. That is why G10 now orders generated code first instead of using line-0 rows.
  - **GNU `objcopy --strip-debug` on the runtime archive works** (G13): 22 MB → 13 MB, 312 → 0 members with `.debug_info`, and all 286 `.eh_frame` members kept.
  - **gdb is not in the `lo-verify` image**, so G14's gdb smoke test has to run on the course x86 host.
  - From the earlier LLVM run: LLVM's `llvm-mc` rejects `.file 1` with the same name as `.file 0`. That only matters if someone assembles with LLVM; the grader uses gas.

---

## A. Plumbing

- [ ] **G1 Add a `-g` flag and a `DebugMode` threaded through every stage.**
  - **Today:**
    - `parse_options` (`codegen/driver.rs`) treats an unknown `-g` as a positional argument, so you get the USAGE error.
    - No stage takes options.
    - `build_ir` throws away `typed`/`classes`.
    - `emit_program` gets only the `ProgramIr`, not the source path.
  - **Change:**
    - Parse `-g` for `--emit-asm` and `--native`. Check the exact spelling against the P4 grading contract once it's published.
    - Under `-g`, `assemble_and_link` passes `--gdwarf-5` to `as` (§9.1) and `-Wl,--eh-frame-hdr` to `gcc -static`. Without that flag, `gcc -static` omits `.eh_frame_hdr`, which §9.1 lists (verified in G0). The default invocation doesn't change.
    - Thread the mode through `build_ir → lower_program → insert_gc_roots → remove_dead_copies → allocate → emit_program → select_function → Frame`.
    - Pass the source path down, for `.file` and the CU's name and comp_dir.
  - *deps: none*

- [ ] **G2 Keep declaration lines for params, locals and fields, using side data.**
  - **Today:** `typed_ast.rs` flattens formals and locals to `(String, Type)`, so the parser's `Formal.line`/`VarDecl.line` are lost. `FieldInfo` has no line, though `own_fields` keeps `(String, Type, u32)`.
  - **Change:**
    - Add `formal_lines`/`local_lines: Vec<u32>` to the `TypedMethodDecl` *struct*, not inside `TypedMethodBody::UserDefined`. Also add them to `TypedConstructor::Explicit`. `bodies.rs` fills them in.
    - Leave `TypedConstructor::Implicit` alone. Its params are the fields, whose lines come from `FieldInfo`.
    - Add `line` to `FieldInfo`.
    - Don't change the tuple types or the variants that the interpreter and wasm destructure without `..`: `eval.rs:420,479,656`, `wasm/module.rs:31`, `wasm/function.rs:105,126`. If a variant has to change, add `..` at those sites.
  - **Why:** params, variables and members carry `DW_AT_decl_line`. L22's parameter abbrev has `decl_file` + `decl_line`.
  - *deps: none*

- [ ] **G3 Add a `DebugInfo` side table, built during lowering (under `-g`).**
  - **Today:** the emitter sees only IR. Source names, classes, types and lines are gone by then.
  - **Change:** while lowering, record:
    - per function: the symbol, its class, the bare source name, its kind (user method, explicit or implicit constructor, preamble IO method, `lo_entry`), its return type (for `DW_AT_type`), and its declaration file and line. Implicit constructors use the class's declaration line;
    - per named variable: its vreg, source type, declaration line, and whether it is a param;
    - per class: parent, instance size, its *own* declared fields with type, absolute offset (16 + 8i, from `layout.rs`) and line;
    - the addresses of the prebound `in`/`out`/`err` (their root slots in the static `lo_bindings` frame), for §9.5's `DW_OP_addr` globals.

    Store it in `ProgramIr`, or return it next to it, and pass it to `emit_program`.
  - **Why:** G12 needs all of it. A side table keeps the AST out of codegen.
  - *deps: G1, G2*

## B. Source positions (lowering → IR)

- [ ] **G4 Give statements code that carries their line.**
  - **Today:**
    - `Terminator` has no line.
    - Reading a local or formal (`read_binding`, `lower/operations.rs`), a literal, or `this` emits **no instruction**.
    - `lower.rs` discards the statement lines of `If`/`While`/`Return`/`Break`.

    So `return 1;`, `return x;`, `if (x)`, `while (x)` and `break;` have no code tagged with their line.
  - **Change:** add `terminator_line: Option<u32>` to `BasicBlock`, not to each `Terminator` variant. Only about 11 `BasicBlock` literals exist, versus about 30 `Terminator` constructions in tests. `return 1;`, `if (x)` and `break;` have no instructions, so their terminator is the statement's only code, and it needs a line. Builder sets it as follows:
    - **Statement terminators** get the statement's line: the `Branch` of an `if`/`while` test, a `return`'s `Return`, and a `break`'s `Jump`.
    - **Expression-internal branches** get the expression's line: the `Branch`es created by `choose` (`&&`, `||`, ternary), `null_guard`, `branch_if_equal`, and the division guards.
      - Without this, `while ((done | (i < n)))` has a loop header (`cbr done`) with no position. It inherits the previous statement's line, so line 6 gets no row on iterations where `done` is false. In one case the header even maps to an unrelated `return` line.
    - **`Abort`** gets the line of the call it guards.
    - **Structural jumps get no line** (`None`): the jump into a `while` test, if-joins, `while` back-edges, and empty-`else` jumps. If the jump into a loop carried the `while` line, it would be that line's *first* instruction, and the breakpoint would hit once instead of every iteration (G10's first-instruction rule). A jump with no line just keeps the previous instruction's line, which is harmless, since the debugger never stops on one except under `stepi`.
    - **The implicit `Return(None)`** at the end of a void body takes the body's last statement's line. That line is already "seen", so it's never a stopping point, and `next` from that statement runs through the return into the caller. gas drops line-0 rows, so line 0 isn't an option (G0).

    `dump.rs` must not print `terminator_line` in default mode.
  - *deps: none*

- [ ] **G5 Line rules for lowering (no new IR field).**
  - **Today:** every instruction already carries `line`, the line of the sub-expression that produced it. That line survives `remove_dead_copies` and `gc_roots`, which copies it onto the root stores and loads it inserts.
  - **Decision: no statement numbers.** G10 finds where each line's code starts by taking the first instruction emitted for that line in each function. Blocks are emitted in creation order, and a statement's own block is always created before any block its expression spawns, so the first instruction for a line is where that line's code begins. That holds even when later pieces of the same line land after other code, e.g. division guards created inside a then-branch end up after the else code.
  - **Change (under `-g`):** locals' zero-init in `Builder::new` gets line 0 instead of the method's declaration line. Line 0 emits no `.loc`, so the zero-init stays under the function-entry row, and a one-line method like `int f() { return 1; }` still gets its body stopping point. This is `-g` only, because `--dump-ir` prints lines and the default dump must not change. It's the first change that needs the `-g` flag inside lowering.
  - Code after `return`/`break` is never lowered, so those lines get no code. Write tests that expect this.
  - **TODO, known limitation: statements split across lines.** LO allows them; the lexer treats newlines as spaces. Today each piece keeps its own line:
    ```
    36  r =
    37      w.loop(1, false);
    ```
    The call is tagged 37 and comes first; the store into `r` is tagged 36 and comes last. With the first-instruction rule, `break 37` stops before the call, but `break 36` stops *after* the call, just before the store. It works, just in a surprising order. To fix it, under `-g` tag every instruction of a statement with one line. Decide which:
    - **The first line (36).** `break 36` stops before the statement. `break 37` has no code, so it slides forward to the *next* statement, after this one has run.
    - **The last line (37):** the largest line anywhere in the statement's expression, or in its condition for `if`/`while`. Breaking on any line of the statement stops before it runs, and a stop there reports 37. This matches gcc + gdb: a 2-line C statement checked in `lo-verify` was labeled entirely with its later line.

    Either way, the statement's line is easy to get: the parser records each statement's first-token line (`parser.rs`), and every expression node carries its own line. Check the instructor's goldens for split statements before deciding.
  - *deps: none*

## C. Named variables (IR → allocation → frame)

- [ ] **G6 Under `-g`, keep assignments to named variables.**
  - **Today:** `ir::remove_dead_copies` (run in `build_ir` after `gc_roots`) is the only pass that deletes instructions. It drops every dead `Copy`, including user assignments like `x = 5;` and every zero-init of a local. The statement can end up with no code.
  - **Change:** under `-g`, skip copies whose `dst` has `register_names[..] = Some(_)`.
  - `x = x;` still emits nothing, because the selector drops self-moves. That matches gdb: the value doesn't change.
  - *deps: G1*

- [ ] **G7 Give every named value one fixed home. Named refs in functions with calls use their root slot.**
  - **Today:**
    - Linear scan ignores `register_names`, so named values get registers or spill slots like temporaries.
    - `gc_roots` roots a `Ref` only while it is live across a call, and **nulls every other root slot** at each call.
    - The Lisp-2 collector compacts. It rewrites root slots and heap fields in place, but never registers or spill slots.

    So a named ref the program no longer needs, but the user can still `print`, points at a moved or freed object.
  - **Design.** Linear scan never reuses spill slots (`register_allocator/allocation.rs`), so a pre-assigned spill slot already *is* a fixed home. No new frame region is needed.
    - **Named `Ref` in a function with calls** (`this`, ref params, ref locals): it gets a fixed root slot, numbered *before* temporaries. Its location is a new `PhysicalLocation::Root(slot)`, which `Frame::location` maps to `Frame::root(slot)`.
      - In `gc_roots`, take these out of the save list (`refs_live_across_calls`) and out of the null loop. Emit no `RootStore`/`RootLoad` for them, because the vreg *is* the slot.
      - Number them even when they are never live across a call.
      - Temporaries keep today's behavior.
    - **Every other named value** (int, bool, and refs in leaf functions): it gets a pre-assigned `Spill(k)` before the scan.
      - Leaf functions can't collect garbage. `has_calls` excludes `Abort`, and every allocating helper is an IR `Call`.
    - Skip named vregs in `build_function_live_intervals`, including where it seeds intervals for params, so linear scan never sees them.
    - Apply the same mapping in `spill_all.rs`. Otherwise `-g --spill-all` silently loses the root slots.
    - Carry the mapping in a new `FunctionIr` field (`named_root_slots`), set by `gc_roots` under `-g` and read by both allocators.
  - **Exact slot ranges in `gc_roots`:**
    - named refs take slots `0..N`, and temporaries take `N..N+T`;
    - `assign_root_slots` starts numbering temporaries at N, after removing named refs from each call's save list;
    - the null loop covers only `N..N+T`; today it covers `0..slots.len()`, which would null the named slots;
    - `function.root_slots = N + T`.
  - **Linear scan:** `allocate_function` starts with the named vregs already in `register_locations` and `spill_slot_count = K`. Otherwise `assign_spill_slot` hands out slot 0 again, and `Frame::spill`'s range assert depends on that count.
  - **Liveness:** filter named vregs inside `include_position` in `live_intervals.rs`. That one spot covers all call sites, including the parameter seeding.
  - **Selector:** `select.rs` needs no change. Its only `PhysicalLocation` match goes through `Frame::location`, and a `Root` resolves to a memory operand, so call-argument moves, `move_params` and `parallel_move` work unchanged.
  - **`verify.rs`:** add checks for `named_root_slots`: every entry is a `Ref` in a function with calls, its slot is less than `root_slots`, and no two vregs share a slot. `Frame::root` panics on a bad slot rather than returning an error.
  - **Prologue order fix:** `Frame::prologue` runs `move_params` and *then* nulls every root slot, which would wipe out `this` and the ref params. Under `-g`, null first and store params after. Both must finish before `lo_push_frame`.
  - **What stays the same:**
    - With every param in memory, `move_params` only does its phase-1 stores.
    - Stack params (7th and later) at `[rbp+16+8k]` go through `rax`, which already works.
    - Memory-to-memory copies via `r11`, compares and `idiv` are all already proven by `--spill-all`.
    - 16-byte alignment holds.
  - **The resulting guarantee:** at every statement boundary, each named value's home holds its current value. It is written only by `Copy` or the prologue; call, load and choose results go through temporaries first.
  - **Costs, `-g` only:**
    - a few more root slots for the collector to scan;
    - an object held by a dead local stays alive until return, which the debugger needs anyway.
  - **Tests:**
    - force a collection between an assignment and its `print`;
    - a named ref kept across a collection in a *caller* frame, during recursion;
    - `LO_EMPTY_STRING` in a root slot;
    - `-g --spill-all`;
    - the P3 GC stress suite under `-g` with `LO_GC_POISON=1` and a small `LO_HEAP_SIZE`. If one test runs out of memory, report peak usage before raising the heap; that could hide a real retention bug.
  - *deps: G1, G6*

- [x] **G8 Reference-field stores: no compiler change. Keep the write barrier.**
  `runtime-abi.md` §3.4 requires codegen to send *every* pointer store through `lo_gc_write_barrier`, and the barrier itself does the store. That rule is what keeps the collector choice a runtime-only decision. So `-g` must not inline these stores.
  Consequence: the instruction that writes a reference field is inside `lo_gc_write_barrier` (Rust). The debugger handles it; see "Known `-g` consequences". The only compiler-side requirement is that the runtime keeps its `.eh_frame` (G13), so the debugger can unwind out of the barrier.

## D. Emission (selection → assembly), all under `-g`

- [ ] **G9 Add a typed `.loc` item, and use plain directives for the rest.**
  - `Item::Directive(String)` already exists (`x86.rs`). Only `.loc FILE LINE 0 is_stmt N [prologue_end]` needs a typed variant, because the selector compares positions. The lexer has no columns, so the column is always 0.
  - `.file`, `.cfi_*`, `.type`, `.size` and section switches can be `Directive` strings.
  - `check_instructions` already ignores items that aren't instructions.
  - `Frame::prologue`/`epilogue` return `Vec<Inst>`. Make them return `Vec<Item>` (or positions) so CFI can sit between `push rbp` and `mov rbp, rsp`.
  - *deps: none*

- [ ] **G10 Emit the line table from one place.**
  - **Today:** `select.rs` ignores `Instruction.line`. Blocks are emitted in creation order, and `if_statement` creates the `then`/`else` blocks before lowering the then-body. So blocks created *inside* the then-branch (choose, division, a nested `if`) land after the else code. With rows only at statement starts, that code would inherit the else branch's line.
  - **Change:** make `Selector::emit` the only place that emits `.loc`. It follows these rules (validated by simulating the real compiler's output on seven test programs):
    1. **Track a per-function `seen` set of lines.** The first instruction emitted for a line not yet seen is where the debugger stops for that line: emit `.loc 1 <line> 0 is_stmt 1` and add the line to `seen`.
       - Every later instruction for an already-seen line that needs a `.loc` gets `is_stmt 0`: mapped to that line, but never a stopping point. That covers continuation blocks after a label, expression-internal branches laid out later, and the implicit return.
       - Comparing against just the previous `.loc` is not enough. `choose` blocks laid out after a later statement would get a second stopping point, and the debugger would stop twice.
       - Two statements on one line share one stopping point. That's fine: gdb's `next` doesn't stop twice on one line either, and the transcript only shows `FILE:L`.
    2. **Emit a `.loc` when the line differs from the last one emitted, and at the first instruction after every block label** except block 0's. Write `is_stmt` explicitly every time, because gas's `is_stmt` is sticky.
    3. **Instructions with no line** (structural jumps, G4) emit no `.loc`.
    4. **Line 0** (the `-g` zero-init, G5) emits no `.loc`. It stays under the function-entry row. Otherwise gdb 12 and older stop at the zero-init row.
    5. **A terminator with a line that emits no instruction** (a `break` jump that falls through) still emits `jmp <next label>` under `-g`, so the statement keeps its row. `jmp` is in the Chapter 8 subset; `nop` isn't listed.
    6. **`prologue_end`** goes on the first instruction of the body, the first one with a nonzero line, after the root setup and `lo_push_frame` (L22, §9.3). It gets its own `.loc ... is_stmt 1 prologue_end` even when its line equals the declaration line (one-line methods).
       - If that entry isn't in block 0, for example because the first statement is a `while` and block 0 falls into the loop header, `prologue_end` would land on code that runs every iteration, and `break <fn>` would hit each time. Instead, under `-g`, emit the block-0 fall-through as an explicit `jmp` to the header, carrying `.loc 1 <while line> 0 is_stmt 0 prologue_end`. It runs once.
       - If *no* statement emits code (`void nothing() { ; }`, or an empty explicit constructor), put `prologue_end` on the return code at the declaration line.
  - **File header:** emit `.file 0 "<comp_dir>" "<name>.lo"` and `.file 1 "<name>.lo"`, matching L22. Don't emit a free-standing `.loc` here: two `.loc`s in a row make gas write a spurious row at `.Ltext0`. Use `DW_AT_decl_file 1`. Use the basename, or a path relative to `comp_dir`, never an absolute build path (§9.4.1).
  - **§9.3 requirements this design meets:**
    - a row at each statement boundary, with `is_stmt` set;
    - `prologue_end` after `sub rsp` and the `lo_push_frame` setup;
    - a row before the terminator for terminator-only statements, carried on the block, which is exactly G4;
    - no columns;
    - one sequence for the whole CU, with no per-function `end_sequence`. That's what gas emits.
  - **Function entry:** emit `.loc 1 <decl_line> 0 is_stmt 1` before `push rbp`; otherwise the prologue is charged to the *previous* function's last row. `prologue_end` placement is rule 6 above.
  - **Generated code goes first, with no `.loc` at all.** Under `-g`, emit every generated function before the first LO method: the `main` wrapper, `lo_entry`, implicit constructors, and the preamble IO methods. They get no `.loc`.
    - Today functions come out in class order, so generated ones sit *between* user methods. With gas's one line sequence per section, they would inherit the previous method's last row.
    - Line-0 rows can't fix that, because gas silently drops them (G0).
    - With generated code first, the sequence starts at the first LO method and its single `end_sequence` lands at the end of the last one. Verified in G0: `main` had no rows, and `--verify` was clean.
    - That satisfies §9.3 ("one sequence per CU") and both consumers. The reference debugger looks up rows by exact address. gdb looks up "greatest row ≤ pc", and finds none before the first method.
    - The generated functions still get their §9.4.2 DIEs. They are inside `[.Ltext0, .Letext0)` because they're in the same `.text`.
    - Default mode keeps today's order.
  - **Function bounds:** add a `.Lfunc_end_<sym>:` label after each function, plus `.type sym,@function` and `.size sym, .Lfunc_end_<sym>-sym`. Today only `.globl` is emitted, which makes ELF symbol lookup weak.
  - **CU range:** put `.Ltext0` before the first function and `.Letext0` after the `main` wrapper, before `data.rs` switches sections.
  - **`.debug_line` label:** at the top of the file, emit `.section .debug_line,"",@progbits` / `.Ldebug_line0:` / `.text`, the way gcc does, so `DW_AT_stmt_list` can point at it. gas writes the rest of `.debug_line` and owns `.debug_line_str`.
  - *deps: G0, G1 (source path), G3 (decl lines and the generated-function kind), G4, G5, G9*

- [ ] **G11 Emit exactly §9.6's three-state CFI for every function**, including `lo_entry` and the `main` wrapper. This must be `-g` only, because `.cfi_startproc` makes gas emit `.eh_frame`, which changes default output.
  - **The whole program:** `.cfi_startproc`, then after `push rbp` `.cfi_def_cfa_offset 16` and `.cfi_offset 6, -16`, then after `mov rbp, rsp` `.cfi_def_cfa_register 6`. Use DWARF register numbers (§9.6 table: rbp = 6), as gcc does, not names; names are a needless risk under `.intel_syntax noprefix`. End with `.cfi_endproc`. The CIE comes from gas: CFA = rsp+8, return address in register 16.
  - **No epilogue CFI.** §9.6: "The epilogue needs no extra CFI … sound because LO breakpoints land on statement boundaries, never inside the epilogue." So don't add `remember`/`restore_state` around the mid-function `leave; ret`s, and no `.cfi_offset` for callee-saved registers either. §9.6 calls the subset "exactly that program". The only wrong row is between `leave` and `ret`, which no statement-boundary stop can reach.
  - **An `Abort` at the very end of a function** is a no-return call whose return address is `.Lfunc_end`, which can be inside the *next* function's FDE. The debugger must look up `pc − 1` (required anyway, per L24). Don't add `ud2`: it isn't in the Chapter 8 instruction subset. The debugger's `pc − 1` lookup covers this case.
  - Use `.eh_frame` with `.eh_frame_hdr`, not `.debug_frame` (§9.1).
  - §9.6 also lets the debugger unwind by following the rbp chain. The CFI is still the conformant mechanism, so emit it.
  - *deps: G9*

- [ ] **G12 Add a `.debug_abbrev`/`.debug_info` emitter, exactly per §9.4–§9.5** (new `codegen/dwarf.rs`): a typed DIE tree plus serializer, in deterministic declaration order (no `HashMap` order), with unit tests. Inputs come from G3 plus G7's homes.
  - **Unit header (v5, DWARF32):** `.long` length, version 5, `DW_UT_compile`, address size 8, then `.long .Ldebug_abbrev0` (a relocation, as in L22). Label the start of `.debug_abbrev`.
  - **Encoding details:**
    - `unit_length` = `.long .Lcu_end − .Lcu_start`, with `.Lcu_start` placed *after* the length field.
    - `ref4` offsets count from the first byte of the unit header (the length field): `.long .LdieX − .Ldebug_info0`.
    - Abbreviation codes are unique ULEBs. Each attribute list ends `0,0`, and the table ends with a single `0`.
    - Every DIE with children ends its child list with a null entry. Give childless subprograms a no-children abbreviation.
    - `this` gets its own abbreviation, with `DW_AT_artificial` as `DW_FORM_flag_present`.
    - The `exprloc` length is 1 + the SLEB length; don't hard-code `02`. For example, −48 is `50`, but −72 is `b8 7f`.
    - `data_member_location` uses `udata`, so large classes fit.
  - **Strings:** inline `DW_FORM_string`, with no `.debug_str`. §9.1 allows it, and it's what the reference emitter does. gas owns `.debug_line_str`.
  - **CU (§9.4.1):**
    - `DW_AT_producer`, a string with no timestamp, e.g. `"lo-compiler <version>"`;
    - **`DW_AT_language` = `DW_LANG_C99`**;
    - `DW_AT_name` as a path relative to `DW_AT_comp_dir`, plus `DW_AT_comp_dir`;
    - `low_pc` `.quad .Ltext0`, with `high_pc` as a data4 length `.long .Letext0-.Ltext0`;
    - `stmt_list` `.long .Ldebug_line0`.
  - **Subprograms (§9.4.2):** one per LO method, constructor, **and the synthesized `main` wrapper**, flat under the CU (no method DIEs inside class types). Each has:
    - `DW_AT_name` = the source-level name: `fact`, `main` for Main.main, and `main` for the wrapper too;
    - `low_pc` plus `high_pc` as a data offset (`.Lfunc_end_<sym> − sym`);
    - `DW_AT_frame_base` = `exprloc 01 9c` (`DW_OP_call_frame_cfa`);
    - `DW_AT_decl_file`/`DW_AT_decl_line`;
    - **`DW_AT_type` = the return type, absent for void**;
    - **`DW_AT_external` as an explicit `DW_FORM_flag`: 1 only on the `main` wrapper, 0 on every LO method, constructor and Main.main.** That's how two subprograms named `main` are told apart.
    - Our method symbols are `.globl` today, while §9.4.2 describes them as file-local. Emit the flag per the contract and leave symbol binding alone; changing it would only be needed if a tool depended on it.
    - Class descriptors are already local `.symtab` symbols (`data.rs` emits no `.globl`), so they need no change.
    - **Class-qualified identity** (`break Calc.fact`) comes from the link symbol, which the debugger can demangle (`lo_method_4_Calc_4_fact`), or from the name plus decl line. No `DW_AT_linkage_name` is needed, and §9.4.2 doesn't list one.
    - **`lo_entry`** is our own helper; the reference's wrapper does that work. It gets no DIE, so it reads as non-LO code.
    - **Implicit constructors and preamble IO methods** are LO constructors and methods, so §9.4.2 implies DIEs (decl line = class line, 0 for the preamble). Whether a backtrace shows them as frames, or treats them as "compiler-generated helpers", needs checking against the **[triples]**. Emit the DIEs either way, so the debugger can decide.
  - **Params and variables (§9.4.3):** direct children, in source order, with no lexical blocks.
    - `DW_TAG_formal_parameter` comes first for `this`: `DW_AT_artificial`, typed as a pointer to the enclosing class. Then the declared params.
    - Then a `DW_TAG_variable` per local.
    - Each has `DW_AT_name`, `DW_AT_type`, `DW_AT_decl_line`, and `DW_AT_location` = a single `DW_OP_fbreg (rbp_offset − 16)` (§9.5: a local at `[rbp−16]` is `fbreg −32`). `rbp_offset` comes from `Frame::spill(k)` or `Frame::root(slot)` (G7).
  - **Types (§9.4.4), a fixed table:**
    - `int`: `DW_TAG_base_type`, `DW_ATE_signed`, byte_size 4, name `"int"`.
    - **`bool`: `DW_ATE_boolean`, byte_size 1**, name `"bool"`. The consumer reads 1 byte. Our code stores bool as a 32-bit 0/1, so the low byte is the value. Watch length for bool is 1.
    - void: no DIE.
    - Each class `C`: a `DW_TAG_pointer_type` (byte_size 8) pointing to a `DW_TAG_class_type`. The class type has name `"C"` and byte_size = `ClassLayout.instance_size`; don't compute `16+8n`, because `Output` has a hidden slot. It has one `DW_TAG_member` per field *the class itself declares*, located at that field's `FieldSlot.offset` from `layout.rs`. That's the absolute offset from the object start, so the first field is at 16. A subclass gets one `DW_TAG_inheritance` (parent type, member_location 0). The 16-byte header is **not** described.
    - `String`: a pointer type pointing to a `DW_TAG_class_type` named `"String"`, nominal byte_size 16, no members. The debugger recognizes it by name.
  - **Globals (§9.5):** a CU-level `DW_TAG_variable` for each of `in`/`out`/`err`, with `DW_OP_addr` = its root-slot address in `lo_bindings`. Use a `.quad` relocation.
  - **Excluded (§9.8):** no location lists, lexical blocks, vtable attributes, ranges, `.debug_names`/pubnames, or DWARF64.
  - **Consistency:** every subprogram range must fall inside `[.Ltext0, .Letext0)`, or `--verify` fails.
  - *deps: G2, G3, G7, G10*

## E. Runtime and validation

- [ ] ⏸ **G13 Strip debug info from the runtime archive.** §9.4 assumes exactly one CU per program; this makes that literally true.
  - **Today:** in the x86 `liblo_runtime.a`, 312 of 329 members carry `.debug_info`/`.debug_line` (rustc std), and 286 carry `.eh_frame`.
  - **Why it matters:**
    - `llvm-dwarfdump --verify` on the *linked* binary checks std's CUs too, which we don't control.
    - gdb, the grading oracle, behaves differently in runtime frames.
    - Our debugger has to skip many CUs that aren't ours or aren't v5.
  - **Change:** run `objcopy --strip-debug` on the archive and **keep `.eh_frame`**. Do it inside `lo-build` in our grading-contract script, after the second cargo build, since that's the grader's only build step. A host run with `llvm-objcopy` confirmed the approach: 312 debug members down to 0, `.eh_frame` unchanged, and the linked binary drops from 13 CUs to 1. Confirmed with GNU `objcopy` in `lo-verify` (G0). This doesn't touch code, so P3 behavior is unchanged. Re-run the P3 suite to confirm.
  - Also verify `program.o` on its own.
  - *deps: none*

- [ ] **G14 Validate the debug info.** Over the corpus, under `-g`:
  - `llvm-dwarfdump --verify` on `program.o` and **especially on the linked binary**, since range containment is only checked after linking, using both our `gcc -static` link and the grader's `as --64` + `cc -no-pie`;
  - `readelf --debug-dump=info,line,frames` parses the output;
  - a gdb smoke test **on the course x86 host** (gdb isn't in `lo-verify`), on `test_18` and on a recursive factorial: break by line and by function, `run`, `bt`, `print`, `next`, `step`.

  Wire all of this into CI or the grading-contract script.
  - *deps: G10–G13*

- [ ] **G15 Write golden line-table tests.** For a few programs, assert the `.loc`/`is_stmt`/`prologue_end` sequence and the decoded line table. Seed programs should cover: short-circuit `while` headers, division in a loop, `break` falling through, a first-statement `while`, an empty method, and a multi-line statement. Check:
  - exactly one stopping point per line in each function, reached once per iteration for a `while` test;
  - no row for `else`, braces, join jumps or back-edges;
  - a `return 1;` row;
  - an `if (x)` row;
  - no rows at all for generated code, which is emitted before the first LO method.
  - *deps: G10*

- [ ] **G16 Run the regression gate for `-g` itself.** Run the P1–P3 suites under `-g` (with linear scan and with `--spill-all`). Output and exit codes must match the default build.
  - **One exception:** a GC stress test can legitimately run out of memory under `-g` at a small `LO_HEAP_SIZE`, because dead locals keep objects alive (G7). Triage it with G7's rule: report the peak, and confirm the extra retention comes only from named locals, before accepting it.
  - *deps: all of C and D*

- [ ] **G17 Run the default-mode byte-identical gate** from the Ground rules.
  - *deps: all*

---

## Known `-g` consequences for the debugger (not compiler changes)

- **Watchpoint traps outside LO code: sort them by which runtime function the pc is in (G8).** The ABI defines a pointer-field store as "call `lo_gc_write_barrier`", so a trap inside the barrier *is* the LO assignment's write. A watch on a reference or String field `obj.f` therefore traps with the pc in the barrier. Int and bool field stores, and stores of `LO_EMPTY_STRING`, are inline and trap in LO code. Classify the trap by the `.symtab` symbol containing the pc. The barrier's name is fixed by the ABI, so this is stable:
  - **pc in `lo_gc_write_barrier`:** a program write. Report it, with the write site taken from the caller frame (`return_pc − 1` → the assignment's `file:line`). This reuses the unwinder and the "is this LO code" check that backtrace and `step` already need. The runtime's `.eh_frame` is kept (G13) because Rust release builds omit frame pointers.
  - **pc anywhere else in the runtime** (the collector relocating objects, `fix_references`): not a program write. Refresh the cached value and continue. gdb, the oracle, *does* report these, so this is a deliberate divergence; confirm it against the **[triples]**.
  - **When the write site is generated code:** the implicit constructor stores ref fields through the barrier, and generated code has no line rows. Report the first frame that *has* a line instead.
  - **Field watches must follow a moving object.** Compaction (`slide`) moves the watched object, which leaves the debug register on the old address. No trap fires there, so trap classification alone can't notice. Do what gdb does for `watch c.radius`: also watch the *base variable's* home slot.
    - The collector rewrites that root slot when it moves the object, which traps.
    - On that trap, re-resolve `c + offset` and re-arm the field's register.
    - A field watch therefore costs two of the four debug-register slots.
    - A base in a leaf function's plain slot can't move during the leaf.
    - No runtime hook is needed.
  - Check the reported line against the **[triples]**. The instructor's compiler is bound by the same ABI, so the goldens must already take a position on this.

- **Two functions are named `main`.** Both the wrapper and Main.main have `DW_AT_name` `main`. §9.4.2 tells them apart by `DW_AT_external`: 1 on the wrapper. The debugger:
  - resolves `break <function>` by DWARF name over the subprograms with `external = 0`, or by demangling the link symbol for `Class.method`;
  - drops the wrapper (`external = 1`) and anything with no DIE (`lo_entry`, the runtime, libc) from backtraces.
- **The collector rewrites named-ref root slots.** It writes every root slot on every collection, and the value changes when an object moves. A watch on a ref local can therefore trap inside the collector. That falls under the "anywhere else in the runtime" case above: refresh the cached value and continue.
- **Watch scope.** Watches must be deleted when their frame returns, because a later callee's prologue reuses the slot. Graded scripts guarantee this.
- **Only read the LO program's own debug info.** A binary can contain other debug info, e.g. the Rust standard library's in the runtime if G13/S2 is skipped. Use only the unit our compiler wrote: it's the one covering the LO methods' addresses, and its `stmt_list` names our line program. Skip every other unit by its length field.
- **Prologue writes and default init happen before `prologue_end`.** That's fine, since watches are only set at a stop after it.
- **Line-table consumption rules for the debugger** (from the round-2 simulation):
  - When `step` enters a callee, skip rows before `prologue_end`. L23's "activation changed" rule would otherwise stop at the callee's entry row, before its frame is set up.
  - Treat a pc with no covering row, i.e. generated code before the first LO method, as "no line", and step out of it.
  - Look up `is_stmt 1` rows only when resolving `break FILE:L`, and dedupe them per function. A one-line constructor such as `Base() { count = 0; tag = "b"; }` correctly has two statement rows on the same line, and a breakpoint there should resolve to the first.

## Implementation order: 24 small steps

The G-items above are the *design*. This section is the *build order*: each step is one small PR that can land on its own.

**Every step must keep the default build byte-identical.** Run S1's gate (`--emit-asm`, `--emit-asm --spill-all` and `--dump-ir` over the corpus, plus `cargo test`) before merging, every time. From S14 on, also run the P1–P3 suites under `-g`.

### Dependency graph

```
Lane A (safety)    S1 ──────────────────────────────────────────────────────────┐ (gate for every step)
                   S2 ─────────────────────────────────────────────────┐        │
Lane B (front end) S5 ─┐                                               │        │
                   S6 ─┼─→ S9 ───────────────┬──────────┬───────┐      │        │
                   S3 ─┘        ┌────────────┘          │       │      │        │
                   S8 ──────────┼──────────────→ S18 ──→ S19     │      │        │
Lane C (homes)     S3 → S10                  ↑          │       │      │        │
                   S11 → S12 → S13 → S14 ────┼──────────┼──→ S22 │      │        │
Lane D (emission)  S15 → S16 ────────────────┼──────────┼───────┼──→ S23 → S24 ←┘
                   S3,S9 → S17 ──────────────┘          │       │      ↑
                   S3 → S4 ──────────────────────────────┘       │      │
                   S9,S17 → S20 → S21 → S22 ─────────────────────┘──────┘
```

**Start immediately, in parallel:** S5, S6, S8 (bringing S3's flag), S11, S15. None of them depends on anything else. S1 is done (kept as a local tool).

### Steps

| # | Step | G-item | Main files | Depends on | Done when |
|---|---|---|---|---|---|
| **S1** ✅ | **Regression gate script** (done; kept as a local tool, not committed). It builds the baseline compiler from a git ref and the working tree, diffs `--emit-asm`, `--emit-asm --spill-all` and `--dump-ir` for every corpus program, and runs `cargo test`. | G17 | outside the repo | — | Passed on unchanged `main` (678 comparisons, 183 tests); a planted one-character selector change was caught (176 diffs) |
| **S2** ⏸ | **Optional, decide later: strip the runtime archive's debug info.** Not needed for correctness: our debugger reads only the LO program's own debug info, and `--verify` passed with the Rust std debug info present. Stripping (`objcopy --strip-debug`, keeping `.eh_frame`) only makes gdb comparisons cleaner and binaries smaller. Its home is `lo-build` in the P4 grading contract, which isn't published yet. | G13 | P4 grading contract | — | Decide when the P4 contract exists |
| **S3** | **Add the `-g` flag and a `DebugMode`.** *Lands together with its first consumer (S8), not on its own, so no unused parameter is ever threaded through.* Parse it, then thread the mode (unused) through `build_ir → lower_program → insert_gc_roots → remove_dead_copies → allocate → emit_program → select_function → Frame::new`. | G1 | `codegen/driver.rs`, signatures | — | `-g` is accepted and its output equals default output |
| **S4** | **`-g` toolchain flags and source path.** *Lands with S16/S18, when there is CFI and a line table for them to matter to.* Pass `as --gdwarf-5` and `-Wl,--eh-frame-hdr` under `-g`, and pass the source path and comp_dir down to `emit_program`. | G1 | `codegen/driver.rs`, `codegen/mod.rs` | S3 | A `-g` binary has `.eh_frame_hdr` |
| **S5** | **Record field declaration lines** in `FieldInfo`, from `own_fields`. | G2 | `type_checker/class_table.rs`, `declarations.rs` | — | Unit test reads a field's line |
| **S6** | **Record param and local declaration lines.** Add `formal_lines`/`local_lines` on the `TypedMethodDecl` struct and on `TypedConstructor::Explicit`. Don't change the interpreter or wasm. | G2 | `type_checker/typed_ast.rs`, `bodies.rs` | — | Interpreter and wasm unchanged; unit test |
| ~~S7~~ | **Dropped.** Statement numbers aren't needed; G10 uses the first instruction for each line (see G5). Multi-line statements are a TODO in G5. | — | — | — | — |
| **S8** | **Put lines on terminators**, plus the `-g` zero-init line 0. Add `terminator_line` on `BasicBlock`, set for statement terminators, expression-internal branches (`choose`, `null_guard`, `branch_if_equal`, division guards) and `Abort`; `None` for structural jumps. The implicit `Return(None)` takes the last statement's line. **The `-g` flag (S3) lands in this step**, as its first consumer. | G4, G5 | `ir/mod.rs`, `ir/lower.rs`, `ir/lower/operations.rs`, `codegen/driver.rs` | — | IR test: `return 1;`, `if (x)`, `while (a \| b)` and `break` have lines, and structural jumps don't; gate passes |
| **S9** | **Build the `DebugInfo` side table** during lowering, under `-g`. Per function: kind, bare name, return type, declaration lines. Per variable: vreg, type, line, whether it's a param. Per class: parent, `instance_size`, its own fields with offsets. Plus the `in`/`out`/`err` addresses. | G3 | `ir/lower/program.rs`, new `ir/debug_info.rs` | S3, S5, S6 | Unit test on a small recursive program and a class hierarchy |
| **S10** | **Keep named copies under `-g`:** `remove_dead_copies` skips copies whose `dst` has a name. | G6 | `ir/liveness.rs` | S3 | `-g` IR keeps `x = 5;` and the zero-inits; default unchanged |
| **S11** | **Add a `PhysicalLocation::Root(slot)` variant.** `Frame::location` maps it to `Frame::root(slot)`. Nothing produces it yet. | G7 | `register_allocator/mod.rs`, `codegen/frame.rs` | — | Compiles; gate passes |
| **S12** | **`gc_roots` under `-g`:** add the `named_root_slots` field. Named refs get slots `0..N` and temporaries `N..N+T`. Nulling covers only the temporaries, and named refs get no `RootStore`/`RootLoad`. Only functions with calls. Add the `verify.rs` checks. | G7 | `ir/gc_roots.rs`, `ir/mod.rs`, `ir/verify.rs` | S3, S11 | `-g` IR dump shows the slot layout; verify rejects bad entries |
| **S13** | **Pin named values in both allocators.** Linear scan: pre-assign `Spill(k)`/`Root(slot)`, start with `spill_slot_count = K`, and filter named vregs in `include_position`. `spill_all` gets the same mapping. | G7 | `register_allocator/{allocation,live_intervals,spill_all}.rs` | S12 | Allocation test: named vregs never get registers; temporaries unchanged |
| **S14** | **Reorder the prologue under `-g`:** null the root slots first, then `move_params`, all before `lo_push_frame`. **After this step a `-g` binary is semantically complete.** | G7 | `codegen/frame.rs` | S13 | P1–P3 suites pass under `-g`, with linear scan and `--spill-all`, and with `LO_GC_POISON=1` plus a small heap (G16 triage rule) |
| **S15** | **Add a typed `Item::Loc` and make prologue/epilogue return `Item`s.** The other directives stay `Item::Directive`. | G9 | `codegen/x86.rs`, `print.rs`, `frame.rs` | — | Gate passes |
| **S16** | **Three-state CFI for every function** under `-g`, using register numbers. No epilogue CFI. | G11 | `codegen/frame.rs`, `codegen/mod.rs` (wrapper) | S3, S15 | `readelf --debug-dump=frames` shows three rows per FDE |
| **S17** | **Function bounds and layout under `-g`:** `.type`, `.size`, `.Lfunc_end_<sym>` and `.Ltext0`/`.Letext0`, and **emit generated functions first** (`main` wrapper, `lo_entry`, implicit constructors, IO methods). | G10 | `codegen/mod.rs`, `select.rs` | S3, S9 | `nm -n` shows generated code first; default order unchanged |
| **S18** | **Emit the line table:** `.file 0`/`.file 1`, the `.Ldebug_line0` label, the function-entry row, and `Selector::emit`'s `.loc` rules 1–6 (the `seen` set, explicit `is_stmt`, the fall-through `break` `jmp`, `prologue_end` placement). | G10 | `codegen/select.rs`, `codegen/mod.rs` | S8, S9, S15, S17 | `llvm-dwarfdump --debug-line` shows the expected rows |
| **S19** | **Golden line-table tests,** covering the G15 cases: one `is_stmt 1` row per statement, one per `while` iteration, none for generated code. | G15 | `compiler/tests/` | S18 | Tests pass |
| **S20** | **DWARF emitter skeleton:** a typed DIE tree and serializer, the abbrev table, the unit header with label-difference length, the CU DIE (`DW_LANG_C99`, name, comp_dir, producer, `low_pc`/`high_pc`, `stmt_list`), and the `int`/`bool` base types. | G12 | new `codegen/dwarf.rs` | S9, S17 | `--verify` passes on a program with an empty CU; serializer unit tests |
| **S21** | **Type DIEs:** pointer types, class types with `instance_size` and their own members at `FieldSlot.offset`, `DW_TAG_inheritance`, and the name-only `String` class. | G12 | `codegen/dwarf.rs` | S20 | `llvm-dwarfdump` shows the type table for a class hierarchy with inherited fields |
| **S22** | **Subprogram DIEs:** return type, `external` (1 only on the wrapper), frame base `9c`, `this` (artificial, abbrev with `flag_present`), then params and variables with `fbreg(rbp_offset − 16)` taken from the homes. Plus the `DW_OP_addr` globals. | G12 | `codegen/dwarf.rs`, `frame.rs` (offset query) | S13, S21 | `fact`'s `n` is `fbreg −48`, as in L22 |
| **S23** | **Validation script:** compile the corpus with `-g`, link with both `gcc -static` and `cc -no-pie`, run `llvm-dwarfdump --verify` on the **linked** binaries plus `readelf`. Wire it into CI or the contract. | G14 | `scripts/` | S2, S16, S18, S22 | Clean over the whole corpus |
| **S24** | **Final gates:** S1's default byte-identical gate, the P1–P3 suites under `-g` (G16), and a **gdb smoke test on the course x86 host**: break by line and by function, `run`, `bt` through recursion, `print`, `next`, `step`. | G14, G16, G17 | — | All pass |

### Why this order

- **S1 comes first** because it's the safety net for everything else.
- **S3 makes every later `-g`-only change a one-line branch.** It's the only step most others need.
- **The data-only steps (S5–S9, S11, S15) change no output.** They can land early and in any order.
- **The variable-homes lane (S10–S14) is the riskiest code change.** It touches GC and the allocator, so it ends with a full `-g` suite run (S14) before any DWARF depends on it.
- **The line table (S18) needs terminator lines (S8), declaration lines (S9) and layout (S17).** The DWARF emitter needs the homes (S13) for its locations.
- **The debugger team can start against S16 + S18 output** (CFI and line table) before S20–S22 exist. That's enough for breakpoints, stepping and an rbp/CFI backtrace. Locals and `print` need S22.
