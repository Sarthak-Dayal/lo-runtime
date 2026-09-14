# WASM production index

Construct names link to the design rules; implementation links locate their handlers.
Each handler has a short production comment. Rules erased by parsing or checking
are listed explicitly and need no emission handler. The
[test report](wasm-test-results.md) separates implementation from execution evidence.

| Production | Construct | Implementation |
|---|---|---|
| P1 | [Program](wasm-emitter-design.md#program) | [module.rs](../compiler/src/wasm/module.rs): `p1_program`; [function.rs](../compiler/src/wasm/function.rs): `entry` |
| P2 | Method-only program | Outside LO-3/LO-4 |
| P3 | Bare-body program | Outside LO-3/LO-4 |
| P4 | [Class / extends](wasm-emitter-design.md#program) | [module.rs](../compiler/src/wasm/module.rs): `p4_class` |
| P5 | [Constructor / this delegation](wasm-emitter-design.md#constructors) | [function.rs](../compiler/src/wasm/function.rs): `p5_constructor`, `p6_delegation` |
| P6 | [Constructor / super delegation](wasm-emitter-design.md#constructors) | [function.rs](../compiler/src/wasm/function.rs): `p5_constructor`, `p6_delegation` |
| P7 | [Method](wasm-emitter-design.md#functions) | [function.rs](../compiler/src/wasm/function.rs): `p7_method`, `io_wrapper` |
| P8 | Standalone body | Outside LO-3/LO-4 |
| P9 | [Formals](wasm-emitter-design.md#functions) | [function.rs](../compiler/src/wasm/function.rs): `p9_formals` |
| P10 | [Actuals](wasm-emitter-design.md#calls) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p10_actuals` |
| P11 | [Variable declaration](wasm-emitter-design.md#functions) | [function.rs](../compiler/src/wasm/function.rs): `p11_locals`; field layout in `p4_class` |
| P12 | [Block](wasm-emitter-design.md#functions) | [statements.rs](../compiler/src/wasm/statements.rs): `p12_block` |
| P13 | [Return](wasm-emitter-design.md#statements) | [statements.rs](../compiler/src/wasm/statements.rs): `p13_return` |
| P14 | [If / else](wasm-emitter-design.md#statements) | [statements.rs](../compiler/src/wasm/statements.rs): `p14_if` |
| P15 | [While](wasm-emitter-design.md#statements) | [statements.rs](../compiler/src/wasm/statements.rs): `p15_while` |
| P16 | [Break](wasm-emitter-design.md#statements) | [statements.rs](../compiler/src/wasm/statements.rs): `p16_break` |
| P17 | [Assignment](wasm-emitter-design.md#statements) | [statements.rs](../compiler/src/wasm/statements.rs): `p17_assign`, `store_field` |
| P18 | [Empty statement](wasm-emitter-design.md#statements) | [statements.rs](../compiler/src/wasm/statements.rs): `p18_empty` |
| P19 | [Void call statement](wasm-emitter-design.md#calls) | [statements.rs](../compiler/src/wasm/statements.rs): `p19_call_statement`, using P23 |
| P20 | [this expression](wasm-emitter-design.md#simple) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p20_this` |
| P21 | [null](wasm-emitter-design.md#simple) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p21_null` |
| P22 | [new](wasm-emitter-design.md#allocation) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p22_new`, `allocate` |
| P23 | [Value call](wasm-emitter-design.md#calls) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p23_call` |
| P24 | Unqualified call | Outside LO-3/LO-4 |
| P25 | [Ternary](wasm-emitter-design.md#operators) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p25_ternary` |
| P26 | [Binary expression](wasm-emitter-design.md#operators) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p26_binary` |
| P27 | [Unary expression](wasm-emitter-design.md#operators) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p27_unary` |
| P28 | [Parentheses](wasm-emitter-design.md#simple) | Parser removes the wrapper; `expression` emits the enclosed node |
| P29 | [Cast](wasm-emitter-design.md#casts) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p29_cast`; downcasts need the runtime implementation |
| P30 | [instanceof](wasm-emitter-design.md#casts) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p30_instanceof`; runtime implementation missing |
| P31 | [Variable expression](wasm-emitter-design.md#simple) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p31_variable` |
| P32 | [Literal expression](wasm-emitter-design.md#simple) | Typed AST uses the P40-P43 variants directly |
| P33 | [Binary operator](wasm-emitter-design.md#operators) | `p26_binary`, `integer_instruction`, `short_circuit`, `total_division` |
| P34 | [Unary operator](wasm-emitter-design.md#operators) | `p27_unary` selects the checked operand's operation |
| P35 | [void](wasm-emitter-design.md#types) | [mod.rs](../compiler/src/wasm/mod.rs): `signature`; no result local |
| P36 | [Class type](wasm-emitter-design.md#types) | `is_reference`; i32 address, null default, root slot |
| P37 | [int](wasm-emitter-design.md#types) | i32 local/field, zero default |
| P38 | [bool](wasm-emitter-design.md#types) | i32 local/field, zero default; true is 1 |
| P39 | [String](wasm-emitter-design.md#types) | `is_reference`, `p11_locals`, `allocate`; empty-string default and root slot |
| P40 | [Integer literal](wasm-emitter-design.md#simple) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p40_number` |
| P41 | [true](wasm-emitter-design.md#simple) | `p41_p42_boolean(true)` |
| P42 | [false](wasm-emitter-design.md#simple) | `p41_p42_boolean(false)` |
| P43 | [String literal](wasm-emitter-design.md#simple) | `p43_string`; nonempty literals need `lo_string_new` |
| P44 | [Class name](wasm-emitter-design.md#names) | [mod.rs](../compiler/src/wasm/mod.rs): `class_symbol`; checked class-table lookup |
| P45 | [Method name](wasm-emitter-design.md#names) | `method_symbol`; checked owner and slot lookup |
| P46 | [Variable receiver](wasm-emitter-design.md#calls) | [expressions.rs](../compiler/src/wasm/expressions.rs): `p46_p49_receiver`, using P31 |
| P47 | [this receiver](wasm-emitter-design.md#calls) | `p46_p49_receiver`, using P20 |
| P48 | [super receiver](wasm-emitter-design.md#calls) | `p46_p49_receiver`; P23 uses the resolved ancestor |
| P49 | [Computed receiver](wasm-emitter-design.md#calls) | `p46_p49_receiver`; evaluates its expression once |
| P50 | [Variable name](wasm-emitter-design.md#names) | Checker resolves the binding; P17/P31 store/read it |
| P51 | [Number token](wasm-emitter-design.md#names) | Lexer validates it; P40 emits the i32 value |
| P52 | [String token](wasm-emitter-design.md#names) | Lexer decodes it; P43 emits UTF-8 bytes |
| P53 | [Identifier token](wasm-emitter-design.md#names) | Lexer validates it; symbol and binding lookup consume its name |

Shared mechanics live in [function.rs](../compiler/src/wasm/function.rs).
`finish` writes ENTER and LEAVE around the buffered body. `publish`, `reload`,
and `release` manage linear-memory root slots. `call_instruction` snapshots roots
before allocating a result local. `branch` computes a numeric depth from the
`controls` stack, including each intervening `if`. These helpers implement ABI
and WASM rules shared by several productions.

P26 already lowers non-String equality to i32.eq. The checker currently prevents
class/null equality from reaching it; the test report identifies that dependency.
Ordinary stderr remains deferred until the runtime's print-destination ABI lands.
