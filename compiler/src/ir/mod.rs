mod dump;
pub mod lower;
mod verify;

#[cfg(test)]
mod lower_tests;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SymbolId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignatureId(pub usize);

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

pub struct ProgramIr {
    pub symbols: Vec<Symbol>,
    pub signatures: Vec<Signature>,
    pub functions: Vec<FunctionIr>,
    pub data: Vec<DataDef>,
    // Explicitly identifies the function allowed to expose root-slot addresses.
    pub startup: Option<SymbolId>,
}

pub struct DataDef {
    pub symbol: SymbolId,
    pub section: Section,
    pub align: u32,
    pub items: Vec<DataItem>,
}

pub enum Section {
    ReadOnly,
    Writable,
}

pub enum DataItem {
    U32(u32),
    // A relocation; the target determines address width.
    Addr(SymbolId),
    Bytes(Vec<u8>),
    Zero(u32),
}

impl ProgramIr {
    pub fn intern_signature(&mut self, signature: Signature) -> SignatureId {
        if let Some(id) = self
            .signatures
            .iter()
            .position(|existing| *existing == signature)
        {
            return SignatureId(id);
        }
        let id = SignatureId(self.signatures.len());
        self.signatures.push(signature);
        id
    }
}

pub struct CheckedIr {
    program: ProgramIr,
}

pub struct FunctionIr {
    pub symbol: SymbolId,
    pub params: Vec<ValueId>,
    // Function-local IDs index these tables. Values are mutable, not SSA.
    pub value_types: Vec<IrType>,
    pub value_names: Vec<Option<String>>,
    // Frame construction initializes these slots to null.
    pub root_slots: u32,
    pub blocks: Vec<BasicBlock>,
    pub entry: BlockId,
}

pub struct BasicBlock {
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
}

pub struct Instruction {
    pub kind: InstructionKind,
    pub line: u32,
}

#[derive(Clone, Copy, Debug)]
pub enum Operand {
    Value(ValueId),
    Int(i32),
    Bool(bool),
    Null,
    Symbol(SymbolId),
}

pub enum InstructionKind {
    Copy {
        dst: ValueId,
        src: Operand,
    },
    Binary {
        dst: ValueId,
        op: BinaryOp,
        lhs: Operand,
        rhs: Operand,
    },
    Unary {
        dst: ValueId,
        op: UnaryOp,
        src: Operand,
    },
    // Load width comes from dst's type; offsets come from target layout.
    // Code-pointer loads trust layout to supply the declared signature.
    Load {
        dst: ValueId,
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
        dst: ValueId,
        slot: u32,
    },
    RootAddr {
        dst: ValueId,
        slot: u32,
    },
    // Every call is conservatively treated as a GC safepoint.
    Call {
        dst: Option<ValueId>,
        target: CallTarget,
        args: Vec<Operand>,
    },
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
    pub fn uses(&self) -> Vec<Operand> {
        match self {
            Self::Branch { condition, .. } => vec![*condition],
            Self::Return(Some(value)) => vec![*value],
            Self::Abort { args, .. } => args.clone(),
            Self::Jump(_) | Self::Return(None) => vec![],
        }
    }

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
    pub fn destination(&self) -> Option<ValueId> {
        match self {
            Self::Copy { dst, .. }
            | Self::Binary { dst, .. }
            | Self::Unary { dst, .. }
            | Self::Load { dst, .. }
            | Self::RootLoad { dst, .. }
            | Self::RootAddr { dst, .. } => Some(*dst),
            Self::Call { dst, .. } => *dst,
            _ => None,
        }
    }

    pub fn uses(&self) -> Vec<Operand> {
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
