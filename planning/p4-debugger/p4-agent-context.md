# P4 Debugger: Agent Context (Group 6, CS 378H, Fall 2026)

Reference for a coding agent working on Project 4 (the `-g` compiler mode and the ptrace debugger) in this repo. The human-facing summary is `p4-brief-for-neha.pdf` in this folder; this file is the exhaustive version. Draft of Thu 11/26/2026. Team: Neha (P4 design lead), Jay, Rahik, Sarthak.

## How to use this file

- **Status of content.** Three things are *decided* by the team (11/26) and must not be reopened: reflection gating (combined plan Decision 1), branch strategy (Decision 2), terminator lines (Decision 4). Everything labelled D1–D10, and combined-plan Decisions 5, 15, 16, 17, is a *draft recommendation* until Neha confirms it, **except** where a point is marked `[V §9.x]`: those are fixed by the course's DWARF contract and are not team choices. Ask Neha (or check for a newer decision record) before building on a draft choice that is expensive to reverse.
- **The contracts are in the LO language reference.** The "DWARF Subset Reference" the P4 spec names (`dwarf-subset-reference.md`, "DSR") is **Chapter 9** of `external-docs/LO-language-reference.pdf` (book pp. 65–74, §9.1–§9.9). The "LO Architecture Reference" / `architecture-reference.md` is **Chapter 8**, "Native x86-64 Back End" (pp. 55–63). `runtime-abi.md` is Chapter 6 and `error-codes.md` is Chapter 5. The chapters keep their stand-alone section numbers in cross-references, so map them as in the table in §1. Ch. 9 overrides any draft choice in this file; where ch. 9 is silent, this file says so.
- **Compiler-side `-g` is planned elsewhere, and that plan wins.** `planning/p4-g-mode.md` on branch `origin/docs/p4-g-plan` ("the g-mode plan"; read with `git show origin/docs/p4-g-plan:planning/p4-g-mode.md`) is the verified compiler plan: items G0–G17, build steps S1–S24, reconciled against ch. 9 and toolchain-tested in `lo-verify` (G0: GNU as 2.40, gcc 12.2). Where it differs from a compiler-side draft below, **follow the g-mode plan**; this file now only summarizes and points to it (cited as "g-mode G*n*"/"S*n*"). This file stays authoritative for the debugger side and for the P4/LO-5 coordination.
- **Still missing**: the published conformance triples, the P4 grading contract, and course-host access (§1).
- **Markers.** `[V]` = checked against the cited code or document. `[V §9.x]` = fixed by LO reference ch. 9, section 9.x (likewise `§8.x`, `§6.x`, `§5.x` for chs. 8, 6, 5). `[I]` = inferred, proposed, or recalled from memory (especially gdb and GNU `as` behaviour). **Never treat an `[I]` item as verified**: confirm it (run the tool, read the code, calibrate against gdb on the course host) before relying on it in code or tests, and say so in your notes when you have.
- **Citations.** Paths are relative to the repo root; `compiler/src/` is abbreviated where unambiguous (e.g. `codegen/frame.rs` means `compiler/src/codegen/frame.rs`). Line numbers were spot-checked on 11/26 and drift; re-grep before editing. "Brand ch. N" is Sy Brand, *Building a Debugger* (`external-docs/building-a-debugger-sy-brand.pdf`). "spec p. N" is `external-docs/p4-debugger.pdf`. "ref §N.x" is `external-docs/LO-language-reference.pdf`. "agent1" = draft P4 plan, "agent3" = P4/LO-5 overlap report, "combined plan" = `planning/p4-lo5-plan/p4-lo5-plan-overview.tex`.
- **Repo rules.** Follow `CLAUDE.md`: no pushes, no commits or branches unless the user asks, never change the runtime ABI without sign-off. Respect the hotspot-file branch rules below.

---

## 1. Status and contracts

- **Due Mon 11/30, 11:59pm Central**, tagged `p4-final` (spec p. 4–6) [V]. Thanksgiving break 11/23–11/27; LO-5 talk block 1 is in class the same Monday (spec p. 5–6) [V].
- **Nothing is implemented** as of 11/26: no `-g` flag, no `.loc`/`.cfi`/`.debug_*` emission, no debugger crate, no P4 branch (agent1, from a grep of `compiler/src` and `git log --all`) [V].

### Contracts we have (all in `external-docs/LO-language-reference.pdf`) [V]

| Name used by the spec / repo | Where it is | Section mapping |
|---|---|---|
| DWARF Subset Reference (`dwarf-subset-reference.md`, DSR) | **Ch. 9**, pp. 65–74 | Its internal "§N" = ref §9.N (e.g. "§4.4" = §9.4.4, "§7" = §9.7). |
| LO Architecture Reference (`architecture-reference.md`; ch. 7 also calls it "the Target Architecture Reference") | **Ch. 8**, "Native x86-64 Back End", pp. 55–63 | "the Native x86-64 Back End chapter §N.x" = ref §8.N.x. Confirmed by every cross-reference ch. 9 makes: §2.6 int3 = §8.2.6 "Breakpoint"; §2.4 SETcc = §8.2.4 "Comparison and branches"; §1.4 no SSE/x87 = §8.1.4 "What's excluded"; §3 RIP-relative = §8.3 "Addressing modes"; §4–§5 prologue/frame = §8.4 "Calling convention", §8.5 "Stack discipline". |
| Runtime ABI (`runtime-abi.md`) | **Ch. 6**, pp. 37–46 | "the Runtime ABI chapter §N.x" = ref §6.N.x (§2 object header = §6.2, §2.1 ClassDescriptor = §6.2.1, §2.2 String = §6.2.2, §3.3 shadow stack = §6.3.3, §3.8 aborts = §6.3.8). |
| Error-code vocabulary (`lo-testing/error-codes.md`) | **Ch. 5**, pp. 31–35 | Runtime-abort exit codes in §5.2; how to add a code in §5.3. |

**Caution on "architecture-reference.md §2.6 (debug registers)".** The P4 spec labels §2.6 "debug registers" (spec p. 6), but §8.2.6 is the **`int3` software breakpoint** ("plants int3 … catches the resulting SIGTRAP via ptrace, restores the original byte … and steps", Brand ch. 7 and 10). The only debug-register text in ch. 8 is §8.1.4: dr0–dr7 are excluded from emitted code; "the debugger project uses dr0–dr3 via the ptrace API, not by direct instruction". Ch. 9 cites §2.6 for exactly the int3 meaning (§9.7). So **no course document pins the dr7 encoding or slot management**; that comes from Brand ch. 9–10 and the Intel SDM (§6.4).

### Still missing [V: not in the reference PDF, the P4 spec, or the repo]

| Document | What depends on it | Priority |
|---|---|---|
| Published conformance triples | Exact whitespace (e.g. `#0   main`), `break *` address forms, rendering edge cases, gdb's watchpoint/stepping behaviour on our code. "Published with the assignment" (spec p. 3), so possibly on the course site. `test_18.lo` (spec example) is not in the repo either. | First |
| P4 grading contract (cf. `p3-grading-contract.sh`) | Debugger command line, where the transcript goes, debuggee stdin/stdout, who builds the `-g` binary and with which `as`/link flags. Ch. 9 adds one fact: the reference `-g` object is assembled with `as --gdwarf-5` (§9.1). | First |
| Course x86-64 host access | All live-process work (spec p. 3). Ask whether crates such as `nix` can be fetched there; `nix` is **not** in the local cargo cache (`gimli` and `object` are) [V]. | Second |

**Can start without them** (the spec's own week-1 items, spec p. 5): debugger engine over a mock process; script parser and transcript printer; ELF/`.symtab` parser; the `-g` frame work (spill-all, home slots); `.loc`/CFI emission; and now all of `codegen/dwarf.rs` and the DWARF parsers, which ch. 9 fully specifies.

---

## 2. Requirements checklist (all [V] from the spec unless marked)

Grading: correctness 50%, code review 20%, viva voce 20%, contributed tests 10%; peer factor applied (spec p. 6). Correctness = instructor's held-out superset of the published triples, run in batch mode on the course host, diffed against golden output, plus `-g` DWARF validity.

### 2.1 Batch transcripts (correctness, 50%)

- Reads a gdb-syntax command script; output must match golden **byte for byte** (spec p. 2–3).
- Only these commands appear (parse only these forms): `run`, `continue`; `break <function>`, `break <file>:<line>`, `break *<address>`, `delete <n>`; `stepi`, `step`, `next`; `backtrace`, `info locals`, `print <expr>`, `x/1dw <expr>`; `set var <lvalue> = <value>`; `watch <expr>` (spec p. 3).
- `<expr>`: a local or parameter name, `obj.field`, a register such as `$rax`, or an address form such as `*(int*)((char*)c + 16)`. Inside a method a field is `this.radius`, never bare `radius`.
- Breakpoints and watchpoints share **one numbering from 1**; `delete` applies to both.
- Script guarantees: `delete` always has an argument; a watchpoint is deleted before its frame returns; `continue`, not stepping, is used past the return from `Main.main`.

**Transcript grammar** (spec p. 3), verbatim:

```
set-break ::= Breakpoint N at FILE:L (FN)
set-watch ::= Hardware watchpoint N: EXPR
bp-stop   ::= Hit breakpoint N: FILE:L
wp-stop   ::= Hit watchpoint N: EXPR at FILE:L NEWLINE
              Old value = V NEWLINE
              New value = V
step-stop ::= Stopped: FILE:L
bt-line   ::= #K   FN (ARGS) at FILE:L
value     ::= EXPR = V
locals    ::= NAME = V ...   |   No locals.
exit      ::= Debuggee exited with code D
```

Rules:

- `step`, `next`, `stepi` print `step-stop`. `print` and `x/1dw` print `value` with EXPR **as written in the script**.
- `run`, `continue`, `delete`, `set var` print nothing of their own; a resume prints only the stop or exit it reaches.
- FILE = source file base name. FN = subprogram's `DW_AT_name` (`Main.main` prints as `main` in the example). D = exit code in decimal.
- ARGS = comma-separated `name=V`, **omitting `this`**, non-null references shown as `<heap>`.
- Backtraces list only LO methods and constructors; compiler-generated helpers, the runtime and the C entry point are omitted; K counts from 0 over the remaining frames.
- Values V: int and registers as signed decimal; bool as `true`/`false`; String as contents in double quotes; `null`; an object as `Class@<heap> (f = V, ...)` using its **dynamic** class; a reference held in a field as `Class@<heap>` **without** its fields.
- The debuggee's own output is not part of the transcript.
- Determinism: launch with `personality(ADDR_NO_RANDOMIZE)`; no raw addresses anywhere in transcripts (stack/heap vary even with ASLR off; code addresses vary between teams) (spec p. 2).
- Exit codes include runtime aborts: 101, 102, 110–112, 120, 137 [V] (`runtime-abi.md:243-251`; ref §5.2 table). LO-5 adds 140 (reflection), which is not yet registered (§7.8).
- Whitespace in `bt-line` is three spaces after `#K` in the spec example; confirm against the published triples [I].

**Spec example** (p. 2–3). Script:

```
break test_18.lo:19
run
backtrace
print c
next
print c
continue
```

Golden transcript:

```
Breakpoint 1 at test_18.lo:19 (main)
Hit breakpoint 1: test_18.lo:19
#0   main () at test_18.lo:19
c = null
Stopped: test_18.lo:20
c = circle@<heap> (radius = 4)
Debuggee exited with code 48
```

`test_18.lo` is not in this repo [V], so its class naming (`circle` lowercase) cannot be checked.

### 2.2 The `-g` compiler mode (correctness: "DWARF validity")

- `rbp` is a true frame pointer in every function, never omitted or repurposed; every named parameter and local has a fixed home slot, one `DW_OP_fbreg` each. The default build is unchanged; both modes coexist (spec p. 1).
- DWARF **version 5**: `.debug_line`; the `.debug_info` subset (compilation unit, subprograms, formal parameters, locals, the LO type DIE table); the CFI the DSR requires. The full subset is in §2.2.1 below.
- `llvm-dwarfdump --verify` clean over the conformance corpus; gdb and `readelf --debug-dump` should understand the output.
- Ch. 9 frames the same thing as two reference-compiler build modes: the default optimized build (linear scan, no DWARF; what P2 was graded on) and a `-g` build ("frame pointer guaranteed, named locals pinned to their home stack slots, DWARF sections emitted"), the `-O0 -g` posture [V §9.1].

### 2.2.1 The DWARF contract, ref ch. 9 (checklist; all [V §9.x] unless marked)

The spec: "a closed contract — what it names, you emit; what it excludes, you must not need" (spec p. 1). Ch. 9 is "the spec their P2 compiler's debug-info emission targets … and the spec their P4 debugger parses"; the reference compiler and reference debugger are held to it (§9.9). It applies to native x86-64 only, not WASM (ref ch. 7 intro).

**Sections emitted, and no others (§9.1).** `.debug_abbrev`; `.debug_info`; `.debug_line`; `.debug_str` / `.debug_line_str` as needed; `.eh_frame` with `.eh_frame_hdr`. The producer **may** write `.debug_info` strings inline as `DW_FORM_string` (`.asciz`) and emit no `.debug_str`; **the consumer must accept both**. The reference emitter inlines `.debug_info` strings and lets `as` produce `.debug_line_str` for the line program's file/dir table. The `-g` object is assembled with **`as --gdwarf-5`**, so the assembler-built `.debug_line`/`.eh_frame` are DWARF 5, matching the hand-emitted `.debug_info`/`.debug_abbrev`. The default build emits no debug directives. `.debug_frame` is **not** emitted.

**Code shape the DWARF describes (§9.2, ref §8.4–§8.5).** Every function: `push rbp; mov rbp, rsp; sub rsp, <frame_size>` … `leave` (or `mov rsp, rbp; pop rbp`) `; ret`. After the prologue: saved rbp at `[rbp]`, return address at `[rbp+8]`, stack args 7+ at `[rbp+16]`, `[rbp+24]`, …; named locals and home slots of register params at `[rbp-k]`. The shadow-stack frame is one more local whose structure the DWARF does not describe. **CFA = rbp + 16 everywhere in the body.**

**`.debug_line` (§9.3).**
- Line granularity, **no columns**: `DW_LNS_set_column` is not emitted; the debugger ignores any column.
- **`prologue_end` is emitted** on the first instruction after the prologue, "after `sub rsp, …` and the `lo_push_frame` setup"; the reference debugger honors it for `break Foo.bar` and `break foo.lo:N` on a function's opening line.
- **`is_stmt`** is set on the recommended-breakpoint rows (statement boundaries).
- **Every statement that emits code gets a row, including terminator-only statements** (`return <local>;`, `if (<bool-var>)`): the row goes immediately before the terminator. "The reference emitter carries the line on the block for this case" — exactly our Decision 4 (`BasicBlock.terminator_line`).
- **One line-program sequence per CU**, one `end_sequence`; `prologue_end` is the only per-function delimiter. The consumer finds a stop's line by the row whose address **exactly matches** (row-at-boundary), *not* "greatest row ≤ pc", which would alias across the gap between functions. The debugger never plants a breakpoint at the `end_sequence` address.
- Opcodes that appear: special opcodes, `DW_LNS_copy`, `advance_pc`, `advance_line`, `set_file`, `negate_stmt`, `set_prologue_end`, `DW_LNE_set_address`, `DW_LNE_end_sequence`. No vendor opcodes, no `DW_LNE_define_file`.
- File/dir tables in the DWARF 5 header, paths via `.debug_line_str`; one file entry per `.lo` source.

**`.debug_info` (§9.4): one CU per program; closed DIE vocabulary.**
- `DW_TAG_compile_unit`: `DW_AT_producer` (e.g. `loc <version>`); `DW_AT_language = DW_LANG_C99`; `DW_AT_name` (primary source, **relative to** `DW_AT_comp_dir`, not an absolute build path); `DW_AT_comp_dir`; `DW_AT_low_pc`/`DW_AT_high_pc`; `DW_AT_stmt_list`.
- `DW_TAG_subprogram`, a **flat child of the CU**, for each LO method, constructor, **and the synthesized `main` wrapper**: `DW_AT_name` (source-level name, e.g. `bar`; no class qualifier); `DW_AT_low_pc`/`DW_AT_high_pc` (high as an offset, `DW_FORM_data*`); `DW_AT_frame_base = DW_OP_call_frame_cfa`; `DW_AT_decl_file`/`DW_AT_decl_line`; `DW_AT_type` = return type, **absent for void**; `DW_AT_external` as an explicit `DW_FORM_flag`, **1 only on the C `main` wrapper**, 0 on every LO method, constructor and the user's `Main.main`. So a program has two subprograms named `main`, told apart by `DW_AT_external` and their link symbols. Class-qualified identity lives only in the link symbol; `break Foo.bar` resolves "through the source name plus `DW_AT_decl_file`/`DW_AT_decl_line` (or directly via the link symbol)". **Not in the subprogram's attribute list**: `DW_AT_linkage_name`, `DW_AT_artificial`, `DW_AT_virtuality`, `DW_AT_vtable_elem_location`.
- Parameters and locals: **direct children of the subprogram, in source order**, no `DW_TAG_lexical_block`. `DW_TAG_formal_parameter` for each parameter; the implicit receiver comes **first**, named `this`, typed pointer-to-enclosing-class, with `DW_AT_artificial`. `DW_TAG_variable` for each named local (block-locals are hoisted). Each carries `DW_AT_name`, `DW_AT_type`, **`DW_AT_decl_line`**, and `DW_AT_location` = one `DW_OP_fbreg <offset>`, never a location list.
- Types (§9.4.4), a fixed table:

| LO type | DIE | Attributes |
|---|---|---|
| `int` | `DW_TAG_base_type` | `DW_ATE_signed`, byte_size 4, name `"int"` |
| `bool` | `DW_TAG_base_type` | `DW_ATE_boolean`, **byte_size 1**, name `"bool"` |
| void (return) | none | absence of `DW_AT_type` on the subprogram |
| `String` | `DW_TAG_pointer_type` → String class type | byte_size 8 |
| class `C` | `DW_TAG_pointer_type` → `C` class type | byte_size 8 |

- The pointee of every class (and String) reference is a **`DW_TAG_class_type`**: `DW_AT_name = "C"`, `DW_AT_byte_size = instance_size`; `DW_TAG_member` children for **the LO-declared fields** (name, type, `DW_AT_data_member_location` measured **from the object start, header included**: the first field is at 16); for a subclass, **one `DW_TAG_inheritance`** child (`DW_AT_type` = parent class DIE, `DW_AT_data_member_location = 0`). Members start at the first LO field; the 16-byte header is **not** described in DWARF. Class DIEs contain **no method DIEs**.
- **String's class type is name-only**: `DW_AT_name = "String"`, no members, nominal `DW_AT_byte_size = 16`.

**Location expressions (§9.5): three operators only.** `DW_OP_call_frame_cfa` (every frame base); `DW_OP_fbreg <sleb128>` (every parameter and local; offsets negative; a local at `[rbp-16]` is `DW_OP_fbreg -32` because CFA = rbp+16); `DW_OP_addr <addr>` for "program-scope globals the debugger may be asked to print: the in/out/err singletons and any compile-time-known global". No `DW_OP_reg*`/`breg*`, `piece`, `plus`, `deref`, `lit*`/`const*`.

**CFI (§9.6).** One CIE (CFA = rsp+8 at entry, return-address register 16, x86-64 alignment factors). Each FDE: `advance_loc` past `push rbp`; `def_cfa_offset 16` + `offset rbp, -16`; `advance_loc` past `mov rbp, rsp`; `def_cfa_register rbp`. **The epilogue needs no extra CFI** (a documented simplification: breakpoints land on `is_stmt` rows, never inside the epilogue). DWARF register numbers: 0 rax, 1 rdx, 2 rcx, 3 rbx, 4 rsi, 5 rdi, 6 rbp, 7 rsp, 8–15 r8–r15, 16 RA; nothing ≥ 17. A debugger **may** unwind by chasing the rbp chain without CFI, and the reference debugger may do so for `backtrace`; CFI is "the conformant mechanism" and the one that is right between entry and `mov rbp, rsp`.

**Consumer contract (§9.7).** One DWARF 5 CU; line-granular rows with `prologue_end`; CFA frame bases; one `DW_OP_fbreg` per variable; the §9.4.4 type table; three-state CFI. "A debugger that handles exactly these constructs handles every LO binary, and the reference debugger is held to no more." Value formatting by `DW_AT_type`:
- `int`: read 4 bytes, signed decimal. `bool`: **read 1 byte**, `false` for 0, else `true`.
- class reference: 8-byte pointer; `null` if 0; otherwise **the debugger may read the object header**: `class_descriptor` at offset 0, then the descriptor's `name`/`name_len` (ref §6.2.1), to print the **dynamic** class — "uses the ABI's fixed header layout directly rather than DWARF".
- String reference: recognized by the pointee class being the well-known `String` class; read the u32 length after the header and the inline UTF-8 bytes (ref §6.2.2). "The one place the debugger hard-codes knowledge of a specific class's layout."
- The debugger never interprets `gc_bits`/`flags`, "and it never mutates debuggee memory except through the explicit breakpoint mechanism (int3 plant/restore, the Native x86-64 Back End chapter §2.6)". **This sentence conflicts with the spec's graded `set var` and "memory read/write"** (spec p. 2–3); the spec's feature list governs what is graded, so keep `set var` and ask (question 13).

**Excluded (§9.8).** Location lists; `DW_TAG_lexical_block`, `DW_TAG_inlined_subroutine`; C++ member-function/vtable apparatus (`DW_AT_virtuality`, `DW_AT_vtable_elem_location`, `DW_AT_containing_type`); `.debug_macro`, `.debug_names`, `.debug_pubnames`, `.debug_pubtypes`; split DWARF/DWO, `.debug_addr`, address indices; **DWARF64** (DWARF32 throughout); `DW_AT_ranges`/`.debug_rnglists` (functions are contiguous); float/vector encodings; thread/LWP constructs. None of our drafts planned any of these [V: checked against D1–D10 and §5].

**Projects (§9.9).** The reference debugger reading reference-compiler DWARF is "the standing proof" that `.debug_line` + the DIEs + the CFI drive every P4 feature. **Hardware watchpoints need no DWARF.** Ch. 9 says nothing about LO-5, reflection, the Rust runtime's own debug info, or generated helpers other than the `main` wrapper.

**Two consequences for our debugger [I].** (1) The contract is about *any* LO `-g` binary, and §9.9 mentions a "reference-provided debug build": do not rely on our own symbol scheme. The reference mangles as `lometh__Foo__bar` (§9.4.2); ours is `lo_method_3_Foo_3_bar` (`symbols.rs:14-26`) [V]. Resolve functions through DIE names first. (2) Accept both `DW_FORM_string` and `DW_FORM_strp`, and `DW_FORM_line_strp` in the line header.

### 2.3 Debugger features (exercised through transcripts; also the feature ceiling)

- Process control: launch only (`PTRACE_TRACEME` + exec), no attach, single-threaded; `continue`; exit code reporting incl. runtime aborts.
- Software breakpoints: plant `int3`, restore original byte, step over and replant; by function, `file:line`, raw address.
- Stepping: `stepi`; `step` (into calls); `next` (over calls), driven by `.debug_line`.
- Inspection: register and memory read/write; backtrace by **CFI unwinding** with names from `.debug_info`; `info locals`; typed `print` per the DSR consumer contract.
- Hardware watchpoints in dr0–dr3 via `PTRACE_POKEUSER`/`PEEKUSER`, "the dr7 encode / dr6 decode discipline"; on a named local, `obj.field` or a raw address; report old value, new value, write site as `file:line`; four slots, managed.
- Two front ends over one engine: interactive prompt and batch mode.

### 2.4 Contributed tests (10%), code review (20%), viva (20%), docs

- ≥ 8 team-authored triples covering every feature bullet, **including a watchpoint test and a backtrace-through-recursion test** (spec p. 4).
- Code review: the process-interface abstraction and "the DWARF parsers' honesty (a parser that works by offset-guessing is legible as such)" (spec p. 6).
- Viva: drive an unseen program live at the prompt, explaining int3 bytes, `SIGTRAP`s and dr6 decoding (spec p. 6). Every team member should be able to.
- README (declare the debugger's language) and a 2–3 page design note: process interface, `.debug_line`/`.debug_info` data structures, breakpoint bookkeeping, dr7 slot management (spec p. 3–5).

---

## 3. Architecture sketch (all [I] design unless cited)

### 3.1 Compiler side (`-g`)

**Authoritative detail: the g-mode plan** (`planning/p4-g-mode.md`, `origin/docs/p4-g-plan`). Summary only:

- A `-g` flag and `DebugMode` threaded through every stage; **the default build stays byte-identical** (`--emit-asm`, `--emit-asm --spill-all`, `--dump-ir` gate, g-mode G1, G17).
- **Named values get fixed homes in both allocators**, not "force spill-all": named refs in functions with calls are pinned to fixed **GC root slots** (so the compacting collector keeps them current); every other named value gets a pre-assigned spill slot; prologue nulls root slots before storing params (g-mode G7, S11–S14). Named copies survive `remove_dead_copies` (G6).
- Lines: `terminator_line: Option<u32>` on `BasicBlock` (structural jumps `None`); the `.loc` rules live in `Selector::emit` (a per-function `seen` set; explicit `is_stmt`; `prologue_end` after `lo_push_frame`); **generated functions are emitted first with no `.loc` rows**, because **gas silently drops line-0 rows** (g-mode G0, G4, G5, G10).
- CFI: exactly the §9.6 three-state program, register numbers not names, **no epilogue CFI** (G11).
- DWARF: hand-written `.debug_abbrev`/`.debug_info` in a new `codegen/dwarf.rs`, **inline `DW_FORM_string`, no `.debug_str`**; `DW_LANG_C99`; `DW_AT_external` flag separating the two `main`s; `bool` byte_size 1; CU-level `in`/`out`/`err` variables via `DW_OP_addr` (G12).
- Toolchain (verified, G0): `as --gdwarf-5` under `-g`; **`gcc -static` drops `.eh_frame_hdr`, so link with `-Wl,--eh-frame-hdr`**; `cc -no-pie` keeps it. Exactly one CU in the linked binary from our `.s` (gas adds none). Stripping the runtime archive's debug info is optional (S2, Decision 16).
- Assembly and linking reuse `as` then `gcc -static` (`codegen/driver.rs:83-99`) [V]; the P3 grader links with `cc -no-pie` (`p3-grading-contract.sh:46`) [V]. Both give fixed load addresses, so ELF addresses equal runtime addresses. The debugger should not depend on `.eh_frame_hdr` (a linear `.eh_frame` scan is enough at our size).

### 3.2 Debugger crate (new `debugger/`, Rust), layers lowest first

1. `elf`: header, section headers, `.symtab` (Brand ch. 11).
2. `dwarf`: abbreviations, `.debug_info` DIE tree, DWARF 5 line program (Brand ch. 12–13, adapted from v4), CFI from `.eh_frame` (ch. 15), and a location-expression evaluator for exactly `DW_OP_call_frame_cfa`, `DW_OP_fbreg`, `DW_OP_addr` (ch. 19; [V §9.5]). Reject anything else loudly rather than guessing (code review grades parser "honesty").
3. `process` trait with two implementations: `LiveProcess` (nix ptrace, `cfg(target_os = "linux")`) and `MockProcess` (scripted, for laptop tests). Shape in D9.
4. `engine`: breakpoint and watchpoint tables (one shared numbering), stepping state machine (D6), CFI unwinder (Brand ch. 16), expression parser/evaluator (Brand ch. 21) and value renderer (ch. 20).
5. `script` parser and `transcript` printer (batch front end); `repl` (interactive veneer over the same engine).

**Runtime hooks: none.** The debugger reads object headers, class descriptors and Strings straight from debuggee memory (§5); the consumer contract explicitly allows exactly this [V §9.7].

### 3.3 Launching the debuggee (spec p. 2; Brand ch. 3)

`fork`; in the child: `personality(ADDR_NO_RANDOMIZE)` (Brand ch. 8 shows the same call), `ptrace(PTRACE_TRACEME)`, redirect stdout/stderr (D5), then `execv`. The parent `waitpid`s for the post-exec `SIGTRAP`, plants breakpoints, continues on `run`. Handle `ET_DYN` defensively (read `/proc/pid/maps` or the auxiliary vector) even though our links are not PIE.

### 3.4 Platform

- **Native only.** WASM and the interpreter are untouched (agent1 §2) [V].
- The live layer must run on the course host: hardware watchpoints and faithful ptrace do not survive the Rosetta container (spec p. 3) [V].
- Everything except `LiveProcess` is pure data processing and should build and test on macOS against fixture ELF binaries built once in the Linux container or on the host and checked in [I]. The spec recommends this split (spec p. 3).

---

## 4. Design decisions D1–D10 (draft; Neha decides)

Each: question, options, tradeoffs, draft recommendation, Brand check, and **Ch. 9** (what the DWARF contract settles; this line replaces the old "Depends on DSR"). Summary:

| D | Before (DSR missing) | Now |
|---|---|---|
| D1 | No | Confirmed by §9.1. **Changed** (g-mode G7): pinned homes in both allocators, named refs in root slots; not "force spill-all". |
| D2 | Yes | **Resolved** [V §9.1, §9.3]: `.loc` + `as --gdwarf-5`; `prologue_end`, `is_stmt`, no columns. Emission rules: g-mode G10. |
| D3 | Yes | **Resolved** [V §9.1, §9.6]: `.eh_frame` only; three-state CFI; **changed:** no epilogue CFI (g-mode G11). Runtime (Rust) CFI: ch. 9 silent. |
| D4, D5, D9 | No | No change (D9: see the §9.7 note on memory writes). |
| D6 | Partly | `prologue_end`/`is_stmt` **resolved** [V §9.3]; line lookup rule fixed; gdb rules still [I]. |
| D7 | Yes (DIE vs artificial) | **Changed** [V §9.4.2]: the `main` wrapper, implicit constructors and preamble I/O methods get DIEs (g-mode G12), `lo_entry` does not; no `DW_AT_artificial` on subprograms; function kind recorded in the `DebugInfo` side table (g-mode G3), not a `FunctionIr.origin` field. Whether implicit ctors / I/O methods appear in backtraces: open. |
| D8 | Yes | **Resolved** [V §9.4.4, §9.7]; subclass member layout **changed**. |
| D10 | Arch. §2.6 | §2.6 = §8.2.6 int3; nothing on dr7. **Changed:** width from `DW_AT_byte_size` (bool 1); field watches also watch the base variable's slot (compaction moves objects; g-mode "Known consequences"). |

### D1. How do named values get home slots under `-g`?

- **Options.** (a) Reuse the spill-all allocator: every vreg gets its own slot (`ir/register_allocator/spill_all.rs:16-25`) [V]; already a supported mode (`codegen/driver.rs:54-61`) [V]. (b) Linear scan with named vregs pre-assigned to the stack.
- **Tradeoffs.** (a) no allocator change, already exercised; slower code with many temporary slots. (b) keeps temporaries in registers but changes the allocator four days before the deadline.
- **Recommendation: superseded by g-mode G7 (follow it).** The earlier draft was (a). The g-mode plan instead pins named values in **both** allocators: (b) for linear scan, plus the same mapping in `spill_all.rs`. Named `Ref`s in functions with calls (`this`, ref params, ref locals) live in fixed **GC root slots** (`PhysicalLocation::Root`, numbered before temporaries); every other named value gets a pre-assigned `Spill(k)`. Reason: the Lisp-2 collector compacts and rewrites only root slots, so a named ref in a plain spill slot can point at a moved object when the user prints it. Plain spill-all has the same hole. Offsets still come from `Frame::spill`/`Frame::root`, never recomputed (`codegen/frame.rs:157-164`) [V].
- **Caveat [V], handled by g-mode G6.** `remove_dead_copies` runs on every function (`codegen/driver.rs:44-46`, `ir/liveness.rs:118`) and can delete a local's default-initialization copy (locals start as `0`, `false`, `null` or the empty string, `ir/lower.rs:228-242`), so the golden `c = null` could print garbage. Under `-g`, skip copies into named vregs.
- **Brand check.** No change. Brand ch. 20 reads variables via `DW_OP_fbreg` against the frame base: one slot per variable.
- **Ch. 9.** Confirms the approach: under `-g`, "rbp is always a true frame pointer … and every named parameter and local has a canonical home slot"; "named locals pinned to their home stack slots" while the default build keeps linear scan [V §9.1]. Block-locals are hoisted, so each is one more home slot for the whole function [V §9.1, §9.4.3]. If home slots were ever dropped, location lists would re-enter the subset (§9.5); they are excluded (§9.8).
- **Open [I].** The P1–P3 suites under `-g` (both allocators, `LO_GC_POISON=1`, small heap) are g-mode S14/G16; not yet run.

### D2. How is `.debug_line` produced, and what must our parser read?

- **Options.** (a) Emit `.file`/`.loc` and let GNU `as` build the line program. (b) Hand-encode the line program (opcodes and label arithmetic).
- **Tradeoffs.** (a) small and gdb-compatible; a version-5 table likely needs `.file 0` plus `--gdwarf-5` on the assembler, depending on host binutils [I] (no `as` was run). (b) full control and clear code-review story; several hundred lines.
- **Recommendation: (a), now fixed by the contract.** The reference does exactly this: "the `-g` debug object is assembled with `as --gdwarf-5` so the as-built `.debug_line`/`.eh_frame` are DWARF 5" and `as` produces `.debug_line_str` [V §9.1]. Pin `--gdwarf-5` in `driver.rs` for `-g` builds only. **Emission rules: follow g-mode G10** (verified against gas in G0 and simulated on real compiler output). In outline:
  - `.file 0 "<comp_dir>" "<name>.lo"` + `.file 1 "<name>.lo"` (gas accepts both; `.file 0` alone already yields a v5 table) [V g-mode G0];
  - `.loc 1 <line> 0 is_stmt N [prologue_end]`: column always 0, so no `DW_LNS_set_column` [V §9.3];
  - the **first** instruction of each not-yet-seen line in a function gets `is_stmt 1`; later instructions for an already-seen line get `is_stmt 0` (a per-function `seen` set; comparing with the previous `.loc` alone gives double stops); `is_stmt` written explicitly every time because gas's is sticky;
  - terminator-only statements get their row from `terminator_line` [V §9.3; Decision 4]; structural jumps carry no line;
  - `prologue_end` on the first body instruction, after root setup and `lo_push_frame` [V §9.3], with special cases for a first-statement `while` and empty bodies (G10 rule 6);
  - **generated functions emitted first, with no `.loc` at all**: gas silently drops line-0 rows, so code after one would inherit the previous row [V g-mode G0].
- **Parser must read** [V §9.3]: the DWARF 5 header with `.debug_line_str` paths; special opcodes, `copy`, `advance_pc`, `advance_line`, `set_file`, `negate_stmt`, `set_prologue_end`, `set_address`, `end_sequence`. Implement all twelve standard opcodes anyway (gas may emit `const_add_pc` or `fixed_advance_pc` [I]); they are a few lines each.
- **Brand check: firmed up, one new task.** Brand ch. 13 parses **DWARF 4 only** (its parser rejects other versions: "Only DWARF 4 is supported"); ch. 12 says DWARF versions are not backward compatible. P4 requires v5, so the parser is not a straight port. DWARF 5 line-header differences [I, from the DWARF 5 standard, not checked against a binary]:
  - adds `address_size` and `segment_selector_size` bytes;
  - directory and file entries described by entry-format descriptors (`directory_entry_format`, `file_name_entry_format`);
  - paths may be `DW_FORM_line_strp` offsets into `.debug_line_str`;
  - files numbered from 0 (entry 0 = primary source file), whereas Brand's DWARF 4 code subtracts 1 from file indices.
  - The DWARF 5 CU header also changes: adds a `unit_type` byte and reorders fields (version, unit_type, address_size, debug_abbrev_offset).
  - Brand's line-program state machine (registers, standard and extended opcodes) carries over.
  - Ch. 9 confirms version 5 and DWARF32 [V §9.1, §9.8] but does not spell out header bytes; it defers to the DWARF 5 standard for those. Check the bullets above against `readelf --debug-dump=line` on a real `-g` binary.
- **Ch. 9: resolved** [V §9.1, §9.3]. Lookup rule for the debugger is in D6. Open in g-mode (TODO, needs the triples): label a statement split across lines with its first or its last line (G5).

### D3. Which CFI format do we emit and parse?

- **Options.** `.cfi_*` directives producing `.eh_frame` (assembler default); `.cfi_sections .debug_frame` (or both); hand-written `.debug_frame`.
- **Tradeoffs.** The debugger must unwind through **Rust runtime frames**: reference-field writes happen inside `lo_gc_write_barrier` (`ir/lower/operations.rs:143-154`, `rust/src/gc.rs:34-42`) [V]; Rust release builds omit frame pointers (g-mode G8). 286 of 329 members of the x86 `liblo_runtime.a` carry `.eh_frame` [V g-mode G13].
- **Recommendation (compiler side: g-mode G11).** Emit for every function including `lo_entry` and the synthesized `main` (`codegen/mod.rs:51-67`) [V], `-g` only (`.cfi_startproc` makes gas emit `.eh_frame`, which would change default output):
  - `.cfi_startproc`
  - `.cfi_def_cfa_offset 16` after `push rbp`
  - `.cfi_offset 6, -16` (DWARF register numbers, not names, per g-mode)
  - `.cfi_def_cfa_register 6` after `mov rbp, rsp`
  - `.cfi_endproc`
  - This is exactly the §9.6 three-state program [V §9.6]. The `main` wrapper (`push rbp; mov rbp, rsp; call; call; pop rbp; ret`, `codegen/mod.rs:51-67` [V]) fits the same shape.
  - **Changed: no epilogue CFI.** The earlier draft added epilogue rules or `.cfi_remember_state`/`.cfi_restore_state`. §9.6: "The epilogue needs no extra CFI … a documented simplification". Leave it out (g-mode G11 agrees).
  - An `Abort` call at a function's very end has a return address equal to `.Lfunc_end`, which can sit in the *next* FDE: the debugger's `pc − 1` lookup covers it (g-mode G11).
  - **Changed: `.eh_frame` only.** `.debug_frame` is not emitted [V §9.1], so the parser reads `.eh_frame` only.
- **Brand check: firmed up.** Brand ch. 15: Linux toolchains "tend to leave [`.debug_frame`] blank, instead storing a similar (but slightly incompatible) format in the `.eh_frame` section"; the book's parser targets that (CIE augmentation strings, pointer encodings, optional `.eh_frame_hdr` search table). Parsing `.eh_frame` is the book's path and required for runtime frames anyway. CFI version is independent of `.debug_info` version, so DWARF 5 does not force a new CFI format.
  - From Brand ch. 16: look up the caller's FDE and line using **return address − 1** (the return address can belong to the next line or the next function); report caller frames at the line of the call.
  - Brand ch. 14 shortcut "return address = `[rbp+8]`" works for our LO frames but not runtime frames. §9.6 explicitly permits rbp-chain unwinding and lets the reference debugger use it for `backtrace`, but calls CFI "the conformant mechanism"; the spec says "backtrace by CFI unwinding" (spec p. 2). **Keep CFI as the primary path**, with the rbp chain as a cross-check and fallback.
- **Ch. 9: resolved for LO frames** [V §9.1, §9.6]. **Silent on runtime frames**: §9.6 specifies only the LO three-state FDE. Rust's FDEs (in `liblo_runtime.a`, not yet inspected) will use more CFA instructions (`def_cfa_offset` after each push, possibly `remember_state`/`restore_state`) [I]. Unwinding through them (Decision 15 option (a), §7.6) therefore needs a general CFA-instruction interpreter, beyond the subset. Size it after `readelf --debug-dump=frames` on the runtime.

### D4. Debugger language, crate placement, dependencies

- **Recommendation.** Rust (spec-recommended, with `nix` for ptrace, `waitpid`, `personality`; spec p. 3), as a new crate `debugger/` outside the compiler. Hand-write ELF and DWARF parsers: code review grades their "honesty" and the subset is closed. `gimli`/`object` could serve as a dev-only cross-check oracle if allowed (ask).
- **Brand check.** No change. Brand is C++; its process/target split maps to a Rust trait plus engine. `nix` is not in the local cargo cache [V]: ask whether the host has network access, or vendor it. Fallback: raw `libc::ptrace`.
- **Ch. 9.** No bearing on language. It names `readelf --debug-dump` and `objdump -g` on reference binaries as the way to check our output [V §9.1].

### D5. How is debuggee output kept out of the transcript?

- **Options.** Transcript on stdout with the child's stdout/stderr redirected (to `/dev/null` or a file) before `exec`; or the transcript written to a file.
- **Recommendation.** Redirect in the fork child with `dup2`, a flag to choose the target, and an optional stdin file for programs that read input. Exact interface must match the P4 grading contract (missing).
- **Brand check.** No change; Brand ch. 3's launch sequence has room for the redirect before `exec`.
- **Ch. 9.** Silent. Depends on the (still missing) grading contract.

### D6. How do we reproduce gdb's stopping rules?

gdb is the oracle: where execution stops decides most transcript lines. Brand implements simpler rules; where they differ, goldens follow gdb.

**Recommended rules** (calibrate on the host with real gdb in the first live session):

- **Function breakpoints** skip the prologue: use the `prologue_end` row [V §9.3: emitted by the producer, honored by the reference debugger]; fall back to the function's second row only for binaries without it.
- **Line breakpoints**: use `is_stmt` rows for that line [V §9.3]; if the row is the function's first address (the function's opening line), move to its `prologue_end` row [V §9.3]; one location per function, lowest address [I: gdb rule from memory]. Never plant at the CU's `end_sequence` address [V §9.3].
- **Mapping a PC to a line.** §9.3: "a stop's source line is found by the line table row whose address exactly matches (row-at-boundary), not by 'greatest row address ≤ pc'". That covers breakpoint and step stops, which land on rows. Some PCs are not on a row: caller frames in a backtrace (return address − 1), watchpoint traps (the instruction after the write), `stepi`. For those, ch. 9 gives no rule. **Proposed [I]:** greatest row ≤ pc *within the containing subprogram's `[low_pc, high_pc)`*. The range bound removes the cross-function aliasing §9.3 warns about. Flag it in the design note and ask (question 7).
- **Consumer rules from the g-mode line-table simulation** (g-mode "Known `-g` consequences"): when `step` enters a callee, skip rows before its `prologue_end`; a pc with no covering row (generated code, emitted before the first LO method) is "no line", so step out of it; resolve `break FILE:L` from `is_stmt 1` rows only and dedupe per function (a one-line constructor has two statement rows on one line; use the first); a line with no code (`}`, `} else {`, code after `return`) slides forward to the next line with code, as gdb does [I, confirm with triples].
- **`step`/`next`**: single-step until the PC leaves the current line's address range **and** lands at the start of an `is_stmt` row of a **different line number**.
- **`step` into a function without line info** (runtime helper, `lo_entry`, I/O wrapper): run to the return address and keep stepping, as `next` [I].
- **`next` over a call**: temporary breakpoint at the return address; accept the hit only when the CFA is not deeper than at the start, so recursive calls do not stop early.
- **Returning into the caller mid-line**: keep stepping to the start of the caller's next line rather than stopping inside the call's line [I: gdb behaviour from memory].

**Brand vs gdb: four differences** (Brand ch. 14 is a good skeleton: step instruction by instruction; `run_until_address` with a temporary internal breakpoint; report the stop as a step even if another breakpoint fired first):

1. Brand stops when the *line-table entry* changes, not the *line number*; two rows for one line give an extra stop. Compiler: emit `.loc` only on line change. Debugger: still compare line numbers.
2. Brand's step-in stops as soon as the PC has no line entry, so stepping into a runtime call stops inside Rust. gdb steps back out; we must too.
3. Brand's step-over runs to the instruction after the `call` without a frame check; stops early when the callee recurses into the same line. The CFA check fixes this.
4. Brand stops at the first new entry after returning to a caller (mid-line); gdb, from memory, does not [I].

**Prologue.** GCC does not emit `prologue_end`; Brand uses "the second line-table entry for a function". `prologue_end` is now required [V §9.3]. To make gdb and our debugger agree regardless of host gdb version: emit `prologue_end` **and** make the prologue a single row on the method's header line, with the first body statement as the second row. Our prologue contains a call: functions that call anything run `lo_push_frame` in the prologue (`codegen/frame.rs:207-219`) [V], and parameter moves precede it, so values are valid only after it. §9.3 places `prologue_end` "after `sub rsp, …` and the `lo_push_frame` setup", which matches.

**Detecting calls for `next`.** Brand uses a disassembler. We emit only `call symbol` and `call *operand` (`codegen/print.rs:52-53`) [V]: either decode those forms (`E8`, `FF /2`) or single-step and detect entry into a new function by PC range (no decoder needed) [I].

- **Ch. 9: partly resolved.** `prologue_end` and `is_stmt` are in the subset, columns are not, stepping is line-addressed, and breakpoints never land in the epilogue [V §9.3, §9.6]. The stepping rules themselves (`step`/`next` termination, step-out of no-line code, the CFA check, mid-line returns) are not in ch. 9; they are gdb behaviour and stay [I] until calibrated.

### D7. Line attribution for terminators and generated code

- **Terminator lines: DECIDED (team, 11/26; combined plan Decision 4).** Add `BasicBlock.terminator_line`, filled by `Builder::finish()` from a `Builder.current_line` set at the top of each statement; `finish()`'s signature unchanged. Lands in the shared-prep PR. Today `Terminator` has no line (`ir/mod.rs:257-270`) and every block is closed through `finish` (`ir/lower.rs:290-293`) [V]. Replaces agent1's per-variant `line` field. **Not to be reopened.** Ch. 9 requires exactly this: a row before the terminator of a terminator-only statement, "the reference emitter carries the line on the block" [V §9.3]. g-mode G4 refines the details within the decision: the field is `Option<u32>`; statement terminators take the statement's line, expression-internal branches (`&&`, `||`, ternary, null and division guards) the expression's line, `Abort` the guarded call's line, and **structural jumps** (loop entry, joins, back-edges) **`None`**, so a `while` breakpoint hits every iteration; the implicit `Return(None)` takes the last statement's line.
- **Generated functions: changed by ch. 9 and the g-mode plan.**
  - Ch. 9 gives a `DW_TAG_subprogram` to "each LO method, constructor, **and the synthesized main wrapper**"; the wrapper is the only one with `DW_AT_external = 1` [V §9.4.2]. So the earlier draft ("generated functions get no DIE") is wrong for `main`.
  - `DW_AT_artificial` is **not** in the subprogram attribute list (it appears only on `this`, §9.4.3), so the old alternative "DIEs marked `DW_AT_artificial`" is out.
  - **g-mode G12 (follow it):** DIEs for the `main` wrapper, every user method and explicit constructor, **and implicit constructors and preamble I/O methods** (they are LO constructors and methods; decl line = class line, 0 for the preamble). **`lo_entry` gets no DIE** (our own helper; the reference's wrapper does its work).
  - **g-mode G10:** all generated code (`main`, `lo_entry`, implicit constructors, I/O methods) is emitted **before the first LO method with no `.loc` rows**, because gas drops line-0 rows [V g-mode G0]. So `step` treats them as "no line" and steps out.
  - **Recognition: g-mode G3's `DebugInfo` side table records each function's kind** (user method, explicit or implicit constructor, preamble I/O method, `lo_entry`), **instead of a new `FunctionIr.origin` field** (combined plan Decision 5 changes accordingly, §7.4). Still never key on `GENERATED_LINE = 0` (`ir/mod.rs:82-85`) [V].
  - **Backtrace filter (debugger):** drop frames with no subprogram DIE (`lo_entry`, runtime, libc) and the DIE with `DW_AT_external = 1` (the C entry point). **Open:** whether implicit constructors and preamble I/O methods are "LO constructors" (shown) or "compiler-generated helpers" (omitted, spec p. 4). Ch. 9 is silent; settle from the triples. Because they have DIEs, the debugger can go either way without a compiler change.
- **Brand check.** Consistent: Brand ch. 16 builds backtrace names from subprogram DIEs; D6 rule (step into no-line function) handles stepping. The epilogue (incl. its `lo_pop_frame` call, `codegen/frame.rs:270-279`) [V] belongs to the `return` statement's terminator row, so `next` on a `return` reports no phantom line [I]; breakpoints never land in the epilogue [V §9.6].
- **Ch. 9.** Terminator rows: required [V §9.3]. DIEs for `main` wrapper: required [V §9.4.2]. DIEs for implicit constructors / preamble methods: implied by "each LO … constructor" (g-mode reading) but not stated. Backtrace treatment of those: silent.

### D8. Mapping an object to its dynamic class and fields

- **Options.** (a) Read the descriptor's `name` through the object header and look up the class-type DIE by name. (b) Map the descriptor address to a class via the `.symtab` symbol `lo_class_<len>_<Name>` (`compiler/src/symbols.rs:10-12`) [V]. (c) Read LO-5 reflection tables.
- **Recommendation: (a), now fixed by the contract.** The consumer "may read the object header at the pointee: the `class_descriptor` at offset 0, then the descriptor's `name`/`name_len`" to print the dynamic class, "using the ABI's fixed header layout directly rather than DWARF" [V §9.7]. Then find the `DW_TAG_class_type` with that `DW_AT_name` and render its members. (b) only as a cross-check on our own binaries: the reference compiler's symbols differ (§2.2.1). **Never (c)**: the consumer contract lists what the debugger reads, and reflection tables are not in it (§7.5).
- **Changed: subclass members.** The earlier draft (§5.3) put inherited fields first inside each class DIE. Ch. 9: each class DIE has `DW_TAG_member`s for **the fields it declares** plus **one `DW_TAG_inheritance`** (parent, `data_member_location = 0`) [V §9.4.4]; g-mode G12 emits exactly that. To print `Class@<heap> (f = V, ...)` with inherited fields first, the renderer walks the inheritance chain root-first, then the class's own members. Member offsets are absolute from the object start (first field at 16) [V §9.4.4].
- **String and null.** A pointer to the name-only `String` class: read u32 length at `+16`, bytes from `+20` (ref §6.2.2) [V §9.7]. A null pointer prints `null` [V §9.7].
- **Brand check.** Firms up the DIE shape: Brand ch. 20 renders classes from `DW_TAG_member` children with `DW_AT_data_member_location`. Ch. 9 confirms an LO class-typed variable is a pointer type to a class type [V §9.4.4]. Brand has no dynamic-type lookup; that part is LO-specific.
- **Ch. 9: resolved** [V §9.4.4, §9.7].

### D9. Process-interface shape (graded in code review)

- **Recommendation.** A trait with `read_mem`, `write_mem`, `get_regs`, `set_regs`, `read_debug_reg`, `write_debug_reg`, `cont`, `single_step`, and `wait`, which returns a `StopReason`: `SoftwareBreak`, `SingleStep`, `HardwareBreak(dr6)`, `Exited(code)`, `Signaled(sig)`. The engine never calls ptrace directly. Batch-mode tests run against a scripted mock (spec p. 5).
- **Brand check: firmed up.** See §6 (ptrace details) for siginfo classification, PC-after-int3, step-over-and-replant, debug-register offsets.
- **Ch. 9.** No bearing on the shape. One tension: §9.7 says the debugger "never mutates debuggee memory except through the explicit breakpoint mechanism", yet the spec grades `set var` and "memory read/write" (spec p. 2–3). Keep `write_mem`; ask (question 13).

### D10. What does `watch obj.field` watch, and how are hits reported?

- **Options.** (a) Evaluate `&obj.field` once when set; watch that address. (b) gdb-style: also watch the base variable `obj` and re-resolve when it changes (two slots per expression).
- **Recommendation: changed to (b)** (g-mode "Known `-g` consequences"). The Lisp-2 collector **compacts**, so a watched object can move and leave the debug register on the old address with no trap. Watching the base variable's home slot (a GC root slot under g-mode G7) catches the move, because the collector rewrites that slot; on that trap, re-resolve `obj + offset` and re-arm. A field watch costs two of the four slots; a base in a leaf function's plain slot cannot move. This is also what gdb does for `watch c.radius` [I]. **Report a hit only when the value actually changed**, else continue silently: gdb ignores same-value writes [I].
- **Widths: changed to `DW_AT_byte_size`.** int 4, **bool 1** [V §9.4.4], references 8. The earlier draft used our 4-byte bool storage width. A 1-byte watch at a bool's address still catches our 32-bit stores (they overlap it), the consumer reads 1 byte [V §9.7], and it stays correct on a reference binary that stores bools as one byte [I]. Use the same width for `set var` (our stored bools are 0/1, so writing the low byte is safe [V `codegen/frame.rs:60-65`]). All homes and fields are 8-byte aligned, so every width meets the alignment rule.
- **Brand check: firmed up, one rule added.** Brand ch. 9 dr7 layout and ch. 10 dr6 decode (see §6.4). Brand reports every trigger; gdb suppresses unchanged writes, so the comparison is our extra step.
- **From the Intel manual, not Brand [I].** The processor does not clear dr6: clear it after every hit. A data watchpoint traps **after** the writing instruction, so the stop PC is the next instruction and may already be on the next line. Calibrate the reported `FILE:L` against gdb.
- Writes inside the runtime (barrier, collector): §7.6.
- **Ch. 9 / ch. 8.** "Hardware watchpoints … need no DWARF" [V §9.9]. The "architecture reference §2.6" the spec cites is §8.2.6, the **int3** breakpoint; ch. 8 says only that dr0–dr3 are used "via the ptrace API" (§8.1.4). **No course document pins dr7 encoding, slot management, or a fifth watchpoint**: use Brand ch. 9–10 and ask (question 9).

---

## 5. Compiler changes and debugger data needs

### 5.1 Code areas

**Compiler side: see the g-mode plan** (`planning/p4-g-mode.md` on `origin/docs/p4-g-plan`), items G1–G17 and the 24-step build order S1–S24 with its dependency graph. It supersedes agent1's code-areas table that used to be here. The main differences from that older table: no forced spill-all (pinned homes in both allocators, G7); no `FunctionIr.origin` (kind lives in the `DebugInfo` side table, G3, new `ir/debug_info.rs`); a new `FunctionIr.named_root_slots` and `PhysicalLocation::Root` (G7); declaration lines for formals, locals and fields carried in the typed AST (G2); a typed `Item::Loc` with prologue/epilogue returning `Item`s (G9); `.type`/`.size`/`.Lfunc_end_<sym>` labels and generated-code-first layout (G10); no `DW_AT_linkage_name`, no `.debug_str` (G12); optional runtime-archive strip in the grading-contract build (S2/G13).

**Debugger side (this file):**

| Area | Change | Scale |
|---|---|---|
| **New** `debugger/` | ELF, DWARF (abbrev, info, line, `.eh_frame`) parsers; process trait (live and mock); engine; expression evaluator; renderer; script and transcript; REPL; tests. | Substantial |
| **New** `tests/debugger/`, README, design note | Deliverables 3–4. | Moderate |
| Front end, `interpreter/*`, `wasm/*` | No P4 change, except g-mode G2's declaration-line side data in the type checker (interpreter and wasm untouched). | Minor |

### 5.2 Object header and class descriptors [V] (`layout.rs:180-194`, `rust/src/object.rs`; ref §6.2, §6.2.1)

| Location | Content |
|---|---|
| `obj + 0` | descriptor pointer (first word of the object) |
| header size | 16 bytes |
| `desc + 0` | pointer to class-name bytes |
| `desc + 8` | `name_len` (u32) |
| `desc + 16` | parent descriptor pointer |

The consumer contract sanctions reading only `obj + 0` and the descriptor's `name`/`name_len` [V §9.7]; parentage comes from DWARF `DW_TAG_inheritance`, so the debugger does not need `desc + 16`. It never interprets `gc_bits`/`flags` [V §9.7].

### 5.3 Classes (one `DW_TAG_class_type` each; [V §9.4.4], emitted per g-mode G12)

- **Name**: exactly the LO class name, matching the descriptor's name bytes (agent1, `ir/statics.rs:86-96`).
- **Byte size**: `ClassLayout.instance_size`, which counts the hidden `Output` slot (`layout.rs:136-139`) [V]; do not compute `16 + 8n`.
- **Members: changed.** Only the fields the class **itself declares**, each at its absolute `FieldSlot.offset` (`16 + 8·index`, `layout.rs:58-60`) [V]; a subclass adds one `DW_TAG_inheritance` to its parent (offset 0). The debugger reconstructs inherited-first order by walking up the chain (D8). Hidden slots are not fields, so they get no member.
- **Which classes:** every class that can be the pointee of a reference [V §9.4.4], which includes the preamble classes `Input` and `Output` (formals may have those types; the objects are real heap objects with descriptors) [I: g-mode emits a type per class; confirm Input/Output are included]; `String` is name-only, byte size 16 [V §9.4.4]. ReflectInfo exists only in LO-5 builds (§7.8).
- **Methods**: no method DIEs inside class types [V §9.4.2].
- **Type widths**: int 4, bool **1**, references 8 [V §9.4.4]; these set watch length and `set var` width (D10).

### 5.4 Strings, null [V]

- String: `length: u32` at `+16`, UTF-8 bytes inline from `+20` (ref §6.2.2, `rust/src/object.rs:84-95`). Recognized by pointee class name `"String"` [V §9.7].
- `LO_EMPTY_STRING` is a static of length 0 and renders `""`; String locals start as it (`ir/lower.rs:238`).
- Null: pointer value 0; class-typed locals start as `Null` (`ir/lower.rs:237`).

### 5.5 Subprograms ([V §9.4.2]; emitted per g-mode G12)

- One per LO method, explicit and implicit constructor, preamble I/O method, and the C `main` wrapper; none for `lo_entry` (D7).
- `DW_AT_name`: the bare source name (`main` for `Main.main`, and also `main` for the wrapper). For constructors the source name is the class name, since an LO constructor is declared as `ClassName(...)` (ref P5/P6, §4.3) [I: follows from §9.4.2; confirm with triples].
- `DW_AT_external`: 1 only on the wrapper. `break main` resolves over `external = 0` subprograms (g-mode "Known consequences").
- **No `DW_AT_linkage_name`** (not in §9.4.2's list). Class-qualified `break Class.method` resolves by name + `DW_AT_decl_file`/`decl_line`, or by the link symbol in `.symtab`: ours is `lo_method_<n>_<Class>_<m>_<name>` / `lo_ctor_<n>_<Class>_<arity>` (`compiler/src/symbols.rs:14-26`) [V]; the reference's is `lometh__Foo__bar` (§9.4.2), so prefer DIE names.
- `DW_AT_low_pc`/`high_pc` (offset to `.Lfunc_end_<sym>`), `DW_AT_frame_base = DW_OP_call_frame_cfa`, decl file/line, return `DW_AT_type` (absent for void).
- Children in source order: `this` first (`DW_AT_artificial`, so ARGS omits it) [V §9.4.3], then params, then locals (`info locals` order), each with `DW_AT_decl_line` and one `DW_OP_fbreg`.
- CU-level `DW_TAG_variable`s for `in`/`out`/`err` with `DW_OP_addr` into the static `lo_bindings` frame (`symbols.rs:5-8`) [V; g-mode G12]. §9.5 names these globals but §9.4's DIE list does not show CU-level variables; graded `print` never names them (spec p. 3).

### 5.6 Frames under `-g` ([V §9.2, §9.4.2, §9.5]; homes per g-mode G7)

- After the prologue: CFA = `rbp + 16`; return address at `rbp + 8`; saved `rbp` at `rbp` (`codegen/frame.rs:8-17`) [V].
- **Frame base resolved: `DW_OP_call_frame_cfa`** [V §9.4.2]. A home at `[rbp + d]` is `DW_OP_fbreg (d − 16)`; e.g. `[rbp − 16]` → `fbreg −32` [V §9.5]. `d` comes from `Frame::spill(k)` or `Frame::root(slot)` (g-mode G7, S22).
- The CFA needs the CFI evaluator for caller frames; for frame 0 at a statement stop (always after `prologue_end`), CFA = `rbp + 16` directly.

### 5.7 GC and named values (changed by g-mode G7)

The collector is a compacting Lisp-2 that rewrites root slots and heap fields, never registers or plain spill slots [V g-mode G7]. Under g-mode G7, named refs in functions with calls **are** their root slots, so they stay current in every frame, including callers during recursion; leaf functions cannot collect. Remaining caveats: an `obj.field` watch must follow a moved object (D10); the collector's own writes to watched slots must be filtered (§7.6). An object held by a dead local stays alive until return under `-g` (g-mode G7 costs).

---

## 6. ptrace, signals and debug registers (Brand-derived)

### 6.1 Launch (Brand ch. 3, ch. 8)

`fork` → child: `personality(ADDR_NO_RANDOMIZE)`, `ptrace(PTRACE_TRACEME)`, `dup2` redirects, `execv` → parent `waitpid` for post-exec `SIGTRAP`. Brand ch. 3's trivial launch-and-wait is the first host smoke test.

### 6.2 Classifying stops (Brand ch. 10)

Call `PTRACE_GETSIGINFO` on each `SIGTRAP` inside `wait` and hide the result behind `StopReason`:

| `si_code` | Meaning |
|---|---|
| `TRAP_TRACE` | single step completed |
| `TRAP_HWBKPT` | debug-register (hardware) hit |
| `SI_KERNEL` | `int3` executed — **not** `TRAP_BRKPT`, a long-standing x86-64 kernel quirk |

### 6.3 Software breakpoints (Brand ch. 7)

- Plant: save the original byte, write `0xCC`.
- After an `int3`, the PC is one past the breakpoint; when an enabled breakpoint lies at `pc - 1`, reset PC to `pc - 1`.
- On resume: restore the byte, single-step, replant (step-over-and-replant).
- Viva material: int3 bytes, `SIGTRAP`s.

### 6.4 Debug registers (Brand ch. 5, 9, 10)

- Read/write with `PTRACE_PEEKUSER`/`POKEUSER` at `offsetof(struct user, u_debugreg) + 8*i` (Brand ch. 5).
- dr0–dr3: watched addresses. dr6: status (which slot fired). dr7: control.
- **dr7 layout for slot *i*** (Brand ch. 9):
  - local-enable bit: `2i`
  - condition bits (RW): `16 + 4i` (2 bits). `00b` = instruction execution only, `01b` = **data writes only**, `11b` = data reads and writes.
  - length bits (LEN): `18 + 4i` (2 bits). `00b` = 1 byte, `01b` = 2, `11b` = 4, `10b` = 8 (**8 is `10b`, not `11b`**).
  - The watched address must be aligned to its length.
- **dr6 decode** (Brand ch. 10): low four bits; the lowest set bit gives the slot. Keep the previous and current value per watchpoint to print Old/New.
- [I, Intel manual] Clear dr6 after every hit (hardware does not). Data watchpoints trap after the write; the stop PC is the next instruction.
- Four slots; a fifth watchpoint's behaviour is unknown (instructor question 9). `architecture-reference.md` §2.6 may pin encoding and slot management.

### 6.5 Brand chapter pointers per topic

| Topic | Brand chapter |
|---|---|
| Launch, ptrace basics | ch. 3 (Attaching to a Process) |
| Testing harness, procfs | ch. 4 (Pipes, Procfs, and Automated Testing) |
| Registers, `struct user`, debug-register offsets | ch. 5 (Registers) |
| Software breakpoints, int3, step-over | ch. 7 (Software Breakpoints) |
| Memory read/write; `personality` call; disassembly | ch. 8 (Memory and Disassembly) |
| dr7 encoding, watchpoints | ch. 9 (Hardware Breakpoints and Watchpoints) |
| siginfo, dr6 decode, stop classification | ch. 10 (Signals and Syscalls) |
| ELF, section headers, `.symtab` | ch. 11 (Object Files) |
| DWARF intro, DIEs, abbrevs, frame base example | ch. 12 (Debug Information) — DWARF 4 only |
| Line tables | ch. 13 (Line Tables) — DWARF 4 only |
| Source breakpoints, step/next/finish, prologue skip | ch. 14 (Source-Level Breakpoints and Stepping) |
| `.eh_frame` / CFI parsing | ch. 15 (Call Frame Information) |
| Unwinding, backtrace | ch. 16 (Stack Unwinding) |
| Location expressions (`DW_OP_fbreg`) | ch. 19 (DWARF Expressions) |
| Variables, types, members, rendering | ch. 20 (Variables and Types) |
| Expression evaluation (`print` expressions) | ch. 21 (Expression Evaluation) |
| Not needed (static, single-threaded) | ch. 17 (Shared Libraries), ch. 18 (Multithreading) |

### 6.6 Brand-vs-gdb differences (collected)

1. Step termination on line-table entry change vs line-number change (D6 #1).
2. Step-in stops in no-line code vs gdb steps back out (D6 #2).
3. Step-over has no frame (CFA) check vs gdb's recursion-safe check (D6 #3).
4. Return-to-caller mid-line stop vs gdb continues to the next line start [I] (D6 #4).
5. Watchpoints: Brand reports every trigger; gdb suppresses same-value writes [I] (D10).
6. Brand's DWARF parsers are version 4 only; P4 needs version 5 (D2).
7. Brand's prologue skip = second line-table entry (GCC emits no `prologue_end`); emit both `prologue_end` and a single-row prologue so either rule agrees (D6).

---

## 7. Constraints from the parallel LO-5 work

LO-5 (reflection) runs in parallel on its own branch.

### 7.1 Already decided (team, 11/26)

- **Gating (Decision 1).** ReflectInfo injection and emission are gated on a pre-scan of the parsed program (one `uses_reflection()` check, reused by lowering and WASM rejection); `ReflectInfo` stays a reserved class name. LO-4 output and every P4 debuggee are unchanged. P4 consequence: no ReflectInfo DIEs or wrappers in graded binaries.
- **Parallel work (Decision 2).** `main` receives P4 work plus one small shared-prep PR. LO-5 lives on `lo5/integration` until `p4-final` is tagged, rebased on `main` daily, merged after the tag. New code in new files; hotspot rules in §7.3.
- **Terminator lines (Decision 4).** `BasicBlock.terminator_line` via `Builder.current_line`; `finish()`'s signature unchanged (D7).
- **Staffing (Decision 3) is not decided**: who works on P4 vs LO-5 this week, and who prepares the LO-5 talk if Group 6 is in block 1 on 11/30. Options: everyone splits; some P4-only while others do LO-5 work touching no shared file; LO-5 limited to contracts + shared PR until `p4-final`.

### 7.2 Shared-prep PR (combined plan §5)

Small; reviewed by one person from each side; then `lo5/integration` rebases onto it.

| # | Item | Files | Owner (reviewer) |
|---|---|---|---|
| 1 | `BasicBlock.terminator_line` + `Builder.current_line` (decided) | `ir/mod.rs`, `ir/lower.rs` | **P4 compiler owner (TBD)** |
| 2 | `FunctionIr.origin` (§7.4) | `ir/mod.rs`, `ir/lower.rs`; struct literals at `ir/lower.rs:201`, `ir/tests.rs:24` | **P4 compiler owner (TBD)** (reviewers Neha, Sarthak) |
| 3 | `DataItem::Word` (LO-5 only, but in `ir/mod.rs`) | `ir/mod.rs`, `statics.rs`, `codegen/data.rs`, `dump.rs`, `verify.rs` | Sarthak |
| 4 | `FieldSlot.storage_bytes` + hidden-slot generalization + cross-check against `width_of` | `layout.rs` | Jay |
| – | Optional: `FunctionIr.lo_types` parallel to `register_names` | `ir/mod.rs` | P4 compiler owner |

**Blocker:** the P4 compiler owner must be named; items 1 and 2 cannot start without one, and `ir/mod.rs` freezes after this PR. Target: merged by noon Fri 11/27.

### 7.3 Branch rules for hotspot files

- `ir/lower.rs`: LO-5 adds only one-line match arms calling into a new `ir/lower/reflect.rs`; **P4 owns** `Builder::new`, `finish`, `complete`.
- `layout.rs`: after the shared PR only Jay edits `ClassLayout`; LO-5's `ReflectLayout` is a separate block.
- `ir/mod.rs`: frozen after the shared PR; later changes go through a mini-PR to `main` with a heads-up.
- `ir/statics.rs`: LO-5 adds only a one-line call. P4 puts DWARF in the new `codegen/dwarf.rs`, not here.
- `codegen/*`, `driver.rs`: P4 only. Front end, `rust/src`, `wasm/*`, `interpreter/*`: LO-5 only.
- Any hotspot edit outside these rules gets a heads-up in team chat.
- No LO-5 merges to `main` on Mon 11/30.

### 7.4 The `FunctionIr.origin` contract (combined plan Decision 5, recommended, in shared PR)

- Explicit `FunctionIr.origin`, e.g. `UserMethod{class, name}`, `UserCtor{class}`, `ImplicitCtor`, `PreambleMethod`, `Generated`.
- `add_io_classes.rs:5-9` warns against inferring "preamble" from line 0 [V]; **do not key on `GENERATED_LINE`**.
- Rejected alternatives: infer from line 0 (fragile; cannot tell implicit ctor from I/O wrapper); infer from symbol-name patterns in the debugger (every new generated kind needs a pattern; misses break silently).
- LO-5's construct functions (`lo_construct_*`) set `Generated`; its ReflectInfo query wrappers (ten of them) set `PreambleMethod`, like the I/O wrappers. Both use `GENERATED_LINE`.
- Construct functions are built through `Builder::new` (no receiver), so they inherit any debug fields P4 adds.
- Lowering of user-written reflective syntax passes the node's real line, never `GENERATED_LINE`.
- P4 emits subprogram DIEs and line rows only for `UserMethod`/`UserCtor`. Whether others get `DW_AT_artificial` DIEs instead depends on the DSR.
- Reviewers: Neha (wrappers), Sarthak (construct functions).

### 7.5 Shared class model (combined plan Decision 6)

P4's DWARF members and LO-5's field records both come from `ClassLayout` (shared-prep item 4). Hidden slots stay out of `fields` and `pointer_offsets` but count in `instance_size`, so DWARF members exclude them automatically. Whichever emitter lands second adds a test that DWARF member offsets equal reflection field offsets. **The debugger reads DWARF, never `lo_reflect_*` tables**: those exist only in LO-5 builds and the P4 contract is closed.

### 7.6 Watchpoint write site (combined plan Decision 15; P4 debugger owner decides)

- Reference-field writes already happen inside `lo_gc_write_barrier`, in Rust (`ir/lower/operations.rs:143-154`, `rust/src/gc.rs:34-42`) [V]; LO-5 adds int and bool writes inside `lo_reflect_set_*` (`setfield`, Jay). The trap fires with the PC in runtime code.
- **Pick one general rule**: (a) unwind via CFI to the nearest frame with an LO subprogram DIE and report that frame's line, or (b) print whatever gdb prints. Calibrate against gdb on the host.
- Inlining the barrier under `-g` is ruled out (deviates from `runtime-abi.md` §3.4, and does not help `lo_reflect_set_*`). LO-5 commits to never inlining reflective stores. The FSD's "Debugger Frontend: None" row gets a footnote (G16).

### 7.7 Runtime debug info (combined plan Decision 16; P4 owner decides)

- If `liblo_runtime.a` carries DWARF (unverified; Rust's precompiled `std` usually ships line tables [I]), then `llvm-dwarfdump --verify` also checks non-LO units, and gdb's `step` may enter Rust source (including `lo_reflect_*` helpers) and diverge from our debugger.
- The debugger still needs the runtime's `.eh_frame` to unwind; keep it.
- Options: strip runtime `.debug_*` in `-g` links (keeping `.eh_frame`), or leave it; the instructor may require one.
- One owner, chosen by the P4 design lead, asks the instructor and decides. Ask which runtime the grader links. LO-5 accepts the outcome.

### 7.8 `-g` on LO-5 programs (combined plan Decision 17; P4 owner decides)

- Recommended: compile-and-verify — must compile and pass `llvm-dwarfdump --verify`; debugger behaviour on reflective calls best effort, not golden-tested. Alternatives: full support (golden-tested) or unsupported.
- P4's graded triples are LO-4 programs, so not a grading risk [I].
- Spill-all does not conflict with LO-5's indirect calls (`codegen/select.rs:368-371`) [V].
- One LO-5 `-g` test is added after both efforts merge (Sarthak).

---

## 8. Build order and test plan

### 8.1 Suggested build order, Thu 11/26 – Mon 11/30

Every day ends with something demonstrable. Owners left to Neha. Natural four-track split: (A) compiler `-g` and DWARF emission; (B) process layer and breakpoints/stepping; (C) DWARF parsers, values and backtrace; (D) script/transcript, triples and design note.

| Day | Compiler side | Debugger side |
|---|---|---|
| Thu 11/26 | Design session; name P4 compiler owner; shared-prep PR items 1–2. `-g` flag, forced spill-all, keep named default copies. Request missing contracts. | Crate skeleton; process trait and mock; script parser and transcript printer tested against the spec example; ELF and `.symtab`. Log in to the host; trivial launch-and-wait (Brand ch. 3). |
| Fri 11/27 | `.file`/`.loc`, `prologue_end`, CFI directives; check with `readelf --debug-dump=line` and gdb on the host. Merge shared PR by noon. | Live process: launch, continue, exit codes, `int3` by address and function with step-over-and-replant, `stepi`. `.debug_line` v5 parser; `file:line` breakpoints; `step`/`next` (D6). |
| Sat 11/28 | `codegen/dwarf.rs`: CU, subprograms, params/locals, type table; `llvm-dwarfdump --verify` clean on the corpus. | `.debug_info` parser; `info locals`; `print` with typed rendering; `.eh_frame` parser and CFI backtrace with LO-frame filter. |
| Sun 11/29 | Fix gdb disagreements found in calibration. | Watchpoints (dr7/dr6, four slots, value-change check); `set var`; `x/1dw`; address expressions; registers. Author triples; calibrate against gdb. |
| Mon 11/30 | Freeze. | Hardening, README, design note; tag `p4-final` well before 11:59pm. No LO-5 merges to `main` that day. |

**Cut order if the schedule slips:** interactive polish → `break *<address>` edge cases → `obj.field` watch re-evaluation. **Do not cut** the triples (10%) or the design note.

### 8.2 Test-triple plan (≥ 8; target 10)

Each triple: no raw addresses in script or transcript. Generate goldens by running real gdb on the host and rewriting its output into the grammar, then review each by hand [I]. Cover every bullet of spec §2.2 at least once.

| # | Program | Script exercises |
|---|---|---|
| T1 | Straight-line `Main.main` with int and bool locals | `break file:line`, `run`, `print`, `info locals`, `continue`, exit code |
| T2 | Two classes, a method call | `break <function>`, `step` into the method, `next` over a call, `backtrace` with ARGS (`this` omitted) |
| T3 | **Recursion** (factorial) — required | Breakpoint at base case; `backtrace` through *n* frames; `next` over a recursive call (CFA check) |
| T4 | Class-typed local, null then constructed; subclass held in a superclass variable | `print c` as `null`, then `Class@<heap> (...)` with dynamic class; a String field and a reference field (`Class@<heap>` without fields) |
| T5 | **Watchpoint** on an int local in a loop — required | `watch`, `continue` twice (old/new values, write-site line), `delete`, `continue` to exit |
| T6 | Watchpoint on `this.field` inside a method, and on a reference field (write in the barrier) | Field watch; write-site rule (§7.6); shared numbering with breakpoints |
| T7 | Locals and fields | `set var` on int, bool and a field; `x/1dw *(int*)((char*)c + 16)`; `print $rax` after `stepi` |
| T8 | Runtime abort (cast failure 101 or null receiver 102) | Stop before the abort, `continue` to `Debuggee exited with code 101` |
| T9 | Several breakpoints | `delete <n>`, re-run behaviour, `stepi` `Stopped:` lines |
| T10 | Strings with quotes or escapes | String rendering and escaping (instructor question 8) |

---

## 9. Risks

1. **Missing DSR** (blocking): every "depends on DSR: yes" item may need rework when it arrives.
2. **Schedule**: five days over a holiday, nothing started, talk block 1 the same Monday. If Group 6 presents on 11/30, give talk prep to someone off the P4 critical path.
3. **gdb disagreement**: stepping and watch-stop lines follow gdb, and most D6 rules are from memory [I]. Calibrate on day 2, not day 4.
4. **DWARF 5 parsing**: Brand's parsers are DWARF 4 only (D2); header and form handling is new work.
5. **Runtime frames**: unwinding through Rust depends on its `.eh_frame` (unverified).
6. **Host dependencies**: `nix` not cached locally; host network access unknown.

## 10. Questions for the instructor (full list)

1. Where are the DSR, `architecture-reference.md` §2.6, the published triples and the P4 grading contract?
2. Who builds the `-g` binary for grading (our `--native` link or the grader from our `.s`), with which `as` version and flags, and `-no-pie` or `-static`?
3. How is batch mode invoked; where does the transcript go; how is debuggee stdin supplied?
4. May or must runtime debug info be stripped? Which runtime does the grader link?
5. What is `DW_AT_name` for constructors? Does `break` take `main`, `Main.main` or a qualified name, given that `break main` also matches the C `main` in `.symtab`? What happens with same-named methods in several classes?
6. Which `break *<address>` forms appear, given that addresses must not appear in transcripts?
7. Stepping: the prologue rule for one-line methods; `step` into I/O wrappers or implicit constructors; `step` onto an abort path; are `prologue_end` and `is_stmt` in the subset?
8. Rendering: String arguments in backtraces (`<heap>` or contents)? String fields inside objects? Escaping of quotes and newlines? `print this`? Register width and sign for `$rax`? The EXPR echo for `x/1dw`? Accepted `set var` value forms (negative numbers, `null`)? Exact whitespace in `bt-line`?
9. Watchpoints: does `watch obj.field` also watch `obj`? Which `FILE:L` is reported for a write inside a runtime helper, and for a write that is the last instruction of a line? What happens on a fifth watchpoint? Are same-value writes suppressed?
10. Are graded programs GC-free, or should we pass a large heap size?
11. May `gimli`/`object` be used as test-only oracles?
12. Can crates (e.g. `nix`) be fetched on the course host?

## 11. Not verified in preparing this material

No `cargo`, `as`, `gcc`, `gdb` or `llvm-dwarfdump` was run (not allowed for the planning task; `gdb` and `llvm-dwarfdump` are not installed on the planning Mac [V]). Still open:

- gas DWARF-5 output from `.loc` (does `.file 0` + `--gdwarf-5` work with host binutils?);
- whether `liblo_runtime.a` has `.debug_*` and `.eh_frame`;
- whether spill-all passes the full corpus;
- all gdb behaviour (D6 rules, same-value watch suppression, watch-stop line, `bt-line` whitespace);
- DWARF 5 line/CU header details (from the standard, not checked against a binary);
- course-host access was not tried.

## 12. Provenance (where each item came from)

| Item | Sources |
|---|---|
| §1 status, missing contracts | agent1 header, §6.1, §7; agent3 §5; combined plan §6.1; spec p. 1, 3, 6 |
| §2 requirements | spec p. 1–6; agent1 §1 A–F; `runtime-abi.md:243-251` |
| §3 architecture | agent1 §2; spec p. 2–3; Brand ch. 3, 8, 11–16, 19 |
| D1 home slots | agent1 D1; Brand ch. 20 |
| D2 line table | agent1 D2; Brand ch. 12–13 |
| D3 CFI | agent1 D3; agent3 §0.8; Brand ch. 14–16 |
| D4 language | agent1 D4, risk 13; spec p. 3 |
| D5 output separation | agent1 D5, risk 12; Brand ch. 3 |
| D6 gdb semantics | agent1 D6, risk 7; Brand ch. 14 |
| D7 line attribution | agent1 D7; agent3 §0.3, §0.10, §2.1, §2.4; combined plan Decisions 4–5 |
| D8 dynamic class | agent1 D8, §5; agent3 §1B; Brand ch. 20 |
| D9 process interface | agent1 D9; Brand ch. 5, 7, 10 |
| D10 watchpoints | agent1 D10, risk 9; Brand ch. 9–10 |
| §5.1 code areas | agent1 §4; agent3 §1A |
| §5.2–5.7 debugger needs | agent1 §5; agent3 §0.1–0.2; Brand ch. 12 (frame base) |
| §7.1 decided items | combined plan Decisions 1, 2, 3, 4 (team, 11/26); agent3 §2.1, §2.5, §2.8 |
| §7.2 shared-prep PR | agent3 §2.3, §3; combined plan §5 |
| §7.3 branch rules | agent3 §3; combined plan §5 |
| §7.4 origin contract | agent3 §1C.3, §2.4, §4.2; combined plan Decision 5 |
| §7.5 shared class model | agent3 §1B, §2.2; combined plan Decision 6 |
| §7.6 write site | agent3 §1D.3, §2.6; combined plan Decision 15; agent1 risk 3 |
| §7.7 runtime debug info | agent1 risk 4; agent3 §1D.7; combined plan Decision 16 |
| §7.8 `-g` on LO-5 | agent3 §0.5, §2.7; combined plan Decision 17 |
| §8 build order | spec p. 5; agent1 risk 2; agent3 §3; combined plan §5 |
| §9–11 risks, questions | agent1 §6; agent3 §5; combined plan §6 |
