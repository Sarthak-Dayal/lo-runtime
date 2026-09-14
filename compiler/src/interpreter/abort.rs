#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortKind {
    /// A `Downcast` whose runtime class is not the target or a descendant.
    CastFailed { from: String, to: String },
    /// A method dispatched on a null receiver.
    NullReceiver { method: String },
    /// `read_int` got a token that is not an integer.
    ReadIntMalformed,
    /// `read_int` hit end-of-input before any integer characters.
    ReadIntEof,
    /// `read_bool` got a token other than `true`/`false` (EOF included).
    ReadBoolInvalid,
    /// String repeat (`s * n`) with a negative count.
    RepeatNegative(i32),
    /// The arena byte budget was exhausted.
    OutOfMemory,
}

impl AbortKind {
    /// The native exit status this abort maps to (ABI §3.8).
    pub fn code(&self) -> i32 {
        match self {
            AbortKind::CastFailed { .. } => 101,
            AbortKind::NullReceiver { .. } => 102,
            AbortKind::ReadIntMalformed => 110,
            AbortKind::ReadIntEof => 111,
            AbortKind::ReadBoolInvalid => 112,
            AbortKind::RepeatNegative(_) => 120,
            AbortKind::OutOfMemory => 137,
        }
    }

    /// The stderr message. Prefixes are contractual (see the type-level note).
    pub fn message(&self) -> String {
        match self {
            AbortKind::CastFailed { from, to } => {
                format!("lo_cast_check: cannot cast {from} to {to}")
            }
            AbortKind::NullReceiver { method } => {
                format!("lo_abort_null_receiver: cannot dispatch {method}")
            }
            AbortKind::ReadIntMalformed => "lo_read_int: malformed token".to_string(),
            AbortKind::ReadIntEof => "lo_read_int: end of input".to_string(),
            AbortKind::ReadBoolInvalid => "lo_read_bool: invalid token".to_string(),
            AbortKind::RepeatNegative(n) => format!("lo_string_repeat: negative count {n}"),
            AbortKind::OutOfMemory => "lo_alloc: out of memory".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_the_abi_table() {
        assert_eq!(
            AbortKind::CastFailed {
                from: "A".into(),
                to: "B".into()
            }
            .code(),
            101
        );
        assert_eq!(AbortKind::NullReceiver { method: "m".into() }.code(), 102);
        assert_eq!(AbortKind::ReadIntMalformed.code(), 110);
        assert_eq!(AbortKind::ReadIntEof.code(), 111);
        assert_eq!(AbortKind::ReadBoolInvalid.code(), 112);
        assert_eq!(AbortKind::RepeatNegative(-3).code(), 120);
        assert_eq!(AbortKind::OutOfMemory.code(), 137);
    }

    #[test]
    fn messages_carry_the_contractual_prefixes() {
        assert_eq!(
            AbortKind::CastFailed {
                from: "Dog".into(),
                to: "Cat".into()
            }
            .message(),
            "lo_cast_check: cannot cast Dog to Cat"
        );
        assert_eq!(
            AbortKind::NullReceiver {
                method: "speak".into()
            }
            .message(),
            "lo_abort_null_receiver: cannot dispatch speak"
        );
        assert_eq!(
            AbortKind::ReadIntMalformed.message(),
            "lo_read_int: malformed token"
        );
        assert_eq!(AbortKind::ReadIntEof.message(), "lo_read_int: end of input");
        assert_eq!(
            AbortKind::ReadBoolInvalid.message(),
            "lo_read_bool: invalid token"
        );
        assert_eq!(
            AbortKind::RepeatNegative(-3).message(),
            "lo_string_repeat: negative count -3"
        );
        assert_eq!(AbortKind::OutOfMemory.message(), "lo_alloc: out of memory");
    }
}
