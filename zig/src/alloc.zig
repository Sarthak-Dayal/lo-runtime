//! Semispace bump allocator backing `lo_alloc` (`runtime-abi.md` §3.1), with the
//! WS-2 Cheney collector wired into the OOM path.
//!
//! The managed heap is split into two equal **semispaces** (`runtime-abi.md`
//! §2.1, runbook WS-2 §2.1). Exactly one is active at a time — the *from-space* —
//! and allocation is a bump pointer (`free_ptr`) within it. When a request does
//! not fit, `lo_alloc` forces a collection (`lo_gc_collect`, see `gc.zig`) and
//! retries once; only a genuine live-set overflow aborts (exit 137 native /
//! `unreachable` WASM).
//!
//! Total heap size inherits the WS-1 mechanism (decision (B)): a 16 MiB default,
//! overridable at `lo_runtime_init` via `LO_HEAP_SIZE`; each semispace is half the
//! total. The geometry matches the Rust reference (`rust/src/alloc.rs`)
//! byte-for-byte so cross-skeleton behavior is identical. Globals are touched only
//! on the single runtime thread.

const std = @import("std");
const builtin = @import("builtin");

const object = @import("object.zig");
const descriptors = @import("descriptors.zig");
const abort = @import("abort.zig");
const exit_codes = @import("exit_codes.zig");
const gc = @import("gc.zig");
const Object = object.Object;
const ClassDescriptor = object.ClassDescriptor;
const StringObject = object.StringObject;

/// Default total heap size (16 MiB), overridable via `LO_HEAP_SIZE` at init.
const DEFAULT_HEAP_SIZE: usize = 16 * 1024 * 1024;
const HEAP_ALIGN: usize = 16;
/// Every allocation's size is rounded up to this so `Object`'s 8-byte pointer
/// field (native) stays aligned within the semispace.
const OBJECT_ALIGN: usize = 8;
const is_wasm = builtin.cpu.arch.isWasm();

/// Backing region for the whole heap (both semispaces); freed at shutdown.
var heap_slice: ?[]u8 = null;

/// Active (from-)space `[from_base, from_limit)`, bump cursor at `free_ptr`.
var from_base: usize = 0;
var from_limit: usize = 0;
var free_ptr: usize = 0;
/// Inactive (to-)space `[to_base, to_limit)`: the collector copies survivors here
/// and then `flip`s the roles.
var to_base: usize = 0;
var to_limit: usize = 0;

inline fn alignUp(n: usize, a: usize) usize {
    return (n + a - 1) & ~(a - 1);
}

inline fn alignDown(n: usize, a: usize) usize {
    return n & ~(a - 1);
}

fn readHeapSize() usize {
    if (comptime is_wasm) {
        return DEFAULT_HEAP_SIZE;
    } else {
        const v = std.process.getEnvVarOwned(std.heap.page_allocator, "LO_HEAP_SIZE") catch
            return DEFAULT_HEAP_SIZE;
        defer std.heap.page_allocator.free(v);
        const n = std.fmt.parseInt(usize, v, 10) catch return DEFAULT_HEAP_SIZE;
        return if (n >= 2 * HEAP_ALIGN) n else DEFAULT_HEAP_SIZE;
    }
}

/// Allocate the heap region and carve it into two equal semispaces. Called from
/// `lo_runtime_init`. Reads the total size from `LO_HEAP_SIZE` (16 MiB default).
pub fn heapInit() void {
    heapInitWith(readHeapSize());
}

/// Like `heapInit` but with an explicit total size — a test hook so the
/// allocation-pressure test can force a small heap deterministically without
/// poking the process environment (the env path itself is covered in the Rust
/// reference). Not part of the ABI.
pub fn heapInitWith(total: usize) void {
    // Each semispace is half the total, rounded down to the heap alignment so
    // both semispace bases stay aligned; allocate `2 * semi` exactly.
    const semi = alignDown(total / 2, HEAP_ALIGN);
    const region = semi * 2;
    const slice = std.heap.page_allocator.alignedAlloc(u8, HEAP_ALIGN, region) catch
        abort.runtimeAbort("lo_alloc: out of memory", exit_codes.EXIT_OOM);
    @memset(slice, 0);
    heap_slice = slice;
    const base = @intFromPtr(slice.ptr);
    from_base = base;
    from_limit = base + semi;
    free_ptr = base;
    to_base = base + semi;
    to_limit = base + region;
}

/// Release the heap region. Called from `lo_runtime_shutdown`. Idempotent.
pub fn heapShutdown() void {
    if (heap_slice) |slice| {
        std.heap.page_allocator.free(slice);
        heap_slice = null;
    }
    from_base = 0;
    from_limit = 0;
    free_ptr = 0;
    to_base = 0;
    to_limit = 0;
}

/// Try to bump `size` bytes off the active semispace; null if it does not fit.
fn bumpIfFits(size: usize) ?usize {
    if (free_ptr == 0 or size > from_limit - free_ptr) return null;
    const p = free_ptr;
    free_ptr += size;
    return p;
}

/// Carve `size` zero-initialized bytes from the active semispace and stamp the
/// header. On a no-fit, forces one collection and retries; aborts (exit 137) only
/// if the live set still leaves no room (runbook WS-2 §2.1, ABI §3.1).
fn allocRaw(size: usize, class: *const ClassDescriptor) *Object {
    const slot = bumpIfFits(size) orelse blk: {
        gc.lo_gc_collect();
        break :blk bumpIfFits(size) orelse
            abort.runtimeAbort("lo_alloc: out of memory", exit_codes.EXIT_OOM);
    };
    const bytes: [*]u8 = @ptrFromInt(slot);
    @memset(bytes[0..size], 0);
    const o: *Object = @ptrCast(@alignCast(bytes));
    o.class_descriptor = class;
    o.gc_bits = 0;
    o.flags = 0;
    return o;
}

/// Allocate a zero-initialized object of `class.instance_size` bytes
/// (`runtime-abi.md` §3.1), header filled in. Triggers a collection and retries
/// on a full semispace; aborts (exit 137) only on live-set overflow. Codegen
/// finalizes String fields to `LO_EMPTY_STRING` after this returns.
pub export fn lo_alloc(class: *const ClassDescriptor) *Object {
    const size = alignUp(class.instance_size, OBJECT_ALIGN);
    return allocRaw(size, class);
}

/// Allocate a `StringObject` with room for `len` inline bytes, header stamped to
/// `LO_STRING_CLASS` and `length` set; the caller fills the (zeroed) data bytes.
/// Internal variable-size string allocator used by the provided `lo_read_string`
/// and the team-implemented `lo_string_*` ops; `lo_alloc` can't size a string
/// itself.
pub fn bumpAllocString(len: u32) *Object {
    const size = alignUp(object.stringDataOffset() + len, OBJECT_ALIGN);
    const o = allocRaw(size, &descriptors.LO_STRING_CLASS);
    const so: *StringObject = @ptrCast(@alignCast(o));
    so.length = len;
    return o;
}

// --- Collector interface ----------------------------------------------------
// The seam the Cheney collector (`gc.zig`) drives: range-check from-space, find
// where to copy survivors, and commit the flip — without re-deriving geometry.

/// Base of the active (from-)space being evacuated.
pub fn fromBase() usize {
    return from_base;
}

/// Limit of the active (from-)space being evacuated.
pub fn fromLimit() usize {
    return from_limit;
}

/// Start of the inactive (to-)space — where the collector copies survivors.
pub fn toSpaceBase() usize {
    return to_base;
}

/// Commit a collection: swap the from/to roles and install `new_free` as the bump
/// cursor in the now-active space (which the collector just filled with
/// survivors). The old from-space becomes inactive, entirely free for the next
/// cycle (runbook WS-2 §2.4 step 4).
pub fn flip(new_free: usize) void {
    const old_from_base = from_base;
    const old_from_limit = from_limit;
    from_base = to_base;
    from_limit = to_limit;
    to_base = old_from_base;
    to_limit = old_from_limit;
    free_ptr = new_free;
}

/// Live bytes currently occupied in the active semispace (`free_ptr - from_base`).
/// Test/diagnostic hook (not part of the ABI).
pub fn heapUsed() usize {
    return if (from_base == 0) 0 else free_ptr - from_base;
}
