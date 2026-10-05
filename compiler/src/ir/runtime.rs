//! The runtime library's entry points, typed for the IR (`runtime-abi.md` §3).
//!
//! The runtime defines these; codegen only declares them. A declared function
//! with no `FunctionIr` is external, and the linker resolves it against
//! `liblo_runtime.a`. The parameter types matter on x86-64: a `Ptr`/`Ref`
//! argument travels in a 64-bit register (`rdi`), an `Int32`/`Bool` in its
//! 32-bit half (`edi`).

use crate::symbols;

use super::*;

pub struct RuntimeFunction {
    pub name: &'static str,
    pub params: &'static [IrType],
    pub result: Option<IrType>,
}

use IrType::{Bool, Int32, Ptr, Ref};

const fn function(
    name: &'static str,
    params: &'static [IrType],
    result: Option<IrType>,
) -> RuntimeFunction {
    RuntimeFunction {
        name,
        params,
        result,
    }
}

/// Every runtime function codegen calls. A descriptor or byte-literal argument
/// is a raw `Ptr`; managed objects are `Ref`. ABI `u32` lengths and offsets are
/// `Int32`. `lo_abort_null_receiver` never returns; it is only ever the target
/// of an `Abort` terminator.
pub const FUNCTIONS: &[RuntimeFunction] = &[
    // §3.6 initialization
    function("lo_runtime_init", &[], None),
    // §3.1 allocation: the class descriptor → a zero-filled object
    function("lo_alloc", &[Ptr], Some(Ref)),
    // §3.3 shadow stack: the frame's address
    function("lo_push_frame", &[Ptr], None),
    function("lo_pop_frame", &[], None),
    // §3.4 write barrier: object, field offset, new value (it performs the store)
    function("lo_gc_write_barrier", &[Ref, Int32, Ref], None),
    // §3.2 strings
    function("lo_string_new", &[Ptr, Int32], Some(Ref)),
    function("lo_string_concat", &[Ref, Ref], Some(Ref)),
    function("lo_string_repeat", &[Ref, Int32], Some(Ref)),
    function("lo_string_compare", &[Ref, Ref], Some(Int32)),
    function("lo_string_reverse", &[Ref], Some(Ref)),
    // §3.5 casts: object, target class descriptor
    function("lo_cast_check", &[Ref, Ptr], Some(Ref)),
    function("lo_instanceof", &[Ref, Ptr], Some(Bool)),
    // §3.7 I/O: every print takes a trailing to_stderr selector (0 or 1)
    function("lo_print_int", &[Int32, Int32], None),
    function("lo_print_bool", &[Bool, Int32], None),
    function("lo_print_string", &[Ref, Int32], None),
    function("lo_println", &[Int32], None),
    function("lo_read_int", &[], Some(Int32)),
    function("lo_read_bool", &[], Some(Bool)),
    function("lo_read_string", &[], Some(Ref)),
    function("lo_eof", &[], Some(Bool)),
    // §3.8 null receiver: the method name's bytes and length
    function("lo_abort_null_receiver", &[Ptr, Int32], None),
];

impl ProgramIr {
    /// Declares the runtime function `name` and returns its symbol.
    pub fn runtime_function(&mut self, name: &str) -> Result<SymbolId, String> {
        let function = FUNCTIONS
            .iter()
            .find(|function| function.name == name)
            .ok_or_else(|| format!("{name} is not a runtime function"))?;
        self.declare_function(name, function.params.to_vec(), function.result)
    }

    /// The runtime's empty-string object. It lives outside the heap but is a
    /// real String, so it is a `Ref` wherever it is used.
    pub fn empty_string(&mut self) -> Result<SymbolId, String> {
        self.declare_symbol(symbols::EMPTY_STRING, SymbolKind::StaticRef)
    }

    /// The runtime's descriptor for `String`, for casts and `instanceof`.
    pub fn string_class(&mut self) -> Result<SymbolId, String> {
        self.declare_symbol(symbols::STRING_CLASS, SymbolKind::Data)
    }
}
