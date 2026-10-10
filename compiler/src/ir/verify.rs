use super::*;
use std::collections::HashSet;

impl CheckedIr {
    pub fn program(&self) -> &ProgramIr {
        &self.program
    }

    pub fn into_program(self) -> ProgramIr {
        self.program
    }
}

impl ProgramIr {
    pub fn verify(self) -> Result<CheckedIr, String> {
        self.validate()?;
        Ok(CheckedIr { program: self })
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        self.verify_signatures()?;
        self.verify_symbols()?;

        // Function and data definitions share the same symbol namespace.
        let mut definitions = HashSet::new();
        self.verify_function_definitions(&mut definitions)?;
        self.verify_startup(&definitions)?;
        self.verify_data_definitions(&mut definitions)
    }

    fn verify_signatures(&self) -> Result<(), String> {
        for (id, signature) in self.signatures.iter().enumerate() {
            if self.signatures[..id].contains(signature) {
                return Err("duplicate signature: use intern_signature".into());
            }
            if signature
                .params
                .iter()
                .chain(signature.result.iter())
                .any(|ty| matches!(ty, IrType::CodePtr(_)))
            {
                return Err("LO/runtime signatures cannot take or return code pointers".into());
            }
        }
        Ok(())
    }

    fn verify_symbols(&self) -> Result<(), String> {
        let mut names = HashSet::new();
        for symbol in &self.symbols {
            if !names.insert(&symbol.name) {
                return Err(format!("duplicate symbol {}", symbol.name));
            }
            if let SymbolKind::Function(id) = symbol.kind {
                self.signature(id)?;
            }
        }
        Ok(())
    }

    fn verify_function_definitions(
        &self,
        definitions: &mut HashSet<SymbolId>,
    ) -> Result<(), String> {
        for function in &self.functions {
            let symbol = self.symbol(function.symbol)?;
            if !definitions.insert(function.symbol) {
                return Err(format!("duplicate function {}", symbol.name));
            }
            self.verify_function(function)
                .map_err(|e| format!("{}: {e}", symbol.name))?;
        }
        Ok(())
    }

    fn verify_startup(&self, definitions: &HashSet<SymbolId>) -> Result<(), String> {
        if let Some(startup) = self.startup {
            self.function_signature(startup)?;
            if !definitions.contains(&startup) {
                return Err("startup function has no definition".into());
            }
        }
        Ok(())
    }

    fn verify_data_definitions(&self, definitions: &mut HashSet<SymbolId>) -> Result<(), String> {
        for data in &self.data {
            if matches!(self.symbol(data.symbol)?.kind, SymbolKind::Function(_)) {
                return Err("data definition requires a data/static-reference symbol".into());
            }
            if !definitions.insert(data.symbol) {
                return Err("duplicate data definition".into());
            }
            if !data.align.is_power_of_two() {
                return Err("data alignment must be a nonzero power of two".into());
            }
            for item in &data.items {
                if let DataItem::Addr(id) = item {
                    self.symbol(*id)?;
                }
            }
        }
        Ok(())
    }

    fn validate_type(&self, ty: IrType) -> Result<(), String> {
        if let IrType::CodePtr(id) = ty {
            self.signature(id)?;
        }
        Ok(())
    }

    fn expect_type(&self, actual: IrType, expected: IrType) -> Result<(), String> {
        self.validate_type(actual)?;
        self.validate_type(expected)?;
        if actual == expected {
            Ok(())
        } else {
            Err(format!("expected {expected:?}, got {actual:?}"))
        }
    }

    fn symbol(&self, id: SymbolId) -> Result<&Symbol, String> {
        self.symbols
            .get(id.0)
            .ok_or_else(|| format!("unknown symbol {}", id.0))
    }

    fn signature(&self, id: SignatureId) -> Result<&Signature, String> {
        self.signatures
            .get(id.0)
            .ok_or_else(|| format!("unknown signature {}", id.0))
    }

    fn function_signature(&self, id: SymbolId) -> Result<&Signature, String> {
        match self.symbol(id)?.kind {
            SymbolKind::Function(signature) => self.signature(signature),
            _ => Err("expected a function symbol".into()),
        }
    }

    fn operand_type(&self, function: &FunctionIr, operand: Operand) -> Result<IrType, String> {
        match operand {
            Operand::Value(id) => value_type(function, id),
            Operand::Int(_) => Ok(IrType::Int32),
            Operand::Bool(_) => Ok(IrType::Bool),
            Operand::Null => Ok(IrType::Ref),
            Operand::Symbol(id) => Ok(match self.symbol(id)?.kind {
                SymbolKind::Function(id) => IrType::CodePtr(id),
                SymbolKind::Data => IrType::Ptr,
                SymbolKind::StaticRef => IrType::Ref,
            }),
        }
    }

    fn verify_function(&self, function: &FunctionIr) -> Result<(), String> {
        // Validate all IDs and types, including in unreachable blocks, before
        // definite-assignment analysis indexes directly into the IR tables.
        self.verify_structure_and_types(function)?;
        verify_definite_assignment(function)
    }

    fn verify_structure_and_types(&self, function: &FunctionIr) -> Result<(), String> {
        if function.register_names.len() != function.register_types.len() {
            return Err("register_names must match register_types length".into());
        }
        for &ty in &function.register_types {
            self.validate_type(ty)?;
        }
        let signature = self.function_signature(function.symbol)?;
        if function.entry.0 >= function.blocks.len() {
            return Err("invalid entry block".into());
        }
        self.verify_parameters(function, signature)?;
        for (index, block) in function.blocks.iter().enumerate() {
            self.verify_block(function, block, signature)
                .map_err(|error| format!("b{index}: {error}"))?;
        }
        Ok(())
    }

    fn verify_parameters(
        &self,
        function: &FunctionIr,
        signature: &Signature,
    ) -> Result<(), String> {
        if function.params.len() != signature.params.len() {
            return Err(format!(
                "parameter count mismatch: expected {}, got {}",
                signature.params.len(),
                function.params.len()
            ));
        }
        let mut params = HashSet::new();
        for (&id, &ty) in function.params.iter().zip(&signature.params) {
            if !params.insert(id) {
                return Err("duplicate parameter value".into());
            }
            self.expect_type(value_type(function, id)?, ty)?;
        }
        Ok(())
    }

    fn verify_block(
        &self,
        function: &FunctionIr,
        block: &BasicBlock,
        signature: &Signature,
    ) -> Result<(), String> {
        for successor in block.terminator.successors() {
            if successor.0 >= function.blocks.len() {
                return Err(format!("invalid target b{}", successor.0));
            }
        }
        for instruction in &block.instructions {
            self.verify_instruction(function, &instruction.kind)
                .map_err(|error| format!("line {}: {error}", instruction.line))?;
        }
        self.verify_terminator(function, &block.terminator, signature)
    }

    fn verify_terminator(
        &self,
        function: &FunctionIr,
        terminator: &Terminator,
        signature: &Signature,
    ) -> Result<(), String> {
        match terminator {
            Terminator::Jump(_) => Ok(()),
            Terminator::Branch { condition, .. } => {
                self.expect_type(self.operand_type(function, *condition)?, IrType::Bool)
            }
            Terminator::Return(value) => {
                let actual = value
                    .map(|operand| self.operand_type(function, operand))
                    .transpose()?;
                if actual != signature.result {
                    return Err(format!(
                        "return type mismatch: expected {:?}, got {actual:?}",
                        signature.result
                    ));
                }
                Ok(())
            }
            Terminator::Abort { target, args } => {
                let helper_signature = self.function_signature(*target)?;
                if helper_signature.result.is_some() {
                    return Err("abort helper must not return a value".into());
                }
                self.verify_arguments(function, args, helper_signature, "abort")
            }
        }
    }

    fn verify_arguments(
        &self,
        function: &FunctionIr,
        args: &[Operand],
        signature: &Signature,
        operation: &str,
    ) -> Result<(), String> {
        if args.len() != signature.params.len() {
            return Err(format!(
                "{operation} argument count mismatch: expected {}, got {}",
                signature.params.len(),
                args.len()
            ));
        }
        for (index, (&arg, &expected)) in args.iter().zip(&signature.params).enumerate() {
            let actual = self.operand_type(function, arg)?;
            self.expect_type(actual, expected)
                .map_err(|error| format!("{operation} argument {}: {error}", index + 1))?;
        }
        Ok(())
    }

    fn verify_instruction(
        &self,
        function: &FunctionIr,
        instruction: &InstructionKind,
    ) -> Result<(), String> {
        let operand_type = |operand| self.operand_type(function, operand);
        match instruction {
            InstructionKind::Copy { dst, src } => {
                self.expect_type(operand_type(*src)?, value_type(function, *dst)?)
            }
            InstructionKind::Binary { dst, op, lhs, rhs } => {
                if matches!(op, BinaryOp::Div | BinaryOp::Mod)
                    && matches!(rhs, Operand::Int(0 | -1))
                {
                    return Err("Div/Mod special-case divisor must be lowered to branches".into());
                }
                let input = operand_type(*lhs)?;
                self.expect_type(operand_type(*rhs)?, input)?;
                let result = match op {
                    BinaryOp::Eq => IrType::Bool,
                    BinaryOp::Lt | BinaryOp::Gt => {
                        self.expect_type(input, IrType::Int32)?;
                        IrType::Bool
                    }
                    BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Mod => {
                        self.expect_type(input, IrType::Int32)?;
                        IrType::Int32
                    }
                };
                self.expect_type(value_type(function, *dst)?, result)
            }
            InstructionKind::Unary { dst, op, src } => {
                let expected = match op {
                    UnaryOp::Neg => IrType::Int32,
                    UnaryOp::Not => IrType::Bool,
                };
                self.expect_type(operand_type(*src)?, expected)?;
                self.expect_type(value_type(function, *dst)?, expected)
            }
            InstructionKind::Load { dst, base, .. } => {
                require_address_type(operand_type(*base)?)?;
                value_type(function, *dst)?;
                Ok(())
            }
            InstructionKind::Store {
                base,
                value,
                ty: expected,
                ..
            } => {
                require_address_type(operand_type(*base)?)?;
                // Static objects cannot move; their direct stores need no barrier (L07, p.18).
                let static_ref = match value {
                    Operand::Symbol(id) => matches!(self.symbol(*id)?.kind, SymbolKind::StaticRef),
                    _ => false,
                };
                // Write permission is independent of the GC barrier exemption.
                // A direct symbolic destination lets us check its declared section;
                // register-based addresses do not establish static provenance.
                let static_base = match base {
                    Operand::Symbol(id) => {
                        let symbol = self.symbol(*id)?;
                        let data = self.data.iter().find(|data| data.symbol == *id);
                        if data.is_some_and(|data| data.section == Section::ReadOnly) {
                            return Err(format!("store to read-only data {}", symbol.name));
                        }
                        matches!(symbol.kind, SymbolKind::Data)
                            && data.is_some_and(|data| data.section == Section::Writable)
                    }
                    _ => false,
                };
                if *expected == IrType::Ref && !static_ref && !static_base {
                    return Err("reference stores require a write-barrier call".into());
                }
                self.expect_type(operand_type(*value)?, *expected)
            }
            InstructionKind::RootStore { slot, value } => {
                verify_root_slot(function, *slot)?;
                self.expect_type(operand_type(*value)?, IrType::Ref)
            }
            InstructionKind::RootLoad { dst, slot } => {
                verify_root_slot(function, *slot)?;
                self.expect_type(value_type(function, *dst)?, IrType::Ref)
            }
            InstructionKind::Call { dst, target, args } => {
                let signature = match target {
                    CallTarget::Direct(id) => self.function_signature(*id)?,
                    CallTarget::Indirect(pointer) => match operand_type(*pointer)? {
                        IrType::CodePtr(id) => self.signature(id)?,
                        _ => return Err("indirect call requires a CodePtr".into()),
                    },
                };
                self.verify_arguments(function, args, signature, "call")?;
                let result = dst.map(|id| value_type(function, id)).transpose()?;
                if result != signature.result {
                    return Err(format!(
                        "call result type mismatch: expected {:?}, got {result:?}",
                        signature.result
                    ));
                }
                Ok(())
            }
        }
    }
}

fn verify_root_slot(function: &FunctionIr, slot: u32) -> Result<(), String> {
    if slot < function.root_slots {
        Ok(())
    } else {
        Err(format!("invalid root slot {slot}"))
    }
}

fn value_type(function: &FunctionIr, id: VirtualRegId) -> Result<IrType, String> {
    function
        .register_types
        .get(id.0)
        .copied()
        .ok_or_else(|| format!("unknown value v{}", id.0))
}

fn require_address_type(ty: IrType) -> Result<(), String> {
    if matches!(ty, IrType::Ref | IrType::Ptr) {
        Ok(())
    } else {
        Err(format!(
            "memory base is not an address: expected Ref or Ptr, got {ty:?}"
        ))
    }
}

// A mutable register must be assigned on every incoming path before it is read.
// Precondition: structure and type verification has validated the entry block,
// successor blocks, parameters, and every operand and destination register ID.
fn verify_definite_assignment(function: &FunctionIr) -> Result<(), String> {
    let control_flow = discover_reachable_control_flow(function);
    let assigned_at_entry = compute_definite_assignments(function, &control_flow);

    // Check reads only after convergence: the initial, optimistic assignment
    // sets can still contain registers that an incoming path never assigns.
    verify_reads(function, &control_flow, &assigned_at_entry)
}

struct ReachableControlFlow {
    reachable: Vec<bool>,
    predecessors: Vec<Vec<usize>>,
}

fn discover_reachable_control_flow(function: &FunctionIr) -> ReachableControlFlow {
    let mut reachable = vec![false; function.blocks.len()];
    let mut pending = vec![function.entry];
    let mut predecessors = vec![vec![]; function.blocks.len()];
    while let Some(block) = pending.pop() {
        if reachable[block.0] {
            continue;
        }
        reachable[block.0] = true;
        for next in function.blocks[block.0].terminator.successors() {
            // Only reachable predecessors constrain definite assignment.
            predecessors[next.0].push(block.0);
            pending.push(next);
        }
    }
    ReachableControlFlow {
        reachable,
        predecessors,
    }
}

fn compute_definite_assignments(
    function: &FunctionIr,
    control_flow: &ReachableControlFlow,
) -> Vec<Vec<bool>> {
    let register_count = function.register_types.len();
    let mut parameter_assignments = vec![false; register_count];
    for id in &function.params {
        parameter_assignments[id.0] = true;
    }

    // Start optimistically with every register assigned. Intersecting incoming
    // paths removes assignments that are not guaranteed. This initialization
    // lets loops retain assignments established before entering the loop.
    let mut assigned_at_entry = vec![vec![true; register_count]; function.blocks.len()];
    let mut assigned_at_exit = assigned_at_entry.clone();
    loop {
        let mut changed = false;
        for (block_index, block) in function.blocks.iter().enumerate() {
            if !control_flow.reachable[block_index] {
                continue;
            }
            let mut entry_assignments = if block_index == function.entry.0 {
                // The first invocation reaches entry with only parameters
                // assigned; a back edge cannot initialize that first visit.
                parameter_assignments.clone()
            } else {
                vec![true; register_count]
            };
            for &predecessor in &control_flow.predecessors[block_index] {
                for (register_index, assigned) in entry_assignments.iter_mut().enumerate() {
                    *assigned &= assigned_at_exit[predecessor][register_index];
                }
            }
            let mut exit_assignments = entry_assignments.clone();
            for instruction in &block.instructions {
                if let Some(dst) = instruction.kind.destination() {
                    exit_assignments[dst.0] = true;
                }
            }
            changed |= assigned_at_entry[block_index] != entry_assignments
                || assigned_at_exit[block_index] != exit_assignments;
            assigned_at_entry[block_index] = entry_assignments;
            assigned_at_exit[block_index] = exit_assignments;
        }
        if !changed {
            break;
        }
    }
    assigned_at_entry
}

fn verify_reads(
    function: &FunctionIr,
    control_flow: &ReachableControlFlow,
    assigned_at_entry: &[Vec<bool>],
) -> Result<(), String> {
    for (block_index, block) in function.blocks.iter().enumerate() {
        if !control_flow.reachable[block_index] {
            continue;
        }
        let mut assigned = assigned_at_entry[block_index].clone();
        for instruction in &block.instructions {
            verify_assigned_operands(&instruction.kind.operands(), &assigned)
                .map_err(|error| format!("b{block_index}: line {}: {error}", instruction.line))?;
            // Check reads before marking the destination, so x = x + 1
            // still requires a previous assignment to x.
            if let Some(dst) = instruction.kind.destination() {
                assigned[dst.0] = true;
            }
        }
        verify_assigned_operands(&block.terminator.operands(), &assigned)
            .map_err(|error| format!("b{block_index}: {error}"))?;
    }
    Ok(())
}

fn verify_assigned_operands(operands: &[Operand], assigned: &[bool]) -> Result<(), String> {
    for operand in operands {
        if let Operand::Value(id) = operand {
            if !assigned[id.0] {
                return Err(format!("v{} used before assignment", id.0));
            }
        }
    }
    Ok(())
}
