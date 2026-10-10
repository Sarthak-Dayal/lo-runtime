//! Native process exit codes for runtime aborts (`runtime-abi.md` §3.8 table).
//! WASM ignores these and traps via `unreachable` instead; see [`crate::abort`].

/// `lo_cast_check`: the object's class is not `target` or a descendant.
pub(crate) const EXIT_CAST_FAILURE: i32 = 101;
/// `lo_abort_null_receiver`: method dispatch on a null receiver.
pub(crate) const EXIT_NULL_DISPATCH: i32 = 102;
/// `lo_read_int`: a non-whitespace token failed to parse as an integer.
pub(crate) const EXIT_MALFORMED_INT: i32 = 110;
/// `lo_read_int`: end of input before any integer characters.
pub(crate) const EXIT_EOF_BEFORE_INT: i32 = 111;
/// `lo_read_bool`: the next token is neither `true` nor `false` (including EOF).
pub(crate) const EXIT_INVALID_BOOL: i32 = 112;
/// `lo_string_repeat`: negative repeat count.
pub(crate) const EXIT_NEGATIVE_REPEAT: i32 = 120;
/// `lo_alloc`: the live set still doesn't fit after a collection.
pub(crate) const EXIT_OOM: i32 = 137;
