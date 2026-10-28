//! Also compiled as a WASM cdylib by tests/run-collector.sh.
use lo_runtime::*;
use std::ptr::null_mut;

#[repr(C)]
struct Node {
    header: Object,
    next: *mut Object,
}
static OFFSETS: [u32; 1] = [std::mem::offset_of!(Node, next) as u32];
static CLASS: ClassDescriptor = ClassDescriptor {
    name: c"Node".as_ptr().cast(),
    name_len: 4,
    parent: std::ptr::null(),
    instance_size: std::mem::size_of::<Node>() as u32,
    pointer_offsets: OFFSETS.as_ptr(),
    pointer_count: 1,
    vtable_size: 0,
    vtable: std::ptr::null(),
};
#[repr(C)]
struct Frame {
    parent: *mut ShadowFrame,
    count: u32,
    roots: [*mut Object; 3],
}
impl Frame {
    fn new() -> Self {
        Self {
            parent: null_mut(),
            count: 3,
            roots: [null_mut(); 3],
        }
    }
    unsafe fn push(&mut self) {
        lo_push_frame((self as *mut Self).cast());
    }
}

#[no_mangle]
pub extern "C" fn lo_entry() -> i32 {
    unsafe {
        lo_runtime_init();
        let mut parent = Frame::new();
        parent.push();
        lo_alloc(&CLASS); // gap before the live cycle
        parent.roots[0] = lo_alloc(&CLASS);
        parent.roots[1] = lo_alloc(&CLASS);
        lo_gc_write_barrier(parent.roots[0], OFFSETS[0], parent.roots[1]);
        lo_gc_write_barrier(parent.roots[1], OFFSETS[0], parent.roots[0]);
        (*parent.roots[0]).flags = 73;
        let mut child = Frame::new();
        child.roots[0] = parent.roots[0];
        child.roots[1] = (&LO_EMPTY_STRING as *const StringObject).cast_mut().cast();
        child.push();
        let old = parent.roots[0];
        for _ in 0..3 {
            lo_gc_collect();
            assert_eq!(child.roots[0], parent.roots[0]);
            assert_eq!((*(parent.roots[0] as *const Node)).next, parent.roots[1]);
            assert_eq!((*(parent.roots[1] as *const Node)).next, parent.roots[0]);
            assert_eq!((*parent.roots[0]).flags, 73);
            assert_eq!((*parent.roots[0]).gc_bits, 0);
            assert!(child.roots[2].is_null());
            assert_eq!(
                child.roots[1],
                (&LO_EMPTY_STRING as *const StringObject).cast_mut().cast()
            );
        }
        assert_ne!(old, parent.roots[0]);
        lo_pop_frame();
        // A string larger than the gap requires an overlapping slide.
        lo_alloc(&CLASS);
        let bytes = [b'x'; 257];
        parent.roots[2] = lo_string_new(bytes.as_ptr(), bytes.len() as u32);
        let old_string = parent.roots[2];
        lo_gc_collect();
        assert_ne!(old_string, parent.roots[2]);
        let data = parent.roots[2].cast::<u8>().add(string_data_offset());
        assert_eq!(std::slice::from_raw_parts(data, bytes.len()), bytes);
        // Exceed the default heap in total allocations while retaining the graph.
        for _ in 0..1_100_000 {
            lo_alloc(&CLASS);
        }
        lo_gc_collect();
        assert_eq!((*(parent.roots[0] as *const Node)).next, parent.roots[1]);
        assert_eq!(
            std::slice::from_raw_parts(
                parent.roots[2].cast::<u8>().add(string_data_offset()),
                bytes.len()
            ),
            bytes
        );
        parent.roots.fill(null_mut());
        lo_gc_collect();
        assert_eq!(heap_used(), 0);
        let fresh = lo_alloc(&CLASS);
        assert!((*(fresh as *const Node)).next.is_null());
        assert_eq!((*fresh).flags, 0);
        lo_pop_frame();
        lo_runtime_shutdown();
    }
    0
}

#[test]
fn collector_stress() {
    assert_eq!(lo_entry(), 0);
}

#[test]
fn live_set_overflow_aborts() {
    if std::env::var_os("LO_TEST_OOM_CHILD").is_some() {
        unsafe {
            lo_runtime_init();
            let mut frame = Frame::new();
            frame.push();
            loop {
                let node = lo_alloc(&CLASS);
                lo_gc_write_barrier(node, OFFSETS[0], frame.roots[0]);
                frame.roots[0] = node;
            }
        }
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "live_set_overflow_aborts", "--nocapture"])
        .env("LO_TEST_OOM_CHILD", "1")
        .env("LO_HEAP_SIZE", "1024")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(137));
    assert_eq!(
        String::from_utf8(result.stderr).unwrap().trim(),
        "lo_alloc: out of memory"
    );
}
