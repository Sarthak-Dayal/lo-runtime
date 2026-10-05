use super::*;

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
        let mut names = std::collections::HashSet::new();
        for symbol in &self.symbols {
            if !names.insert(&symbol.name) {
                return Err(format!("duplicate symbol {}", symbol.name));
            }
            if let SymbolKind::Function(id) = symbol.kind {
                self.signature(id)?;
            }
        }
        let mut definitions = std::collections::HashSet::new();
        for function in &self.functions {
            let symbol = self.symbol(function.symbol)?;
            if !definitions.insert(function.symbol.0) {
                return Err(format!("duplicate function {}", symbol.name));
            }
            self.verify_function(function)
                .map_err(|e| format!("{}: {e}", symbol.name))?;
        }
        if let Some(startup) = self.startup {
            self.function_signature(startup)?;
            if !definitions.contains(&startup.0) {
                return Err("startup function has no definition".into());
            }
        }
        for data in &self.data {
            if matches!(self.symbol(data.symbol)?.kind, SymbolKind::Function(_)) {
                return Err("data definition requires a data/static-reference symbol".into());
            }
            if !definitions.insert(data.symbol.0) {
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

    fn expect(&self, actual: IrType, expected: IrType) -> Result<(), String> {
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

    fn operand_type(&self, f: &FunctionIr, operand: Operand) -> Result<IrType, String> {
        match operand {
            Operand::Value(id) => value_type(f, id),
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

    fn verify_function(&self, f: &FunctionIr) -> Result<(), String> {
        if f.value_names.len() != f.value_types.len() {
            return Err("value_names must match value_types length".into());
        }
        for &ty in &f.value_types {
            self.validate_type(ty)?;
        }
        let sig = self.function_signature(f.symbol)?;
        if f.entry.0 >= f.blocks.len() {
            return Err("invalid entry block".into());
        }
        if f.params.len() != sig.params.len() {
            return Err("parameter count mismatch".into());
        }
        let mut params = std::collections::HashSet::new();
        for (&id, &ty) in f.params.iter().zip(&sig.params) {
            if !params.insert(id.0) {
                return Err("duplicate parameter value".into());
            }
            self.expect(value_type(f, id)?, ty)?;
        }
        for (index, block) in f.blocks.iter().enumerate() {
            for successor in block.terminator.successors() {
                if successor.0 >= f.blocks.len() {
                    return Err(format!("b{index}: invalid target b{}", successor.0));
                }
            }
            for inst in &block.instructions {
                self.verify_instruction(f, &inst.kind)
                    .map_err(|e| format!("b{index}, line {}: {e}", inst.line))?;
            }
            match &block.terminator {
                Terminator::Branch { condition, .. } => {
                    self.expect(self.operand_type(f, *condition)?, IrType::Bool)?
                }
                Terminator::Return(value) => {
                    let actual = value.map(|v| self.operand_type(f, v)).transpose()?;
                    if actual != sig.result {
                        return Err(format!("b{index}: return type mismatch"));
                    }
                }
                Terminator::Abort { target, args } => {
                    let sig = self.function_signature(*target)?;
                    if sig.result.is_some() {
                        return Err("abort helper must not return a value".into());
                    }
                    if args.len() != sig.params.len() {
                        return Err("abort argument count mismatch".into());
                    }
                    for (&arg, &expected) in args.iter().zip(&sig.params) {
                        self.expect(self.operand_type(f, arg)?, expected)?;
                    }
                }
                _ => {}
            }
        }
        verify_assignment(f)
    }

    fn verify_instruction(&self, f: &FunctionIr, inst: &InstructionKind) -> Result<(), String> {
        let ty = |operand| self.operand_type(f, operand);
        match inst {
            InstructionKind::Copy { dst, src } => self.expect(ty(*src)?, value_type(f, *dst)?),
            InstructionKind::Binary { dst, op, lhs, rhs } => {
                if matches!(op, BinaryOp::Div | BinaryOp::Mod)
                    && matches!(rhs, Operand::Int(0 | -1))
                {
                    return Err("Div/Mod special-case divisor must be lowered to branches".into());
                }
                let input = ty(*lhs)?;
                self.expect(ty(*rhs)?, input)?;
                let result = match op {
                    BinaryOp::Eq => IrType::Bool,
                    BinaryOp::Lt | BinaryOp::Gt => {
                        self.expect(input, IrType::Int32)?;
                        IrType::Bool
                    }
                    _ => {
                        self.expect(input, IrType::Int32)?;
                        IrType::Int32
                    }
                };
                self.expect(value_type(f, *dst)?, result)
            }
            InstructionKind::Unary { dst, op, src } => {
                let expected = match op {
                    UnaryOp::Neg => IrType::Int32,
                    UnaryOp::Not => IrType::Bool,
                };
                self.expect(ty(*src)?, expected)?;
                self.expect(value_type(f, *dst)?, expected)
            }
            InstructionKind::Load { dst, base, .. } => {
                address(ty(*base)?)?;
                value_type(f, *dst)?;
                Ok(())
            }
            InstructionKind::Store {
                base,
                value,
                ty: expected,
                ..
            } => {
                address(ty(*base)?)?;
                // Static objects cannot move; their direct stores need no barrier (L07, p.18).
                let static_ref = match value {
                    Operand::Symbol(id) => matches!(self.symbol(*id)?.kind, SymbolKind::StaticRef),
                    _ => false,
                };
                if *expected == IrType::Ref && !static_ref {
                    return Err("reference stores require a write-barrier call".into());
                }
                self.expect(ty(*value)?, *expected)
            }
            InstructionKind::RootStore { slot, value } => {
                root_slot(f, *slot)?;
                self.expect(ty(*value)?, IrType::Ref)
            }
            InstructionKind::RootLoad { dst, slot } => {
                root_slot(f, *slot)?;
                self.expect(value_type(f, *dst)?, IrType::Ref)
            }
            InstructionKind::RootAddr { dst, slot } => {
                root_slot(f, *slot)?;
                if self.startup != Some(f.symbol) {
                    return Err("RootAddr is only permitted in the startup function".into());
                }
                self.expect(value_type(f, *dst)?, IrType::Ptr)
            }
            InstructionKind::Call { dst, target, args } => {
                let sig = match target {
                    CallTarget::Direct(id) => self.function_signature(*id)?,
                    CallTarget::Indirect(pointer) => match ty(*pointer)? {
                        IrType::CodePtr(id) => self.signature(id)?,
                        _ => return Err("indirect call requires a CodePtr".into()),
                    },
                };
                if args.len() != sig.params.len() {
                    return Err("call argument count mismatch".into());
                }
                for (&arg, &expected) in args.iter().zip(&sig.params) {
                    self.expect(ty(arg)?, expected)?;
                }
                let result = dst.map(|id| value_type(f, id)).transpose()?;
                if result != sig.result {
                    return Err("call result type mismatch".into());
                }
                Ok(())
            }
        }
    }
}

fn root_slot(f: &FunctionIr, slot: u32) -> Result<(), String> {
    if slot < f.root_slots {
        Ok(())
    } else {
        Err(format!("invalid root slot {slot}"))
    }
}

fn value_type(f: &FunctionIr, id: ValueId) -> Result<IrType, String> {
    f.value_types
        .get(id.0)
        .copied()
        .ok_or_else(|| format!("unknown value v{}", id.0))
}
fn address(ty: IrType) -> Result<(), String> {
    if matches!(ty, IrType::Ref | IrType::Ptr) {
        Ok(())
    } else {
        Err("memory base is not an address".into())
    }
}

// Must-analysis: a mutable value must be initialized on every incoming path.
fn verify_assignment(f: &FunctionIr) -> Result<(), String> {
    let n = f.blocks.len();
    let mut reachable = vec![false; n];
    let mut pending = vec![f.entry];
    let mut predecessors = vec![vec![]; n];
    while let Some(block) = pending.pop() {
        if reachable[block.0] {
            continue;
        }
        reachable[block.0] = true;
        for next in f.blocks[block.0].terminator.successors() {
            predecessors[next.0].push(block.0);
            pending.push(next);
        }
    }
    let mut initial = vec![false; f.value_types.len()];
    for id in &f.params {
        initial[id.0] = true;
    }
    let mut inputs = vec![vec![true; initial.len()]; n];
    let mut outputs = inputs.clone();
    loop {
        let mut changed = false;
        for (b, block) in f.blocks.iter().enumerate() {
            if !reachable[b] {
                continue;
            }
            let mut input = if b == f.entry.0 {
                initial.clone()
            } else {
                vec![true; initial.len()]
            };
            for &pred in &predecessors[b] {
                for (v, assigned) in input.iter_mut().enumerate() {
                    *assigned &= outputs[pred][v];
                }
            }
            let mut output = input.clone();
            for inst in &block.instructions {
                if let Some(dst) = inst.kind.destination() {
                    output[dst.0] = true;
                }
            }
            changed |= inputs[b] != input || outputs[b] != output;
            inputs[b] = input;
            outputs[b] = output;
        }
        if !changed {
            break;
        }
    }
    for (b, block) in f.blocks.iter().enumerate() {
        if !reachable[b] {
            continue;
        }
        let mut assigned = inputs[b].clone();
        let check = |operand: Operand, assigned: &[bool]| {
            if let Operand::Value(id) = operand {
                if !assigned[id.0] {
                    return Err(format!("b{b}: v{} used before assignment", id.0));
                }
            }
            Ok(())
        };
        for inst in &block.instructions {
            for operand in inst.kind.uses() {
                check(operand, &assigned)?;
            }
            if let Some(dst) = inst.kind.destination() {
                assigned[dst.0] = true;
            }
        }
        for operand in block.terminator.uses() {
            check(operand, &assigned)?;
        }
    }
    Ok(())
}
