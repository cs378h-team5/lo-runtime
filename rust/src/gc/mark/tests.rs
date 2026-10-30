use super::*;
use crate::{
    alloc::{heap_used, lo_alloc},
    descriptors::{LO_EMPTY_STRING, LO_STRING_CLASS},
    gc::{
        lo_gc_write_barrier,
        test_support::{with_heap, Frame},
    },
    object::{ClassDescriptor, StringObject},
    shadow_stack::lo_pop_frame,
};
use core::mem::{offset_of, size_of};

#[repr(C)]
struct Node {
    header: Object,
    left: *mut Object,
    scalar: usize,
    right: *mut Object,
}

static POINTER_OFFSETS: [u32; 2] = [
    offset_of!(Node, left) as u32,
    offset_of!(Node, right) as u32,
];
static NODE_CLASS: ClassDescriptor = ClassDescriptor {
    name: c"Node".as_ptr().cast(),
    name_len: 4,
    parent: ptr::null(),
    instance_size: size_of::<Node>() as u32,
    pointer_offsets: POINTER_OFFSETS.as_ptr(),
    pointer_count: 2,
    vtable_size: 0,
    vtable: ptr::null(),
};
static LEAF_CLASS: ClassDescriptor = ClassDescriptor {
    name: c"Leaf".as_ptr().cast(),
    name_len: 4,
    parent: ptr::null(),
    instance_size: size_of::<Object>() as u32,
    pointer_offsets: ptr::null(),
    pointer_count: 0,
    vtable_size: 0,
    vtable: ptr::null(),
};

unsafe fn node() -> *mut Object {
    lo_alloc(&NODE_CLASS)
}

#[test]
fn marks_cycles_sharing_caller_roots_and_strings_without_rewriting() {
    with_heap(4096, || unsafe {
        let a = node();
        let b = node();
        let shared = node();
        let caller_only = node();
        let scalar_only = node();
        let dead_a = node();
        let dead_b = node();
        let string = alloc::bump_alloc_string(9);
        let bytes = string.cast::<u8>().add(crate::object::string_data_offset());
        ptr::copy_nonoverlapping(b"hello, GC".as_ptr(), bytes, 9);
        let empty = ptr::addr_of!(LO_EMPTY_STRING).cast_mut().cast::<Object>();
        lo_gc_write_barrier(a, POINTER_OFFSETS[0], b);
        lo_gc_write_barrier(a, POINTER_OFFSETS[1], shared);
        lo_gc_write_barrier(b, POINTER_OFFSETS[0], a);
        lo_gc_write_barrier(b, POINTER_OFFSETS[1], shared);
        lo_gc_write_barrier(shared, POINTER_OFFSETS[0], shared);
        lo_gc_write_barrier(shared, POINTER_OFFSETS[1], empty);
        lo_gc_write_barrier(caller_only, POINTER_OFFSETS[1], string);
        lo_gc_write_barrier(dead_a, POINTER_OFFSETS[0], dead_b);
        lo_gc_write_barrier(dead_b, POINTER_OFFSETS[0], dead_a);
        // This looks exactly like a reference but is not in the pointer map.
        (*a.cast::<Node>()).scalar = scalar_only as usize;
        (*a).flags = 0x1234;
        (*a).gc_bits = 0x8000_0080;
        let mut caller = Frame::new([caller_only, empty]);
        caller.push();
        let mut callee = Frame::new([a, a, shared, ptr::null_mut()]);
        callee.push();
        let before = heap_used();

        assert_eq!(mark_reachable(), 5);

        for object in [a, b, shared, caller_only, string] {
            assert!(header::is_marked(&*object));
        }
        for object in [scalar_only, dead_a, dead_b] {
            assert!(!header::is_marked(&*object));
        }
        assert_eq!((*a).gc_bits, 0x8000_0081);
        assert_eq!((*a).flags, 0x1234);
        assert_eq!((*a).class_descriptor, &NODE_CLASS as *const _);
        assert_eq!((*a.cast::<Node>()).left, b);
        assert_eq!((*shared.cast::<Node>()).left, shared);
        assert_eq!((*shared.cast::<Node>()).right, empty);
        assert_eq!(caller.roots, [caller_only, empty]);
        assert_eq!(callee.roots, [a, a, shared, ptr::null_mut()]);
        assert_eq!((*string).class_descriptor, &LO_STRING_CLASS as *const _);
        assert_eq!((*string.cast::<StringObject>()).length, 9);
        assert_eq!(core::slice::from_raw_parts(bytes, 9), b"hello, GC");
        assert_eq!(LO_EMPTY_STRING.header.gc_bits, 0);
        assert_eq!(heap_used(), before);
        lo_pop_frame();
        lo_pop_frame();
    });
}

#[test]
fn empty_heap_and_no_roots_leave_objects_unmarked() {
    with_heap(256, || unsafe {
        assert_eq!(mark_reachable(), 0);
        let unreachable = node();
        assert_eq!(mark_reachable(), 0);
        let mut empty_frame = Frame::new([]);
        empty_frame.push();
        assert_eq!(mark_reachable(), 0);
        assert!(!header::is_marked(&*unreachable));
        lo_pop_frame();
    });
}

#[test]
fn non_heap_roots_are_ignored_before_header_access() {
    with_heap(256, || unsafe {
        let empty = ptr::addr_of!(LO_EMPTY_STRING).cast_mut().cast::<Object>();
        let mut outside = Object {
            class_descriptor: ptr::null(),
            gc_bits: 0x80,
            flags: 7,
        };
        let mut frame = Frame::new([empty, ptr::addr_of_mut!(outside), ptr::null_mut()]);
        frame.push();
        assert_eq!(mark_reachable(), 0);
        assert_eq!(outside.gc_bits, 0x80);
        assert_eq!(outside.flags, 7);
        assert_eq!(LO_EMPTY_STRING.header.gc_bits, 0);
        lo_pop_frame();
    });
}

#[test]
fn worklist_holds_every_minimum_size_object_and_deduplicates_roots() {
    with_heap(64 * MIN_OBJECT_SIZE, || unsafe {
        let mut roots = [ptr::null_mut(); 128];
        for index in 0..64 {
            let object = lo_alloc(&LEAF_CLASS);
            roots[index] = object;
            roots[index + 64] = object;
        }
        let mut frame = Frame::new(roots);
        frame.push();
        assert_eq!(mark_reachable(), 64);
        for object in frame.roots {
            assert!(header::is_marked(&*object));
        }
        lo_pop_frame();
    });
}

#[test]
fn deep_graph_uses_iteration_instead_of_the_call_stack() {
    with_heap(4 * 1024 * 1024, || unsafe {
        let mut head = ptr::null_mut();
        for _ in 0..30_000 {
            let object = node();
            lo_gc_write_barrier(object, POINTER_OFFSETS[0], head);
            head = object;
        }
        let mut frame = Frame::new([head]);
        frame.push();
        assert_eq!(mark_reachable(), 30_000);
        let mut object = head;
        while !object.is_null() {
            assert!(header::is_marked(&*object));
            object = (*object.cast::<Node>()).left;
        }
        lo_pop_frame();
    });
}

#[test]
fn next_mark_pass_observes_changed_roots_after_previous_marks_are_cleared() {
    with_heap(256, || unsafe {
        let a = node();
        let b = node();
        let mut frame = Frame::new([a]);
        frame.push();
        assert_eq!(mark_reachable(), 1);
        assert!(header::is_marked(&*a));
        assert!(!header::is_marked(&*b));
        // Sliding will clear survivor state; simulate that boundary here.
        header::clear(&mut *a);
        frame.roots[0] = b;
        assert_eq!(mark_reachable(), 1);
        assert!(!header::is_marked(&*a));
        assert!(header::is_marked(&*b));
        header::clear(&mut *b);
        frame.roots[0] = ptr::null_mut();
        assert_eq!(mark_reachable(), 0);
        assert!(frame.roots[0].is_null());
        lo_pop_frame();
    });
}
