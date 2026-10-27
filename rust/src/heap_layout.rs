//! Shared allocation and heap-walk sizes. No collector may invent its own stride.
use crate::object::{string_data_offset, Object};

pub(crate) const OBJECT_ALIGN: usize = core::mem::align_of::<Object>();

pub(crate) fn aligned_size(bytes: usize) -> Option<usize> {
    bytes.checked_add(OBJECT_ALIGN - 1).map(|n| n & !(OBJECT_ALIGN - 1))
}

pub(crate) fn string_size(length: usize) -> Option<usize> {
    u32::try_from(length).ok()?;
    aligned_size(string_data_offset().checked_add(length)?)
}

pub(crate) fn oom() -> ! {
    crate::abort::runtime_abort("lo_alloc: out of memory", crate::exit_codes::EXIT_OOM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_target_layout_and_reject_overflow() {
        for len in [0, 1, 3, 8, 17] {
            let size = string_size(len).unwrap();
            assert!(size >= string_data_offset() + len);
            assert!(size - (string_data_offset() + len) < OBJECT_ALIGN);
            assert_eq!(size % OBJECT_ALIGN, 0);
        }
        assert_eq!(aligned_size(usize::MAX), None);
        assert_eq!(string_size(usize::MAX), None);
    }
}
