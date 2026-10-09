use super::*;
use std::collections::HashMap;
use std::fmt::Write;

impl ProgramIr {
    /// Produces a readable dump, including placeholders for malformed IR.
    pub fn dump(&self) -> String {
        let mut output = String::new();
        self.write_externals(&mut output);
        self.write_data_definitions(&mut output);
        self.write_functions(&mut output);
        output
    }

    fn write_externals(&self, output: &mut String) {
        for (index, symbol) in self.symbols.iter().enumerate() {
            if self.has_definition(SymbolId(index)) {
                continue;
            }
            match symbol.kind {
                SymbolKind::Function(signature) => writeln!(
                    output,
                    "extern func @{}{}",
                    symbol.name,
                    self.signature_text(signature)
                )
                .unwrap(),
                SymbolKind::Data => writeln!(output, "extern data @{}", symbol.name).unwrap(),
                SymbolKind::StaticRef => writeln!(output, "extern ref @{}", symbol.name).unwrap(),
            }
        }
    }

    fn has_definition(&self, symbol: SymbolId) -> bool {
        self.functions
            .iter()
            .any(|function| function.symbol == symbol)
            || self.data.iter().any(|data| data.symbol == symbol)
    }

    fn write_data_definitions(&self, output: &mut String) {
        for data in &self.data {
            self.write_data_definition(output, data);
        }
    }

    fn write_data_definition(&self, output: &mut String, data: &DataDef) {
        let section = match data.section {
            Section::ReadOnly => ".rodata",
            Section::Writable => ".data",
        };
        writeln!(
            output,
            "{section} {} align {} {{",
            self.symbol_text(data.symbol),
            data.align
        )
        .unwrap();
        for item in &data.items {
            self.write_data_item(output, item);
        }
        output.push_str("}\n");
    }

    fn write_data_item(&self, output: &mut String, item: &DataItem) {
        match item {
            DataItem::U32(value) => writeln!(output, "  u32 {value}").unwrap(),
            DataItem::Addr(symbol) => {
                writeln!(output, "  addr {}", self.symbol_text(*symbol)).unwrap()
            }
            DataItem::Zero(size) => writeln!(output, "  zero {size}").unwrap(),
            DataItem::Bytes(bytes) => {
                output.push_str("  bytes");
                for byte in bytes {
                    write!(output, " {byte:02x}").unwrap();
                }
                output.push('\n');
            }
        }
    }

    fn write_functions(&self, output: &mut String) {
        for function in &self.functions {
            FunctionDump::new(self, function).write_function(output);
        }
    }

    fn symbol_text(&self, id: SymbolId) -> String {
        self.symbols
            .get(id.0)
            .map(|symbol| format!("@{}", symbol.name))
            .unwrap_or_else(|| format!("@<invalid:{}>", id.0))
    }

    fn function_result_text(&self, symbol: SymbolId) -> String {
        match self.symbols.get(symbol.0).map(|symbol| symbol.kind) {
            Some(SymbolKind::Function(signature)) => self
                .signatures
                .get(signature.0)
                .map(|signature| self.result_text(signature.result))
                .unwrap_or_else(|| "<invalid signature>".into()),
            _ => "<invalid function>".into(),
        }
    }

    fn signature_text(&self, id: SignatureId) -> String {
        match self.signatures.get(id.0) {
            Some(signature) => {
                let params = comma_separated(
                    signature
                        .params
                        .iter()
                        .map(|ty| self.signature_value_type_text(*ty)),
                );
                let result = signature
                    .result
                    .map(|ty| self.signature_value_type_text(ty))
                    .unwrap_or_else(|| "Void".into());
                format!("({params}) -> {result}")
            }
            None => format!("<invalid signature:{}>", id.0),
        }
    }

    fn signature_value_type_text(&self, ty: IrType) -> String {
        // Signatures cannot contain code pointers. Stop here even for malformed
        // IR so a signature that refers to itself cannot recurse indefinitely.
        match ty {
            IrType::CodePtr(_) => "<invalid nested CodePtr>".into(),
            IrType::Int32 | IrType::Bool | IrType::Ref | IrType::Ptr => self.type_text(ty),
        }
    }

    fn type_text(&self, ty: IrType) -> String {
        match ty {
            IrType::Int32 => "Int32".into(),
            IrType::Bool => "Bool".into(),
            IrType::Ref => "Ref".into(),
            IrType::Ptr => "Ptr".into(),
            IrType::CodePtr(signature) => format!("CodePtr{}", self.signature_text(signature)),
        }
    }

    fn result_text(&self, ty: Option<IrType>) -> String {
        ty.map(|ty| self.type_text(ty))
            .unwrap_or_else(|| "Void".into())
    }
}

struct FunctionDump<'a> {
    program: &'a ProgramIr,
    function: &'a FunctionIr,
    register_names: Vec<String>,
}

impl<'a> FunctionDump<'a> {
    fn new(program: &'a ProgramIr, function: &'a FunctionIr) -> Self {
        Self {
            program,
            function,
            register_names: register_display_names(function),
        }
    }

    fn write_function(&self, output: &mut String) {
        self.write_header(output);
        for (index, block) in self.function.blocks.iter().enumerate() {
            self.write_block(output, BlockId(index), block);
        }
        output.push_str("}\n");
    }

    fn write_header(&self, output: &mut String) {
        let params = comma_separated(
            self.function
                .params
                .iter()
                .map(|register| self.typed_register_text(*register)),
        );
        let result = self.program.function_result_text(self.function.symbol);
        let startup = if self.program.startup == Some(self.function.symbol) {
            " startup"
        } else {
            ""
        };
        writeln!(
            output,
            "\nfunc {}({params}) -> {result} entry {} roots {}{startup} {{",
            self.program.symbol_text(self.function.symbol),
            block_label_text(self.function.entry),
            self.function.root_slots,
        )
        .unwrap();
    }

    fn write_block(&self, output: &mut String, id: BlockId, block: &BasicBlock) {
        writeln!(output, "{}:", block_label_text(id)).unwrap();
        for instruction in &block.instructions {
            writeln!(
                output,
                "  {} ; line {}",
                self.instruction_text(&instruction.kind),
                instruction.line
            )
            .unwrap();
        }
        writeln!(output, "  {}", self.terminator_text(&block.terminator)).unwrap();
    }

    fn register_text(&self, id: VirtualRegId) -> String {
        self.register_names
            .get(id.0)
            .cloned()
            .unwrap_or_else(|| format!("t{}", id.0))
    }

    fn typed_register_text(&self, id: VirtualRegId) -> String {
        let ty = self
            .function
            .register_types
            .get(id.0)
            .map(|ty| self.program.type_text(*ty))
            .unwrap_or_else(|| "<invalid type>".into());
        format!("{}:{ty}", self.register_text(id))
    }

    fn operand_text(&self, operand: Operand) -> String {
        match operand {
            Operand::Value(register) => self.register_text(register),
            Operand::Int(value) => value.to_string(),
            Operand::Bool(value) => value.to_string(),
            Operand::Null => "null".into(),
            Operand::Symbol(symbol) => self.program.symbol_text(symbol),
        }
    }

    fn arguments_text(&self, args: &[Operand]) -> String {
        comma_separated(args.iter().map(|arg| self.operand_text(*arg)))
    }

    fn invocation_text(&self, operation: &str, target: String, args: &[Operand]) -> String {
        format!("{operation} {target}({})", self.arguments_text(args))
    }

    fn call_text(&self, target: CallTarget, args: &[Operand]) -> String {
        match target {
            CallTarget::Direct(symbol) => {
                self.invocation_text("call", self.program.symbol_text(symbol), args)
            }
            CallTarget::Indirect(pointer) => {
                self.invocation_text("call_indirect", self.operand_text(pointer), args)
            }
        }
    }

    fn instruction_text(&self, instruction: &InstructionKind) -> String {
        let body = self.instruction_body_text(instruction);
        match instruction.destination() {
            Some(destination) => format!("{} = {body}", self.typed_register_text(destination)),
            None => body,
        }
    }

    fn instruction_body_text(&self, instruction: &InstructionKind) -> String {
        match instruction {
            InstructionKind::Copy { src, .. } => self.operand_text(*src),
            InstructionKind::Binary { op, lhs, rhs, .. } => format!(
                "{} {} {}",
                self.operand_text(*lhs),
                binary_operator_text(*op),
                self.operand_text(*rhs)
            ),
            InstructionKind::Unary { op, src, .. } => {
                format!("{} {}", unary_operator_text(*op), self.operand_text(*src))
            }
            InstructionKind::Load { base, offset, .. } => {
                format!("load [{} + {offset}]", self.operand_text(*base))
            }
            InstructionKind::Store {
                base,
                offset,
                value,
                ty,
            } => format!(
                "store {} [{} + {offset}], {}",
                self.program.type_text(*ty),
                self.operand_text(*base),
                self.operand_text(*value)
            ),
            InstructionKind::RootStore { slot, value } => {
                format!("root_store root{slot}, {}", self.operand_text(*value))
            }
            InstructionKind::RootLoad { slot, .. } => format!("root_load root{slot}"),
            InstructionKind::RootAddr { slot, .. } => format!("root_addr root{slot}"),
            InstructionKind::Call { target, args, .. } => self.call_text(*target, args),
        }
    }

    fn terminator_text(&self, terminator: &Terminator) -> String {
        match terminator {
            Terminator::Jump(block) => format!("br {}", block_label_text(*block)),
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => format!(
                "cbr {}, {}, {}",
                self.operand_text(*condition),
                block_label_text(*then_block),
                block_label_text(*else_block)
            ),
            Terminator::Return(Some(value)) => format!("ret {}", self.operand_text(*value)),
            Terminator::Return(None) => "ret".into(),
            Terminator::Abort { target, args } => {
                self.invocation_text("abort", self.program.symbol_text(*target), args)
            }
        }
    }
}

fn register_display_names(function: &FunctionIr) -> Vec<String> {
    let mut name_counts = HashMap::new();
    for name in function.register_names.iter().flatten() {
        *name_counts.entry(name.as_str()).or_insert(0) += 1;
    }
    function
        .register_names
        .iter()
        .enumerate()
        .map(|(index, name)| match name {
            Some(name)
                if is_generated_register_name(name)
                    || has_duplicate_register_name(name, &name_counts) =>
            {
                format!("{name}.v{index}")
            }
            Some(name) => name.clone(),
            None => format!("t{index}"),
        })
        .collect()
}

fn is_generated_register_name(name: &str) -> bool {
    name.strip_prefix('t')
        .is_some_and(|suffix| suffix.parse::<usize>().is_ok())
}

fn has_duplicate_register_name(name: &str, name_counts: &HashMap<&str, usize>) -> bool {
    name_counts.get(name).is_some_and(|count| *count > 1)
}

fn comma_separated(items: impl Iterator<Item = String>) -> String {
    items.collect::<Vec<_>>().join(", ")
}

fn block_label_text(block: BlockId) -> String {
    format!(".L{}", block.0)
}

fn binary_operator_text(operator: BinaryOp) -> &'static str {
    match operator {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Mod => "%",
        BinaryOp::Eq => "==",
        BinaryOp::Lt => "<",
        BinaryOp::Gt => ">",
    }
}

fn unary_operator_text(operator: UnaryOp) -> &'static str {
    match operator {
        UnaryOp::Neg => "neg",
        UnaryOp::Not => "!",
    }
}
