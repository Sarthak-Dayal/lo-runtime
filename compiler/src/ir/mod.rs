mod dump;
mod verify;

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
    CodePtr,
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
}

pub struct FunctionIr {
    pub symbol: SymbolId,
    pub params: Vec<ValueId>,
    // Function-local IDs index these tables. Values are mutable, not SSA.
    pub value_types: Vec<IrType>,
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
    // Performs the managed field store through the runtime write barrier.
    StoreRef {
        object: Operand,
        offset: i32,
        value: Operand,
    },
    NullCheck {
        receiver: Operand,
        method_name: String,
    },
    Call {
        dst: Option<ValueId>,
        target: CallTarget,
        signature: SignatureId,
        args: Vec<Operand>,
        effects: CallEffects,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
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

#[derive(Clone, Copy, Debug)]
pub struct CallEffects {
    pub may_gc: bool,
    pub no_return: bool,
}

impl Default for CallEffects {
    fn default() -> Self {
        Self {
            may_gc: true,
            no_return: false,
        }
    }
}

pub enum Terminator {
    Jump(BlockId),
    Branch {
        condition: Operand,
        then_block: BlockId,
        else_block: BlockId,
    },
    Return(Option<Operand>),
    Unreachable,
}

impl Terminator {
    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Self::Jump(block) => vec![*block],
            Self::Branch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            Self::Return(_) | Self::Unreachable => vec![],
        }
    }
}

impl InstructionKind {
    pub(super) fn destination(&self) -> Option<ValueId> {
        match self {
            Self::Copy { dst, .. }
            | Self::Binary { dst, .. }
            | Self::Unary { dst, .. }
            | Self::Load { dst, .. } => Some(*dst),
            Self::Call { dst, .. } => *dst,
            _ => None,
        }
    }

    pub(super) fn operands(&self) -> Vec<Operand> {
        match self {
            Self::Copy { src, .. } | Self::Unary { src, .. } => vec![*src],
            Self::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
            Self::Load { base, .. } => vec![*base],
            Self::Store { base, value, .. } => vec![*base, *value],
            Self::StoreRef { object, value, .. } => vec![*object, *value],
            Self::NullCheck { receiver, .. } => vec![*receiver],
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
