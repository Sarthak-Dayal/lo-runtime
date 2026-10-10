//! Spill-everything "allocator": every virtual register lives in its own stack slot.
//!
//! It produces the same `ProgramAllocation` as linear scan, so code generation does
//! not know which producer ran. It is the reference for debugging: a native
//! divergence that disappears under spill-everything points at the allocator, not
//! at instruction selection.

use std::collections::HashMap;

use crate::ir::{CheckedIr, FunctionIr, VirtualRegId};

use super::{FunctionAllocation, PhysicalLocation, ProgramAllocation, SpillSlotId};

/// Virtual register `n` gets spill slot `n`. No registers are used, so no
/// callee-saved register needs preserving.
pub fn spill_everything(function: &FunctionIr) -> FunctionAllocation {
    let count = function.register_types.len();
    FunctionAllocation {
        register_locations: (0..count)
            .map(|n| (VirtualRegId(n), PhysicalLocation::Spill(SpillSlotId(n))))
            .collect(),
        spill_slot_count: count,
        used_callee_saved_registers: vec![],
    }
}

pub fn spill_everything_program(ir: &CheckedIr) -> ProgramAllocation {
    let function_allocations: HashMap<_, _> = ir
        .program()
        .functions
        .iter()
        .map(|function| (function.symbol, spill_everything(function)))
        .collect();
    ProgramAllocation {
        function_allocations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{BlockId, IrType, SymbolId};

    fn function(types: &[IrType]) -> FunctionIr {
        let mut function = FunctionIr {
            symbol: SymbolId(7),
            params: vec![],
            register_types: vec![],
            register_names: vec![],
            root_slots: 0,
            blocks: vec![],
            entry: BlockId(0),
        };
        for ty in types {
            function.new_register(*ty, None);
        }
        function
    }

    #[test]
    fn every_register_gets_its_own_slot() {
        let f = function(&[IrType::Int32, IrType::Ref, IrType::Bool]);
        let allocation = spill_everything(&f);
        assert_eq!(allocation.spill_slot_count, 3);
        assert!(allocation.used_callee_saved_registers.is_empty());
        for n in 0..3 {
            assert_eq!(
                allocation.register_locations[&VirtualRegId(n)],
                PhysicalLocation::Spill(SpillSlotId(n))
            );
        }
        assert_eq!(allocation.register_locations.len(), 3);
    }

    #[test]
    fn function_without_registers_needs_no_slots() {
        let allocation = spill_everything(&function(&[]));
        assert_eq!(allocation.spill_slot_count, 0);
        assert!(allocation.register_locations.is_empty());
    }
}
