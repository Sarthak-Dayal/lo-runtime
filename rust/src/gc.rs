//! Lisp-2 mark-compact: mark, compute destinations, fix references, then slide.
//! Descriptors stay intact; forwarding addresses live in preallocated side data.
//!
//! Safety: collection requires exclusive runtime access, valid object layouts and
//! descriptors, and writable root/reference slots containing object starts or null.
use crate::compaction::{Relocation, Scratch};
use crate::object::{shadow_frame_roots_offset, Object, ShadowFrame};

const MARKED: u32 = 1;
struct CollectorState {
    in_collection: bool,
    poison: bool,
}

static mut STATE: CollectorState = CollectorState {
    in_collection: false,
    poison: false,
};

/// Read debug configuration before collection, when host allocation is allowed.
pub(crate) fn init() {
    let poison = std::env::var("LO_GC_POISON")
        .map(|s| !s.is_empty() && s != "0")
        .unwrap_or(false);
    unsafe {
        STATE.in_collection = false;
        STATE.poison = poison;
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
    *reference_slot_at(obj, field_offset as usize) = value;
}

/// Force collection. The runtime is single-threaded; no allocation occurs here.
#[no_mangle]
pub extern "C" fn lo_gc_collect() {
    unsafe {
        assert!(!STATE.in_collection, "recursive garbage collection");
        let (base, end, limit) = crate::alloc::heap_bounds();
        if base.is_null() {
            return;
        }
        STATE.in_collection = true;
        let scratch = crate::compaction::scratch();
        scratch.clear();
        mark(base, end, scratch);
        let live_bytes = compute_forwarding(base, end, scratch);
        fix_references(base, end, scratch);
        slide(base, scratch);
        let new_end = base.add(live_bytes);
        crate::alloc::set_free(new_end);
        if STATE.poison {
            core::ptr::write_bytes(new_end, 0xEF, limit as usize - new_end as usize);
        }
        scratch.clear();
        STATE.in_collection = false;
    }
}

// Requires a writable, aligned reference field at this byte offset.
unsafe fn reference_slot_at(object: *mut Object, offset: usize) -> *mut *mut Object {
    object.cast::<u8>().add(offset).cast()
}

struct RootSlots {
    frame: *mut ShadowFrame,
    index: usize,
}

impl RootSlots {
    // Registered frames and their inline root arrays must remain valid.
    unsafe fn new() -> Self {
        Self {
            frame: crate::shadow_stack::current_frame(),
            index: 0,
        }
    }
}

impl Iterator for RootSlots {
    type Item = *mut *mut Object;

    fn next(&mut self) -> Option<Self::Item> {
        // SAFETY: construction requires valid frames; iteration only reads their headers.
        unsafe {
            while !self.frame.is_null() {
                if self.index < (*self.frame).num_roots as usize {
                    let slots = self
                        .frame
                        .cast::<u8>()
                        .add(shadow_frame_roots_offset())
                        .cast::<*mut Object>();
                    let slot = slots.add(self.index);
                    self.index += 1;
                    return Some(slot);
                }
                self.frame = (*self.frame).parent;
                self.index = 0;
            }
            None
        }
    }
}

struct ReferenceFields {
    object: *mut Object,
    offsets: *const u32,
    count: usize,
    index: usize,
}

impl ReferenceFields {
    // Object and descriptor must remain valid, with writable reference fields.
    unsafe fn new(object: *mut Object) -> Self {
        let descriptor = (*object).class_descriptor;
        Self {
            object,
            offsets: (*descriptor).pointer_offsets,
            count: (*descriptor).pointer_count as usize,
            index: 0,
        }
    }
}

impl Iterator for ReferenceFields {
    type Item = *mut *mut Object;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index == self.count {
            return None;
        }
        // SAFETY: construction requires valid offsets and reference fields.
        unsafe {
            let offset = *self.offsets.add(self.index) as usize;
            self.index += 1;
            Some(reference_slot_at(self.object, offset))
        }
    }
}

fn is_in_allocated_heap(object: *mut Object, base: *mut u8, end: *mut u8) -> bool {
    !object.is_null() && object as usize >= base as usize && (object as usize) < end as usize
}

fn offset_of_object(object: *mut Object, base: *mut u8) -> usize {
    object as usize - base as usize
}

unsafe fn object_at_offset(base: *mut u8, offset: usize) -> *mut Object {
    base.add(offset).cast()
}

unsafe fn mark_and_enqueue(
    object: *mut Object,
    base: *mut u8,
    end: *mut u8,
    scratch: &mut Scratch,
) {
    if is_in_allocated_heap(object, base, end) && (*object).gc_bits & MARKED == 0 {
        // Mark before enqueueing so cycles and aliases enter the worklist once.
        (*object).gc_bits |= MARKED;
        scratch.push_work(offset_of_object(object, base));
    }
}

unsafe fn mark(base: *mut u8, end: *mut u8, scratch: &mut Scratch) {
    for slot in RootSlots::new() {
        mark_and_enqueue(*slot, base, end, scratch);
    }
    while let Some(offset) = scratch.worklist.pop() {
        for slot in ReferenceFields::new(object_at_offset(base, offset)) {
            mark_and_enqueue(*slot, base, end, scratch);
        }
    }
}

unsafe fn compute_forwarding(base: *mut u8, end: *mut u8, scratch: &mut Scratch) -> usize {
    let allocated = end as usize - base as usize;
    let mut offset = 0;
    let mut destination = 0;
    while offset < allocated {
        let object = object_at_offset(base, offset);
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

unsafe fn forwarded_address(
    object: *mut Object,
    base: *mut u8,
    end: *mut u8,
    scratch: &Scratch,
) -> *mut Object {
    // Null and immortal objects outside the allocated heap are left unchanged.
    if !is_in_allocated_heap(object, base, end) {
        return object;
    }
    let destination = scratch
        .destination(offset_of_object(object, base))
        .expect("live reference has no forwarding address");
    object_at_offset(base, destination)
}

unsafe fn fix_references(base: *mut u8, end: *mut u8, scratch: &Scratch) {
    // Fix all old locations before any move can overwrite a header or field.
    for slot in RootSlots::new() {
        *slot = forwarded_address(*slot, base, end, scratch);
    }
    for relocation in &scratch.relocations {
        let object = object_at_offset(base, relocation.old_offset);
        for slot in ReferenceFields::new(object) {
            *slot = forwarded_address(*slot, base, end, scratch);
        }
    }
}

unsafe fn slide(base: *mut u8, scratch: &Scratch) {
    // Ascending order preserves unread objects. ptr::copy permits overlap.
    for relocation in &scratch.relocations {
        let source = object_at_offset(base, relocation.old_offset);
        let destination = object_at_offset(base, relocation.new_offset);
        core::ptr::copy(
            source.cast::<u8>(),
            destination.cast::<u8>(),
            relocation.size,
        );
        (*destination).gc_bits &= !MARKED;
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

    unsafe fn set_next(object: *mut Object, next: *mut Object) {
        (*object.cast::<Node>()).next = next;
    }

    unsafe fn next(object: *mut Object) -> *mut Object {
        (*object.cast::<Node>()).next
    }

    unsafe fn string_data(object: *mut Object) -> *mut u8 {
        object.cast::<u8>().add(string_data_offset())
    }

    unsafe fn string_bytes(string: &StringObject) -> &[u8] {
        let data = (string as *const StringObject)
            .cast::<u8>()
            .add(string_data_offset());
        core::slice::from_raw_parts(data, string.length as usize)
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
            set_next(a, b); // a.next = b
            frame.roots[0] = a; // root = a

            lo_gc_collect();

            let a2 = frame.roots[0];
            assert_ne!(a2, a, "root must be rewritten to the moved object");
            assert_eq!((*a2).class_descriptor, &NODE_CLASS as *const _);

            let b2 = next(a2);
            assert!(!b2.is_null(), "interior pointer must survive");
            assert_ne!(b2, b, "interior pointer must be rewritten to b's new addr");
            assert_eq!((*b2).class_descriptor, &NODE_CLASS as *const _);
            assert!(next(b2).is_null(), "b.next was null");

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
            let data = string_data(s);
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            frame.roots[0] = s;

            lo_gc_collect();

            let s2 = frame.roots[0];
            assert_ne!(s2, s, "string must slide into the dead prefix");
            assert_eq!((*s2).class_descriptor, &LO_STRING_CLASS as *const _);
            let so = s2 as *const StringObject;
            assert_eq!((*so).length, bytes.len() as u32);
            let round = string_bytes(&*so);
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
            set_next(holder, empty);
            frame.roots[0] = holder;

            lo_gc_collect();

            let holder2 = frame.roots[0];
            assert_ne!(holder2, holder, "holder itself moves");
            let field = next(holder2);
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
                set_next(node, frame.roots[0]);
                frame.roots[0] = node;
            }
            lo_gc_collect();
            assert!(heap_used() > 2048);
            let mut node = frame.roots[0];
            for _ in 0..count {
                assert!(!node.is_null());
                node = next(node);
            }
            assert!(node.is_null());
            lo_pop_frame();
        });
    }
}
