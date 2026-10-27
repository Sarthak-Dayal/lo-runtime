use lo_runtime::*;

unsafe fn text(p: *mut Object) -> String {
    let len = (*(p as *const StringObject)).length as usize;
    String::from_utf8(std::slice::from_raw_parts((p as *const u8).add(string_data_offset()), len).to_vec()).unwrap()
}

#[test]
fn unicode_and_type_operations() {
    // This integration binary has one test: runtime globals cannot race.
    unsafe {
        lo_runtime_init();
        let input = "aé🙂";
        let s = lo_string_new(input.as_ptr(), input.len() as u32);
        assert_eq!(text(lo_string_reverse(s)), "🙂éa");
        assert_eq!(text(lo_string_repeat(s, 2)), "aé🙂aé🙂");
        assert_eq!(text(lo_string_concat(s, s)), "aé🙂aé🙂");
        assert_eq!(lo_string_compare(s, s), 0);
        let empty = lo_string_new(std::ptr::null(), 0);
        assert_eq!(text(lo_string_repeat(empty, i32::MAX)), "");
        assert!(lo_instanceof(s, &LO_STRING_CLASS));
        assert_eq!(lo_cast_check(s, &LO_STRING_CLASS), s);
        assert!(!lo_instanceof(std::ptr::null_mut(), &LO_STRING_CLASS));
        assert!(lo_cast_check(std::ptr::null_mut(), &LO_STRING_CLASS).is_null());
        #[repr(C)]
        struct Frame { parent: *mut ShadowFrame, count: u32, roots: [*mut Object; 1] }
        let mut frame = Frame { parent: std::ptr::null_mut(), count: 1, roots: [s] };
        lo_push_frame(&mut frame as *mut Frame as *mut ShadowFrame);
        lo_gc_collect();
        let moved = frame.roots[0];
        assert_ne!(moved, s);
        assert_eq!(text(moved), input);
        assert_eq!(text(lo_string_reverse(moved)), "🙂éa");
        assert_eq!(lo_cast_check(moved, &LO_STRING_CLASS), moved);
        lo_pop_frame();
        lo_runtime_shutdown();
    }
}
