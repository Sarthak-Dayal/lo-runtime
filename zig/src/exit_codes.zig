//! Native process exit codes for runtime aborts (`runtime-abi.md` §3.8 table).
//! WASM ignores these and traps via `@trap()` instead; see `abort.zig`.

/// `lo_cast_check`: the object's class is not `target` or a descendant.
pub const EXIT_CAST_FAILURE: u8 = 101;
/// `lo_abort_null_receiver`: method dispatch on a null receiver.
pub const EXIT_NULL_DISPATCH: u8 = 102;
/// `lo_read_int`: a non-whitespace token failed to parse as an integer.
pub const EXIT_MALFORMED_INT: u8 = 110;
/// `lo_read_int`: end of input before any integer characters.
pub const EXIT_EOF_BEFORE_INT: u8 = 111;
/// `lo_read_bool`: the next token is neither `true` nor `false` (including EOF).
pub const EXIT_INVALID_BOOL: u8 = 112;
/// `lo_string_repeat`: negative repeat count.
pub const EXIT_NEGATIVE_REPEAT: u8 = 120;
/// `lo_alloc`: the live set still doesn't fit after a collection.
pub const EXIT_OOM: u8 = 137;
