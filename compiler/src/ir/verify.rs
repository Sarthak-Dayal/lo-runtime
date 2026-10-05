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

    pub(super) fn validate(&self) -> Result<(), String> {
        for signature in &self.signatures {
            for &ty in signature.params.iter().chain(signature.result.iter()) {
                self.validate_type(ty)?;
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
        Ok(())
    }

    fn validate_type(&self, ty: IrType) -> Result<(), String> {
        if let IrType::CodePtr(id) = ty {
            self.signature(id)?;
        }
        Ok(())
    }

    fn types_match(&self, actual: IrType, expected: IrType) -> Result<bool, String> {
        let mut pending = vec![(actual, expected)];
        let mut seen = std::collections::HashSet::new();
        while let Some((actual, expected)) = pending.pop() {
            self.validate_type(actual)?;
            self.validate_type(expected)?;
            match (actual, expected) {
                (IrType::CodePtr(a), IrType::CodePtr(b)) => {
                    // Signature graphs may contain recursive callable types.
                    if !seen.insert((a.0, b.0)) {
                        continue;
                    }
                    let a = self.signature(a)?;
                    let b = self.signature(b)?;
                    if a.params.len() != b.params.len() {
                        return Ok(false);
                    }
                    pending.extend(a.params.iter().copied().zip(b.params.iter().copied()));
                    match (a.result, b.result) {
                        (Some(a), Some(b)) => pending.push((a, b)),
                        (None, None) => {}
                        _ => return Ok(false),
                    }
                }
                _ if actual != expected => return Ok(false),
                _ => {}
            }
        }
        Ok(true)
    }

    fn optional_types_match(
        &self,
        actual: Option<IrType>,
        expected: Option<IrType>,
    ) -> Result<bool, String> {
        match (actual, expected) {
            (Some(a), Some(b)) => self.types_match(a, b),
            (None, None) => Ok(true),
            _ => Ok(false),
        }
    }

    fn expect(&self, actual: IrType, expected: IrType) -> Result<(), String> {
        if self.types_match(actual, expected)? {
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
                    if !self.optional_types_match(actual, sig.result)? {
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
                let input = ty(*lhs)?;
                self.expect(ty(*rhs)?, input)?;
                let result = match op {
                    BinaryOp::Eq | BinaryOp::Ne => IrType::Bool,
                    BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
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
            InstructionKind::NullCheck { receiver, .. } => self.expect(ty(*receiver)?, IrType::Ref),
            InstructionKind::Call {
                dst,
                target,
                signature,
                args,
                ..
            } => {
                let sig = self.signature(*signature)?;
                match target {
                    CallTarget::Direct(id) => {
                        if !self.types_match(
                            self.operand_type(f, Operand::Symbol(*id))?,
                            IrType::CodePtr(*signature),
                        )? {
                            return Err("call signature mismatch".into());
                        }
                    }
                    CallTarget::Indirect(pointer) => {
                        self.expect(ty(*pointer)?, IrType::CodePtr(*signature))?
                    }
                }
                if args.len() != sig.params.len() {
                    return Err("call argument count mismatch".into());
                }
                for (&arg, &expected) in args.iter().zip(&sig.params) {
                    self.expect(ty(arg)?, expected)?;
                }
                let result = dst.map(|id| value_type(f, id)).transpose()?;
                if !self.optional_types_match(result, sig.result)? {
                    return Err("call result type mismatch".into());
                }
                Ok(())
            }
        }
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
            for operand in inst.kind.operands() {
                check(operand, &assigned)?;
            }
            if let Some(dst) = inst.kind.destination() {
                assigned[dst.0] = true;
            }
        }
        match &block.terminator {
            Terminator::Branch { condition, .. } => check(*condition, &assigned)?,
            Terminator::Return(Some(value)) => check(*value, &assigned)?,
            Terminator::Abort { args, .. } => {
                for &arg in args {
                    check(arg, &assigned)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}
