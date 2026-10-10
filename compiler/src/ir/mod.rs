mod declare;
mod dump;
pub mod lower;
pub mod runtime;
pub mod statics;
mod verify;

#[cfg(test)]
mod lower_tests;
#[cfg(test)]
mod tests;

// IDs used to index into tables for registers, blocks, symbols, and signatures
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VirtualRegId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SymbolId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SignatureId(pub usize);

// Types used in the IR and in the LO programs
// Ref is a collectible heap object or static reference (like LO_EMPTY_STRING),
// Ptr is a raw pointer, CodePtr is a function pointer
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrType {
    Int32,
    Bool,
    Ref,
    Ptr,
    CodePtr(SignatureId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub params: Vec<IrType>,
    pub result: Option<IrType>,
}

pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
}

#[derive(Clone, Copy, Debug)]
pub enum SymbolKind {
    Function(SignatureId),
    Data,
    // An object outside the collectable heap, such as LO_EMPTY_STRING.
    StaticRef,
}

pub struct DataDef {
    pub symbol: SymbolId,
    pub section: Section,
    pub align: u32,
    pub items: Vec<DataItem>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    ReadOnly,
    Writable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataItem {
    U32(u32),
    // A relocation; the target determines address width
    Addr(SymbolId),
    // For static strings and other raw data
    Bytes(Vec<u8>),
    Zero(u32),
}

pub struct CheckedIr {
    program: ProgramIr,
}

pub struct ProgramIr {
    pub symbols: Vec<Symbol>,
    pub signatures: Vec<Signature>,
    pub functions: Vec<FunctionIr>,
    pub data: Vec<DataDef>,
    // Explicitly identifies the function allowed to expose root-slot addresses.
    pub startup: Option<SymbolId>,
}

impl ProgramIr {
    pub fn intern_signature(&mut self, signature: Signature) -> SignatureId {
        let existing_id = self
            .signatures
            .iter()
            .position(|existing| existing == &signature);

        if let Some(index) = existing_id {
            return SignatureId(index);
        }

        let id = SignatureId(self.signatures.len());
        self.signatures.push(signature);
        id
    }
}

pub struct FunctionIr {
    pub symbol: SymbolId,
    pub params: Vec<VirtualRegId>,
    // Function-local IDs index these tables. Values are mutable, not SSA.
    pub register_types: Vec<IrType>,
    pub register_names: Vec<Option<String>>,
    // Frame construction initializes these slots to null.
    pub root_slots: u32,
    pub blocks: Vec<BasicBlock>,
    pub entry: BlockId,
}

impl FunctionIr {
    /// Adds a mutable virtual register, keeping its type and optional name together.
    /// The register is initially unassigned unless it is added to `params`.
    ///
    /// Panics if the existing register tables have different lengths.
    pub fn new_register(&mut self, ty: IrType, name: Option<String>) -> VirtualRegId {
        assert_eq!(
            self.register_types.len(),
            self.register_names.len(),
            "register_names must match register_types length"
        );
        let id = VirtualRegId(self.register_types.len());
        self.register_types.push(ty);
        self.register_names.push(name);
        id
    }
}

pub struct BasicBlock {
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
}

pub struct Instruction {
    pub kind: InstructionKind,
    pub line: u32,
}

pub enum InstructionKind {
    Copy {
        dst: VirtualRegId,
        src: Operand,
    },
    Binary {
        dst: VirtualRegId,
        op: BinaryOp,
        lhs: Operand,
        rhs: Operand,
    },
    Unary {
        dst: VirtualRegId,
        op: UnaryOp,
        src: Operand,
    },
    // Load width comes from dst's type; offsets come from target layout.
    // Code-pointer loads trust layout to supply the declared signature.
    Load {
        dst: VirtualRegId,
        base: Operand,
        offset: i32,
    },
    Store {
        base: Operand,
        offset: i32,
        value: Operand,
        ty: IrType,
    },
    RootStore {
        slot: u32,
        value: Operand,
    },
    RootLoad {
        dst: VirtualRegId,
        slot: u32,
    },
    RootAddr {
        dst: VirtualRegId,
        slot: u32,
    },
    // Every call is conservatively treated as a GC safepoint.
    Call {
        dst: Option<VirtualRegId>,
        target: CallTarget,
        args: Vec<Operand>,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum Operand {
    Value(VirtualRegId),
    Int(i32),
    Bool(bool),
    Null,
    Symbol(SymbolId),
}

#[derive(Clone, Copy, Debug)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    // Lowering must guard divisors 0 and -1 before these operations.
    Div,
    Mod,
    Eq,
    Lt,
    Gt,
}

#[derive(Clone, Copy, Debug)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug)]
pub enum CallTarget {
    Direct(SymbolId),
    Indirect(Operand),
}

pub enum Terminator {
    Jump(BlockId),
    Branch {
        condition: Operand,
        then_block: BlockId,
        else_block: BlockId,
    },
    Return(Option<Operand>),
    // Calls a nonreturning runtime helper.
    Abort {
        target: SymbolId,
        args: Vec<Operand>,
    },
}

impl Terminator {
    /// Returns read operands in their stored order, including duplicates.
    /// Branch targets and direct function symbols are not read operands.
    pub fn operands(&self) -> Vec<Operand> {
        match self {
            Self::Branch { condition, .. } => vec![*condition],
            Self::Return(Some(value)) => vec![*value],
            Self::Abort { args, .. } => args.clone(),
            Self::Jump(_) | Self::Return(None) => vec![],
        }
    }

    /// Returns the registers in `operands()`, preserving order and duplicates.
    pub fn used_registers(&self) -> Vec<VirtualRegId> {
        registers_in_operands(self.operands())
    }

    /// Returns successor blocks in their stored order, including duplicate targets.
    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Self::Jump(block) => vec![*block],
            Self::Branch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            Self::Return(_) | Self::Abort { .. } => vec![],
        }
    }
}

impl InstructionKind {
    /// Returns the written register, if any. Reads occur before this write.
    pub fn destination(&self) -> Option<VirtualRegId> {
        match self {
            Self::Copy { dst, .. }
            | Self::Binary { dst, .. }
            | Self::Unary { dst, .. }
            | Self::Load { dst, .. }
            | Self::RootLoad { dst, .. }
            | Self::RootAddr { dst, .. } => Some(*dst),
            Self::Call { dst, .. } => *dst,
            Self::Store { .. } | Self::RootStore { .. } => None,
        }
    }

    /// Returns the registers in `operands()`, preserving order and duplicates.
    pub fn used_registers(&self) -> Vec<VirtualRegId> {
        registers_in_operands(self.operands())
    }

    /// Returns read operands, preserving order and duplicates, excluding the destination.
    /// For indirect calls, the function pointer follows the argument operands.
    pub fn operands(&self) -> Vec<Operand> {
        match self {
            Self::Copy { src, .. } | Self::Unary { src, .. } => vec![*src],
            Self::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
            Self::Load { base, .. } => vec![*base],
            Self::Store { base, value, .. } => vec![*base, *value],
            Self::RootStore { value, .. } => vec![*value],
            Self::RootLoad { .. } | Self::RootAddr { .. } => vec![],
            Self::Call { target, args, .. } => {
                let mut operands = args.clone();
                if let CallTarget::Indirect(pointer) = target {
                    operands.push(*pointer);
                }
                operands
            }
        }
    }
}

fn registers_in_operands(operands: Vec<Operand>) -> Vec<VirtualRegId> {
    operands
        .into_iter()
        .filter_map(|operand| match operand {
            Operand::Value(register) => Some(register),
            _ => None,
        })
        .collect()
}
