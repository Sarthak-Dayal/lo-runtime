//! Declaring symbols. Declaring an existing name returns its existing ID, so
//! lowering and the static data share one symbol per name.

use crate::ast::Type;
use crate::symbols;
use crate::type_checker::{ConstructorSig, MethodSig};

use super::*;

impl ProgramIr {
    pub fn find_symbol(&self, name: &str) -> Option<SymbolId> {
        self.symbols
            .iter()
            .position(|symbol| symbol.name == name)
            .map(SymbolId)
    }

    pub fn declare_symbol(&mut self, name: &str, kind: SymbolKind) -> Result<SymbolId, String> {
        if let Some(id) = self.find_symbol(name) {
            let existing = self.symbols[id.0].kind;
            if !Self::same_kind(existing, kind) {
                return Err(format!(
                    "symbol {name} redeclared as {kind:?}, already {existing:?}"
                ));
            }
            return Ok(id);
        }
        self.symbols.push(Symbol {
            name: name.to_string(),
            kind,
        });
        Ok(SymbolId(self.symbols.len() - 1))
    }

    /// Any code symbol: a method, a constructor, or a runtime entry point.
    pub fn declare_callable(
        &mut self,
        name: &str,
        params: Vec<IrType>,
        result: Option<IrType>,
    ) -> Result<SymbolId, String> {
        let signature = self.intern_signature(Signature { params, result });
        self.declare_symbol(name, SymbolKind::Function(signature))
    }

    pub fn declare_method(&mut self, class: &str, sig: &MethodSig) -> Result<SymbolId, String> {
        self.declare_callable(
            &symbols::method(class, &sig.method_name),
            Self::receiver_then(&sig.params),
            ir_type(&sig.return_type),
        )
    }

    pub fn declare_constructor(
        &mut self,
        class: &str,
        ctor: &ConstructorSig,
    ) -> Result<SymbolId, String> {
        self.declare_callable(
            &symbols::constructor(class, ctor.arity),
            Self::receiver_then(&ctor.params),
            None,
        )
    }

    fn receiver_then(params: &[Type]) -> Vec<IrType> {
        std::iter::once(IrType::Ref)
            .chain(params.iter().map(|ty| ir_type(ty).expect("void parameter")))
            .collect()
    }

    fn same_kind(a: SymbolKind, b: SymbolKind) -> bool {
        match (a, b) {
            (SymbolKind::Function(x), SymbolKind::Function(y)) => x == y,
            (SymbolKind::Data, SymbolKind::Data)
            | (SymbolKind::StaticRef, SymbolKind::StaticRef) => true,
            _ => false,
        }
    }
}

/// The IR type of an LO value; `None` for `void`.
pub fn ir_type(ty: &Type) -> Option<IrType> {
    match ty {
        Type::Int => Some(IrType::Int32),
        Type::Bool => Some(IrType::Bool),
        Type::String | Type::Class(_) => Some(IrType::Ref),
        Type::Void => None,
    }
}
