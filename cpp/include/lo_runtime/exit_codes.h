#pragma once

// Native process exit codes for runtime aborts (runtime-abi.md §3.8 table).
// WASM ignores these and traps via __builtin_trap() instead; see abort.cpp.

namespace lo {

// lo_cast_check: the object's class is not `target` or a descendant.
inline constexpr int kExitCastFailure = 101;
// lo_abort_null_receiver: method dispatch on a null receiver.
inline constexpr int kExitNullDispatch = 102;
// lo_read_int: a non-whitespace token failed to parse as an integer.
inline constexpr int kExitMalformedInt = 110;
// lo_read_int: end of input before any integer characters.
inline constexpr int kExitEofBeforeInt = 111;
// lo_read_bool: the next token is neither "true" nor "false" (including EOF).
inline constexpr int kExitInvalidBool = 112;
// lo_string_repeat: negative repeat count.
inline constexpr int kExitNegativeRepeat = 120;
// lo_alloc: the live set still doesn't fit after a collection.
inline constexpr int kExitOom = 137;

} // namespace lo
