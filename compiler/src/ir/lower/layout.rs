/// Concrete ABI layout used by lowering and by subsequent memory/data emission.
/// Every field occupies a pointer-sized slot; Int32/Bool accesses remain four bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetLayout {
    Wasm32,
    X86_64,
}

impl TargetLayout {
    pub fn pointer_bytes(self) -> u32 {
        match self {
            Self::Wasm32 => 4,
            Self::X86_64 => 8,
        }
    }

    pub fn object_header_bytes(self) -> u32 {
        self.pointer_bytes() + 8
    }

    pub fn descriptor_vtable_offset(self) -> i32 {
        match self {
            Self::Wasm32 => 28,
            Self::X86_64 => 48,
        }
    }
    pub(super) fn bindings_frame(self) -> crate::layout::FrameLayout {
        crate::layout::Target {
            ptr: self.pointer_bytes(),
        }
        .frame()
    }

    pub(super) fn binding_offset(self, name: &str) -> Result<i32, String> {
        let slot = crate::symbols::PREBOUND
            .iter()
            .position(|n| *n == name)
            .ok_or_else(|| format!("IR: unknown prebound name {name}"))?;
        offset(self.bindings_frame().root(slot as u32))
    }
}

pub(super) fn align(value: u32, alignment: u32) -> Result<u32, String> {
    value
        .checked_add(alignment - 1)
        .map(|n| n & !(alignment - 1))
        .ok_or_else(|| "IR: layout size overflow".into())
}

pub(super) fn offset(value: u32) -> Result<i32, String> {
    i32::try_from(value).map_err(|_| "IR: offset/size exceeds signed IR range".into())
}
