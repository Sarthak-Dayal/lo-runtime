use crate::symbols;

use super::*;
use IrType::{Bool, Int32, Ptr, Ref};

pub const FUNCTIONS: &[RuntimeFunction] = &[
    RuntimeFunction::new("lo_runtime_init", &[], None),
    RuntimeFunction::new("lo_alloc", &[Ptr], Some(Ref)),
    RuntimeFunction::new("lo_push_frame", &[Ptr], None),
    RuntimeFunction::new("lo_pop_frame", &[], None),
    RuntimeFunction::new("lo_gc_write_barrier", &[Ref, Int32, Ref], None),
    RuntimeFunction::new("lo_string_new", &[Ptr, Int32], Some(Ref)),
    RuntimeFunction::new("lo_string_concat", &[Ref, Ref], Some(Ref)),
    RuntimeFunction::new("lo_string_repeat", &[Ref, Int32], Some(Ref)),
    RuntimeFunction::new("lo_string_compare", &[Ref, Ref], Some(Int32)),
    RuntimeFunction::new("lo_string_reverse", &[Ref], Some(Ref)),
    RuntimeFunction::new("lo_cast_check", &[Ref, Ptr], Some(Ref)),
    RuntimeFunction::new("lo_instanceof", &[Ref, Ptr], Some(Bool)),
    RuntimeFunction::new("lo_print_int", &[Int32, Int32], None),
    RuntimeFunction::new("lo_print_bool", &[Bool, Int32], None),
    RuntimeFunction::new("lo_print_string", &[Ref, Int32], None),
    RuntimeFunction::new("lo_println", &[Int32], None),
    RuntimeFunction::new("lo_read_int", &[], Some(Int32)),
    RuntimeFunction::new("lo_read_bool", &[], Some(Bool)),
    RuntimeFunction::new("lo_read_string", &[], Some(Ref)),
    RuntimeFunction::new("lo_eof", &[], Some(Bool)),
    RuntimeFunction::new("lo_abort_null_receiver", &[Ptr, Int32], None),
];

pub struct RuntimeFunction {
    pub name: &'static str,
    pub params: &'static [IrType],
    pub result: Option<IrType>,
}

impl RuntimeFunction {
    const fn new(name: &'static str, params: &'static [IrType], result: Option<IrType>) -> Self {
        RuntimeFunction {
            name,
            params,
            result,
        }
    }
}

impl ProgramIr {
    pub fn runtime_function(&mut self, name: &str) -> Result<SymbolId, String> {
        let function = FUNCTIONS
            .iter()
            .find(|function| function.name == name)
            .ok_or_else(|| format!("{name} is not a runtime function"))?;
        self.declare_callable(name, function.params.to_vec(), function.result)
    }

    pub fn empty_string(&mut self) -> Result<SymbolId, String> {
        self.declare_symbol(symbols::EMPTY_STRING, SymbolKind::StaticRef)
    }

    pub fn string_class(&mut self) -> Result<SymbolId, String> {
        self.declare_symbol(symbols::STRING_CLASS, SymbolKind::Data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_functions_are_declared_once_with_their_signature() {
        let mut program = ProgramIr {
            symbols: vec![],
            signatures: vec![],
            functions: vec![],
            data: vec![],
            startup: None,
        };
        let alloc = program.runtime_function("lo_alloc").unwrap();
        assert_eq!(program.runtime_function("lo_alloc").unwrap(), alloc);
        let SymbolKind::Function(sig) = program.symbols[alloc.0].kind else {
            panic!("lo_alloc is not a function")
        };
        assert_eq!(
            program.signatures[sig.0],
            Signature {
                params: vec![Ptr],
                result: Some(Ref)
            }
        );
        assert!(program.runtime_function("lo_missing").is_err());
        assert!(program
            .declare_symbol("lo_alloc", SymbolKind::Data)
            .is_err());
    }
}
