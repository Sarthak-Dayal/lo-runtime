//! Runtime aborts (`runtime-abi.md` §3.8). Native: message to stderr + exit with
//! the ABI status code. WASM: emit the same message via the host
//! `host.write_stderr` import (§3.7), then `@trap()` — so the accompanying stderr
//! is present on both targets, in the same format, for the harness to match on
//! (delta D-B3).
//!
//! `runtimeAbort` (and the trapping/native split it selects) is chosen at
//! comptime, so the untaken half is never analyzed (the native body references
//! `std.process`/stderr, which freestanding WASM lacks).
//!
//! ## Why `@trap()`, not the `unreachable` keyword (WASM)
//!
//! The WASM aborts emit a real trap via the `@trap()` builtin (it lowers to the
//! `unreachable` WASM opcode). They must **not** use Zig's `unreachable`
//! *keyword*: in `ReleaseSmall` / `ReleaseFast` that keyword is *undefined
//! behavior* — it tells the optimizer "this path is never reached" rather than
//! emitting a trap. At a call site like `lo_alloc`'s
//! `bumpIfFits(size) orelse runtimeAbort(...)`, an `unreachable`-based abort lets
//! the optimizer conclude the post-collection retry can never return null and
//! **delete the whole OOM-abort branch**; a genuine live-set overflow then
//! silently continues with a bogus slot instead of trapping (exit 137), so the
//! unbounded-allocation OOM test never terminates. `@trap()` is a real
//! side-effecting terminator the optimizer preserves — the Zig analog of the Rust
//! reference's `core::arch::wasm32::unreachable()` intrinsic (NOT Rust's
//! `unreachable!()` macro). This was delta D-B4 / R14.

const std = @import("std");
const builtin = @import("builtin");

const exit_codes = @import("exit_codes.zig");

const is_wasm = builtin.cpu.arch.isWasm();

/// Host stderr-write import (`runtime-abi.md` §3.7): `host.write_stderr(ptr, len)`
/// writes `len` bytes of linear memory at `ptr` to the process's stderr. The WASM
/// abort path emits the §3.8 message through it before trapping (delta D-B3).
extern "host" fn host_write_stderr(ptr: [*]const u8, len: i32) void;

/// Abort with `msg` on stderr and `code` as the exit status (native), or emit
/// `msg` via the host stderr-write import (§3.7) and `@trap()` (WASM). Never
/// returns.
pub const runtimeAbort = if (is_wasm) abortWasm else abortNative;

fn abortNative(msg: []const u8, code: u8) noreturn {
    std.debug.print("{s}\n", .{msg});
    std.process.exit(code);
}

fn abortWasm(msg: []const u8, code: u8) noreturn {
    _ = code;
    // Emit the documented §3.8 message before the trap (delta D-B3) so the WASM
    // stderr matches native; the host buffers it and prints it once.
    host_write_stderr(msg.ptr, @intCast(msg.len));
    @trap();
}

/// Abort on a null receiver at method dispatch (`runtime-abi.md` §3.8). Codegen
/// emits a null check before each dispatch and calls this on failure. Exit 102
/// (native) / message-then-trap (WASM). The body is target-agnostic — it only
/// builds the message and defers to `runtimeAbort` for the target-specific exit.
pub export fn lo_abort_null_receiver(method_name: ?[*]const u8, method_name_len: u32) noreturn {
    var buf: [256]u8 = undefined;
    const name: []const u8 = if (method_name) |p| p[0..method_name_len] else "<unknown>";
    const msg = std.fmt.bufPrint(
        &buf,
        "lo_abort_null_receiver: cannot dispatch {s}",
        .{name},
    ) catch "lo_abort_null_receiver: cannot dispatch";
    runtimeAbort(msg, exit_codes.EXIT_NULL_DISPATCH);
}
