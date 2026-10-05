use super::*;
use std::fmt::Write;

impl ProgramIr {
    pub fn dump(&self) -> String {
        let mut out = String::new();
        for (id, symbol) in self.symbols.iter().enumerate() {
            if self.functions.iter().any(|f| f.symbol == SymbolId(id))
                || self.data.iter().any(|d| d.symbol == SymbolId(id))
            {
                continue;
            }
            match symbol.kind {
                SymbolKind::Function(sig) => writeln!(
                    out,
                    "extern func @{}{}",
                    symbol.name,
                    self.signature_text(sig)
                )
                .unwrap(),
                SymbolKind::Data => writeln!(out, "extern data @{}", symbol.name).unwrap(),
                SymbolKind::StaticRef => writeln!(out, "extern ref @{}", symbol.name).unwrap(),
            }
        }
        for data in &self.data {
            let section = match data.section {
                Section::ReadOnly => ".rodata",
                Section::Writable => ".data",
            };
            writeln!(
                out,
                "{section} {} align {} {{",
                self.symbol_text(data.symbol),
                data.align
            )
            .unwrap();
            for item in &data.items {
                match item {
                    DataItem::U32(n) => writeln!(out, "  u32 {n}").unwrap(),
                    DataItem::Addr(id) => {
                        writeln!(out, "  addr {}", self.symbol_text(*id)).unwrap()
                    }
                    DataItem::Zero(n) => writeln!(out, "  zero {n}").unwrap(),
                    DataItem::Bytes(bytes) => {
                        out.push_str("  bytes");
                        for byte in bytes {
                            write!(out, " {byte:02x}").unwrap();
                        }
                        out.push('\n');
                    }
                }
            }
            out.push_str("}\n");
        }
        for f in &self.functions {
            let context = FunctionDump {
                program: self,
                function: f,
            };
            let params = f
                .params
                .iter()
                .map(|id| context.typed_value(*id))
                .collect::<Vec<_>>()
                .join(", ");
            let result = match self.symbols.get(f.symbol.0).map(|s| s.kind) {
                Some(SymbolKind::Function(id)) => self
                    .signatures
                    .get(id.0)
                    .map(|s| self.result_text(s.result))
                    .unwrap_or_else(|| "<invalid signature>".into()),
                _ => "<invalid function>".into(),
            };
            writeln!(
                out,
                "\nfunc {}({params}) -> {result} entry .L{} roots {}{} {{",
                self.symbol_text(f.symbol),
                f.entry.0,
                f.root_slots,
                if self.startup == Some(f.symbol) {
                    " startup"
                } else {
                    ""
                }
            )
            .unwrap();
            for (id, block) in f.blocks.iter().enumerate() {
                writeln!(out, ".L{id}:").unwrap();
                for inst in &block.instructions {
                    writeln!(
                        out,
                        "  {} ; line {}",
                        context.instruction(&inst.kind),
                        inst.line
                    )
                    .unwrap();
                }
                writeln!(out, "  {}", context.terminator(&block.terminator)).unwrap();
            }
            out.push_str("}\n");
        }
        out
    }

    fn symbol_text(&self, id: SymbolId) -> String {
        self.symbols
            .get(id.0)
            .map(|s| format!("@{}", s.name))
            .unwrap_or_else(|| format!("@<invalid:{}>", id.0))
    }

    fn signature_text(&self, id: SignatureId) -> String {
        match self.signatures.get(id.0) {
            Some(sig) => {
                // Signatures contain only value types, never nested code-pointer types.
                let scalar = |ty| match ty {
                    IrType::CodePtr(_) => "<invalid nested CodePtr>".into(),
                    _ => self.type_text(ty),
                };
                let params = sig
                    .params
                    .iter()
                    .map(|ty| scalar(*ty))
                    .collect::<Vec<_>>()
                    .join(", ");
                let result = sig.result.map(scalar).unwrap_or_else(|| "Void".into());
                format!("({params}) -> {result}")
            }
            None => format!("<invalid signature:{}>", id.0),
        }
    }

    fn type_text(&self, ty: IrType) -> String {
        match ty {
            IrType::Int32 => "Int32".into(),
            IrType::Bool => "Bool".into(),
            IrType::Ref => "Ref".into(),
            IrType::Ptr => "Ptr".into(),
            IrType::CodePtr(id) => format!("CodePtr{}", self.signature_text(id)),
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
}

impl FunctionDump<'_> {
    fn value(&self, id: VirtualRegId) -> String {
        match self
            .function
            .register_names
            .get(id.0)
            .and_then(|name| name.as_ref())
        {
            Some(name) => {
                let collides = name
                    .strip_prefix('t')
                    .is_some_and(|suffix| suffix.parse::<usize>().is_ok())
                    || self
                        .function
                        .register_names
                        .iter()
                        .filter(|other| other.as_ref() == Some(name))
                        .count()
                        > 1;
                if collides {
                    format!("{name}.v{}", id.0)
                } else {
                    name.clone()
                }
            }
            None => format!("t{}", id.0),
        }
    }

    fn typed_value(&self, id: VirtualRegId) -> String {
        let ty = self
            .function
            .register_types
            .get(id.0)
            .map(|ty| self.program.type_text(*ty))
            .unwrap_or_else(|| "<invalid type>".into());
        format!("{}:{ty}", self.value(id))
    }

    fn operand(&self, operand: Operand) -> String {
        match operand {
            Operand::Value(id) => self.value(id),
            Operand::Int(n) => n.to_string(),
            Operand::Bool(b) => b.to_string(),
            Operand::Null => "null".into(),
            Operand::Symbol(id) => self.program.symbol_text(id),
        }
    }

    fn args(&self, args: &[Operand]) -> String {
        args.iter()
            .map(|arg| self.operand(*arg))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn instruction(&self, inst: &InstructionKind) -> String {
        let rhs = match inst {
            InstructionKind::Copy { src, .. } => self.operand(*src),
            InstructionKind::Binary { op, lhs, rhs, .. } => {
                let op = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => "%",
                    BinaryOp::Eq => "==",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gt => ">",
                };
                format!("{} {op} {}", self.operand(*lhs), self.operand(*rhs))
            }
            InstructionKind::Unary { op, src, .. } => format!(
                "{} {}",
                match op {
                    UnaryOp::Neg => "neg",
                    UnaryOp::Not => "!",
                },
                self.operand(*src)
            ),
            InstructionKind::Load { base, offset, .. } => {
                format!("load [{} + {offset}]", self.operand(*base))
            }
            InstructionKind::Store {
                base,
                offset,
                value,
                ty,
            } => format!(
                "store {} [{} + {offset}], {}",
                self.program.type_text(*ty),
                self.operand(*base),
                self.operand(*value)
            ),
            InstructionKind::RootStore { slot, value } => {
                format!("root_store root{slot}, {}", self.operand(*value))
            }
            InstructionKind::RootLoad { slot, .. } => format!("root_load root{slot}"),
            InstructionKind::RootAddr { slot, .. } => format!("root_addr root{slot}"),
            InstructionKind::Call { target, args, .. } => {
                let target = match target {
                    CallTarget::Direct(id) => format!("call {}", self.program.symbol_text(*id)),
                    CallTarget::Indirect(pointer) => {
                        format!("call_indirect {}", self.operand(*pointer))
                    }
                };
                format!("{target}({})", self.args(args))
            }
        };
        match inst.destination() {
            Some(dst) => format!("{} = {rhs}", self.typed_value(dst)),
            None => rhs,
        }
    }

    fn terminator(&self, term: &Terminator) -> String {
        match term {
            Terminator::Jump(block) => format!("br .L{}", block.0),
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => format!(
                "cbr {}, .L{}, .L{}",
                self.operand(*condition),
                then_block.0,
                else_block.0
            ),
            Terminator::Return(Some(value)) => format!("ret {}", self.operand(*value)),
            Terminator::Return(None) => "ret".into(),
            Terminator::Abort { target, args } => format!(
                "abort {}({})",
                self.program.symbol_text(*target),
                self.args(args)
            ),
        }
    }
}
