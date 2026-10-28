//! Lisp-2 mark-compact: mark, compute destinations, fix references, then slide.
//! Descriptors stay intact; forwarding addresses live in preallocated side data.
use crate::compaction::{Relocation, Scratch};
use crate::object::{shadow_frame_roots_offset, Object};

const MARKED: u32 = 1;
static mut IN_COLLECTION: bool = false;
static mut POISON: bool = false;

/// Read debug configuration before collection, when host allocation is allowed.
pub(crate) fn init() {
    let poison = std::env::var("LO_GC_POISON")
        .map(|s| !s.is_empty() && s != "0")
        .unwrap_or(false);
    unsafe {
        IN_COLLECTION = false;
        POISON = poison;
    }
}

/// Non-generational barrier: the helper performs the store itself.
///
/// # Safety
/// Object and reference field must be valid; value is an object or null.
#[no_mangle]
pub unsafe extern "C" fn lo_gc_write_barrier(
    obj: *mut Object,
    field_offset: u32,
    value: *mut Object,
) {
    *((obj as *mut u8).add(field_offset as usize) as *mut *mut Object) = value;
}

/// Force collection. The runtime is single-threaded; no allocation occurs here.
#[no_mangle]
pub extern "C" fn lo_gc_collect() {
    unsafe {
        assert!(!IN_COLLECTION, "recursive garbage collection");
        let (base, end, limit) = crate::alloc::heap_bounds();
        if base.is_null() {
            return;
        }
        IN_COLLECTION = true;
        let scratch = crate::compaction::scratch();
        scratch.clear();
        mark(base, end, scratch);
        let live_bytes = compute_forwarding(base, end, scratch);
        fix_references(base, end, scratch);
        slide(base, scratch);
        let new_end = base.add(live_bytes);
        crate::alloc::set_free(new_end);
        if POISON {
            core::ptr::write_bytes(new_end, 0xEF, limit as usize - new_end as usize);
        }
        scratch.clear();
        IN_COLLECTION = false;
    }
}

// The ABI supplies object-start references, never interior pointers.
// Null and immortal objects outside the allocated heap are left unchanged.
fn managed(object: *mut Object, base: *mut u8, end: *mut u8) -> bool {
    !object.is_null() && object as usize >= base as usize && (object as usize) < end as usize
}

unsafe fn roots(mut visit: impl FnMut(*mut *mut Object)) {
    let mut frame = crate::shadow_stack::current_frame();
    while !frame.is_null() {
        let slots = (frame as *mut u8).add(shadow_frame_roots_offset()) as *mut *mut Object;
        for i in 0..(*frame).num_roots as usize {
            visit(slots.add(i));
        }
        frame = (*frame).parent;
    }
}

unsafe fn fields(object: *mut Object, mut visit: impl FnMut(*mut *mut Object)) {
    let descriptor = (*object).class_descriptor;
    for i in 0..(*descriptor).pointer_count as usize {
        let offset = *(*descriptor).pointer_offsets.add(i) as usize;
        visit((object as *mut u8).add(offset) as *mut *mut Object);
    }
}

unsafe fn enqueue(object: *mut Object, base: *mut u8, end: *mut u8, scratch: &mut Scratch) {
    if managed(object, base, end) && (*object).gc_bits & MARKED == 0 {
        // Mark before enqueueing so cycles and aliases enter the worklist once.
        (*object).gc_bits |= MARKED;
        scratch.push_work(object as usize - base as usize);
    }
}

unsafe fn mark(base: *mut u8, end: *mut u8, scratch: &mut Scratch) {
    roots(|slot| enqueue(*slot, base, end, scratch));
    while let Some(offset) = scratch.worklist.pop() {
        fields(base.add(offset) as *mut Object, |slot| {
            enqueue(*slot, base, end, scratch)
        });
    }
}

unsafe fn compute_forwarding(base: *mut u8, end: *mut u8, scratch: &mut Scratch) -> usize {
    let allocated = end as usize - base as usize;
    let mut offset = 0;
    let mut destination = 0;
    while offset < allocated {
        let object = base.add(offset) as *mut Object;
        let size = crate::heap_layout::object_size(object).expect("invalid object size");
        assert!(
            size >= core::mem::size_of::<Object>() && size <= allocated - offset,
            "invalid heap object boundary"
        );
        if (*object).gc_bits & MARKED != 0 {
            scratch.push_relocation(Relocation {
                old_offset: offset,
                new_offset: destination,
                size,
            });
            destination += size;
        }
        offset += size;
    }
    destination
}

unsafe fn relocated(
    object: *mut Object,
    base: *mut u8,
    end: *mut u8,
    scratch: &Scratch,
) -> *mut Object {
    if !managed(object, base, end) {
        return object;
    }
    let destination = scratch
        .destination(object as usize - base as usize)
        .expect("live reference has no forwarding address");
    base.add(destination) as *mut Object
}

unsafe fn fix_references(base: *mut u8, end: *mut u8, scratch: &Scratch) {
    // Fix all old locations before any move can overwrite a header or field.
    roots(|slot| *slot = relocated(*slot, base, end, scratch));
    for relocation in &scratch.relocations {
        fields(base.add(relocation.old_offset) as *mut Object, |slot| {
            *slot = relocated(*slot, base, end, scratch);
        });
    }
}

unsafe fn slide(base: *mut u8, scratch: &Scratch) {
    // Ascending order preserves unread objects. ptr::copy permits overlap.
    for relocation in &scratch.relocations {
        let destination = base.add(relocation.new_offset);
        core::ptr::copy(
            base.add(relocation.old_offset),
            destination,
            relocation.size,
        );
        (*(destination as *mut Object)).gc_bits &= !MARKED;
    }
}

#[cfg(test)]
mod tests {
    //! Collector tests. Run in-crate so they
    //! can reach the internal string allocator and heap-occupancy hook.
    //!
    //! The runtime is process-global single-threaded state, so these tests
    //! serialize through a mutex and bracket each body with init/shutdown. The
    //! `LO_HEAP_SIZE` override (read by `heap_init`) is set inside the lock so the
    //! allocation-pressure test cannot race another test's init.

    use std::sync::Mutex;

    use crate::alloc::{bump_alloc_string, heap_used, lo_alloc};
    use crate::descriptors::{LO_EMPTY_STRING, LO_STRING_CLASS};
    use crate::object::{string_data_offset, ClassDescriptor, Object, ShadowFrame, StringObject};
    use crate::shadow_stack::{lo_pop_frame, lo_push_frame};
    use crate::{lo_gc_collect, lo_runtime_init, lo_runtime_shutdown};

    static LOCK: Mutex<()> = Mutex::new(());

    #[repr(C)]
    struct Node {
        header: Object,
        next: *mut Object,
    }
    const NEXT: usize = core::mem::offset_of!(Node, next);
    static NODE_PTR_OFFSETS: [u32; 1] = [NEXT as u32];
    static NODE_CLASS: ClassDescriptor = ClassDescriptor {
        name: c"Node".as_ptr() as *const u8,
        name_len: 4,
        parent: core::ptr::null(),
        instance_size: core::mem::size_of::<Node>() as u32,
        pointer_offsets: &NODE_PTR_OFFSETS as *const u32,
        pointer_count: 1,
        vtable_size: 0,
        vtable: core::ptr::null(),
    };

    /// A shadow-stack frame with `N` inline roots, byte-compatible with the ABI's
    /// `ShadowFrame { parent, num_roots, roots: [*mut Object; N] }`.
    #[repr(C)]
    struct TestFrame<const N: usize> {
        parent: *mut ShadowFrame,
        num_roots: u32,
        roots: [*mut Object; N],
    }

    impl<const N: usize> TestFrame<N> {
        fn new() -> Self {
            TestFrame {
                parent: core::ptr::null_mut(),
                num_roots: N as u32,
                roots: [core::ptr::null_mut(); N],
            }
        }
        fn as_frame(&mut self) -> *mut ShadowFrame {
            self as *mut TestFrame<N> as *mut ShadowFrame
        }
    }

    /// Run `f` with the runtime up (optionally with a small heap to force GC),
    /// serialized and bracketed by init/shutdown.
    fn with_runtime<T>(heap_size: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        match heap_size {
            Some(s) => std::env::set_var("LO_HEAP_SIZE", s),
            None => std::env::remove_var("LO_HEAP_SIZE"),
        }
        lo_runtime_init();
        let result = f();
        lo_runtime_shutdown();
        std::env::remove_var("LO_HEAP_SIZE");
        result
    }

    unsafe fn set_field(obj: *mut Object, off: usize, val: *mut Object) {
        *((obj as *mut u8).add(off) as *mut *mut Object) = val;
    }
    unsafe fn get_field(obj: *mut Object, off: usize) -> *mut Object {
        *((obj as *mut u8).add(off) as *mut *mut Object)
    }

    // (i) A known live graph survives: reachable objects are preserved with
    // updated addresses, and both the root slot and the interior pointer field
    // are rewritten to the new locations.
    #[test]
    fn live_graph_preserved_and_pointers_rewritten() {
        with_runtime(None, || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            lo_alloc(&NODE_CLASS); // dead prefix forces actual relocation
            let a = lo_alloc(&NODE_CLASS);
            let b = lo_alloc(&NODE_CLASS);
            set_field(a, NEXT, b); // a.next = b
            frame.roots[0] = a; // root = a

            lo_gc_collect();

            let a2 = frame.roots[0];
            assert_ne!(a2, a, "root must be rewritten to the moved object");
            assert_eq!((*a2).class_descriptor, &NODE_CLASS as *const _);

            let b2 = get_field(a2, NEXT);
            assert!(!b2.is_null(), "interior pointer must survive");
            assert_ne!(b2, b, "interior pointer must be rewritten to b's new addr");
            assert_eq!((*b2).class_descriptor, &NODE_CLASS as *const _);
            assert!(get_field(b2, NEXT).is_null(), "b.next was null");

            lo_pop_frame();
        });
    }

    // (ii) An unreachable object is reclaimed: heap occupancy drops by its size.
    #[test]
    fn unreachable_object_reclaimed() {
        with_runtime(None, || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            let live = lo_alloc(&NODE_CLASS);
            frame.roots[0] = live;
            let _dead = lo_alloc(&NODE_CLASS); // not rooted -> unreachable

            let before = heap_used();
            assert_eq!(before, 2 * core::mem::size_of::<Node>());

            lo_gc_collect();

            let after = heap_used();
            assert_eq!(after, core::mem::size_of::<Node>());
            assert!(after < before);

            // The survivor's root was rewritten to its moved location and is
            // still a valid Node. (Also makes the root store observably-live to
            // the compiler, which cannot see the collector read it via the
            // registered shadow-stack pointer.)
            let live2 = frame.roots[0];
            assert_eq!((*live2).class_descriptor, &NODE_CLASS as *const _);

            lo_pop_frame();
        });
    }

    // (iii) A variable-size String survives a collection intact (bytes round-trip).
    #[test]
    fn string_survives_collection() {
        with_runtime(None, || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            lo_alloc(&NODE_CLASS);
            let bytes: &[u8] = b"hello, Lisp-2";
            let s = bump_alloc_string(bytes.len() as u32);
            let data = (s as *mut u8).add(string_data_offset());
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            frame.roots[0] = s;

            lo_gc_collect();

            let s2 = frame.roots[0];
            assert_ne!(s2, s, "string must slide into the dead prefix");
            assert_eq!((*s2).class_descriptor, &LO_STRING_CLASS as *const _);
            let so = s2 as *const StringObject;
            assert_eq!((*so).length, bytes.len() as u32);
            let data2 = (s2 as *const u8).add(string_data_offset());
            let round = core::slice::from_raw_parts(data2, bytes.len());
            assert_eq!(
                round, bytes,
                "string bytes must round-trip through the copy"
            );

            lo_pop_frame();
        });
    }

    // (iv) LO_EMPTY_STRING is immortal: a moved object's field that references it
    // still points at the same `.rodata` address after collection.
    #[test]
    fn empty_string_not_moved() {
        with_runtime(None, || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            lo_alloc(&NODE_CLASS);
            let holder = lo_alloc(&NODE_CLASS);
            let empty = &LO_EMPTY_STRING as *const StringObject as *mut Object;
            set_field(holder, NEXT, empty);
            frame.roots[0] = holder;

            lo_gc_collect();

            let holder2 = frame.roots[0];
            assert_ne!(holder2, holder, "holder itself moves");
            let field = get_field(holder2, NEXT);
            assert_eq!(
                field, empty,
                "immortal LO_EMPTY_STRING must not be relocated"
            );

            lo_pop_frame();
        });
    }

    // (v) Allocation pressure triggers GC through lo_alloc's
    // OOM path and continues; only a genuine live-set overflow would abort.
    #[test]
    fn allocation_pressure_triggers_gc() {
        // Dead nodes force several collections; the rooted node survives them all.
        with_runtime(Some("4096"), || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());

            let live = lo_alloc(&NODE_CLASS);
            frame.roots[0] = live;

            for _ in 0..600 {
                // Not rooted: each becomes garbage the next collection reclaims.
                let _dead = lo_alloc(&NODE_CLASS);
            }

            // Reaching here means GC repeatedly reclaimed and allocation continued
            // (a missing GC trigger would have aborted with exit 137, killing the
            // test process). The rooted node survived every collection.
            let live2 = frame.roots[0];
            assert_eq!((*live2).class_descriptor, &NODE_CLASS as *const _);

            lo_pop_frame();
        });
    }

    #[test]
    fn live_set_can_use_more_than_half_the_heap() {
        with_runtime(Some("4096"), || unsafe {
            let mut frame = TestFrame::<1>::new();
            lo_push_frame(frame.as_frame());
            let count = 3072 / core::mem::size_of::<Node>();
            for _ in 0..count {
                let node = lo_alloc(&NODE_CLASS);
                set_field(node, NEXT, frame.roots[0]);
                frame.roots[0] = node;
            }
            lo_gc_collect();
            assert!(heap_used() > 2048);
            let mut node = frame.roots[0];
            for _ in 0..count {
                assert!(!node.is_null());
                node = get_field(node, NEXT);
            }
            assert!(node.is_null());
            lo_pop_frame();
        });
    }
}
