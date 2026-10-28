//! One-region bump allocator backing the Lisp-2 mark-compact collector.
//!
//! Total heap size inherits the WS-1 mechanism (decision (5c)): a 16 MiB default,
//! overridable at `lo_runtime_init` via the `LO_HEAP_SIZE` environment variable;
//! Setting `LO_HEAP_SIZE` small makes collection deterministic in stress tests.
//!
//! The heap pointers are `static mut` raw scalars (the runbook sanctions
//! `static mut` for the bump pointer). Every access is by value or through a raw
//! pointer — never `&`/`&mut` of the statics — which keeps this module clear of
//! the `static_mut_refs` lint. Invariant: only the functions here and the
//! collector's commit touch these, and only on the single runtime
//! thread.

use core::alloc::Layout;
use core::ptr;

use crate::descriptors::LO_STRING_CLASS;
use crate::exit_codes::EXIT_OOM;
use crate::heap_layout::{aligned_size, oom, string_size, OBJECT_ALIGN};
use crate::object::{ClassDescriptor, Object, StringObject};

/// Default total heap size (16 MiB), overridable via `LO_HEAP_SIZE` at init.
const DEFAULT_HEAP_SIZE: usize = 16 * 1024 * 1024;
const HEAP_ALIGN: usize = OBJECT_ALIGN;

/// Base of the whole backing region (for `dealloc`); null when uninitialized.
static mut HEAP_BASE: *mut u8 = ptr::null_mut();
/// Layout the region was allocated with (for `dealloc`).
static mut HEAP_LAYOUT: Option<Layout> = None;

/// Managed region `[HEAP_BASE, HEAP_LIMIT)`, allocated prefix ending at `FREE`.
static mut HEAP_LIMIT: *mut u8 = ptr::null_mut();
static mut FREE: *mut u8 = ptr::null_mut();

#[inline]
const fn align_down(n: usize, align: usize) -> usize {
    n & !(align - 1)
}

/// Allocate the single compactable heap region.
pub(crate) fn heap_init() {
    let total = std::env::var("LO_HEAP_SIZE")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= HEAP_ALIGN)
        .unwrap_or(DEFAULT_HEAP_SIZE);
    let region = align_down(total, HEAP_ALIGN);
    let layout = Layout::from_size_align(region, HEAP_ALIGN).unwrap_or_else(|_| oom());
    // SAFETY: `layout` has non-zero size; `alloc_zeroed` returns either a valid
    // zeroed region of `region` bytes or null (handled). Writes below are
    // by-value stores to the allocator statics on the single runtime thread.
    unsafe {
        let base = std::alloc::alloc_zeroed(layout);
        if base.is_null() {
            oom();
        }
        HEAP_BASE = base;
        HEAP_LAYOUT = Some(layout);
        HEAP_LIMIT = base.add(region);
        FREE = base;
    }
    crate::compaction::init(region);
}

/// Release the heap region. Called from `lo_runtime_shutdown`.
pub(crate) fn heap_shutdown() {
    crate::compaction::shutdown();
    // SAFETY: by-value reads of the statics; `dealloc` is paired with the
    // `alloc_zeroed` in `heap_init` using the stored layout. Idempotent: a null
    // base (never inited, or already shut down) is a no-op.
    unsafe {
        let base = HEAP_BASE;
        let layout = HEAP_LAYOUT;
        if !base.is_null() {
            if let Some(layout) = layout {
                std::alloc::dealloc(base, layout);
            }
            HEAP_BASE = ptr::null_mut();
            HEAP_LAYOUT = None;
            HEAP_LIMIT = ptr::null_mut();
            FREE = ptr::null_mut();
        }
    }
}

/// Carve `size` zero-initialized bytes from the allocated prefix and stamp the
/// object header. On a no-fit, forces one collection and retries; aborts (exit
/// 137) only if the live set still leaves no room (runbook WS-2 §2.1, ABI §3.1).
///
/// # Safety
/// `class` must point at a valid, live `ClassDescriptor`. `size` must be at least
/// `size_of::<Object>()` so the header fits, and aligned to `OBJECT_ALIGN`.
unsafe fn alloc_raw(size: usize, class: *const ClassDescriptor) -> *mut Object {
    let bump = bump_if_fits(size);
    let slot = if !bump.is_null() {
        bump
    } else {
        // OOM path: collect, then retry the bump exactly once.
        crate::gc::lo_gc_collect();
        let retry = bump_if_fits(size);
        if retry.is_null() {
            crate::abort::runtime_abort("lo_alloc: out of memory", EXIT_OOM);
        }
        retry
    };

    ptr::write_bytes(slot, 0, size);
    let obj = slot as *mut Object;
    (*obj).class_descriptor = class;
    (*obj).gc_bits = 0;
    (*obj).flags = 0;
    obj
}

/// Try to bump `size` bytes off the managed region. Returns the carved slot on
/// success, or null if it does not fit (or the heap is uninitialized).
///
/// # Safety
/// Reads/writes the allocator statics on the single runtime thread.
unsafe fn bump_if_fits(size: usize) -> *mut u8 {
    let bump = FREE;
    let limit = HEAP_LIMIT;
    // Address arithmetic for the bounds check avoids forming an out-of-bounds
    // pointer when the request would overflow the heap.
    let Some(new_addr) = (bump as usize).checked_add(size) else {
        return ptr::null_mut();
    };
    if bump.is_null() || new_addr > limit as usize {
        return ptr::null_mut();
    }
    FREE = bump.add(size);
    bump
}

/// Allocate a zero-initialized object of `class.instance_size` bytes
/// (`runtime-abi.md` §3.1). Fills in the header. Triggers a collection and
/// retries on a full heap; aborts (exit 137) only when the request still cannot fit.
///
/// Codegen is responsible for finalizing String-typed fields to `LO_EMPTY_STRING`
/// immediately after this returns (see `runtime-abi.md` §3.1); `lo_alloc` is
/// deliberately type-agnostic.
///
/// # Safety
/// `class` must point at a valid `ClassDescriptor` whose `instance_size` is at
/// least `size_of::<Object>()`.
#[no_mangle]
pub unsafe extern "C" fn lo_alloc(class: *const ClassDescriptor) -> *mut Object {
    let size = aligned_size((*class).instance_size as usize).unwrap_or_else(|| oom());
    alloc_raw(size, class)
}

/// Allocate a `StringObject` with room for `len` inline bytes, header stamped to
/// `LO_STRING_CLASS` and `length` set. The caller fills the `len` data bytes
/// (already zeroed) at `base + string_data_offset()`.
///
/// This is the internal variable-size string allocator used by the provided
/// `lo_read_string` and by the team-implemented `lo_string_*` ops. `lo_alloc`
/// cannot size a string itself (it only knows the class's fixed `instance_size`).
///
/// # Safety
/// Must be called after `heap_init`. Returns a valid `*mut Object`/`*mut
/// StringObject` or aborts on OOM.
pub(crate) unsafe fn bump_alloc_string(len: u32) -> *mut Object {
    let size = string_size(len as usize).unwrap_or_else(|| oom());
    let obj = alloc_raw(size, &LO_STRING_CLASS);
    let so = obj as *mut StringObject;
    (*so).length = len;
    obj
}

/// `(base, allocated_end, limit)` for the collector.
pub(crate) unsafe fn heap_bounds() -> (*mut u8, *mut u8, *mut u8) {
    (HEAP_BASE, FREE, HEAP_LIMIT)
}

/// Install the end of the compacted live prefix.
pub(crate) unsafe fn set_free(new_free: *mut u8) {
    debug_assert!(new_free >= HEAP_BASE && new_free <= HEAP_LIMIT);
    FREE = new_free;
}

/// Live bytes currently occupied in the managed region.
///
/// Test/diagnostic hook (not part of the ABI): lets the GC tests observe heap
/// occupancy drop after a collection reclaims dead objects.
#[doc(hidden)]
pub fn heap_used() -> usize {
    // SAFETY: by-value reads of the allocator statics on the single runtime
    // thread. Returns 0 before init (both null).
    unsafe {
        if HEAP_BASE.is_null() {
            0
        } else {
            (FREE as usize) - (HEAP_BASE as usize)
        }
    }
}
