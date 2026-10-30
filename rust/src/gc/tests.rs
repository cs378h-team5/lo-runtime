use super::*;
use crate::{
    alloc::{allocated_range, heap_limit, heap_used, lo_alloc},
    descriptors::LO_EMPTY_STRING,
    gc::test_support::{with_heap, Frame},
    object::{string_data_offset, ClassDescriptor, StringObject},
    shadow_stack::lo_pop_frame,
    string_ops::{lo_string_concat, lo_string_new, lo_string_repeat, lo_string_reverse},
};
use core::{
    mem::{offset_of, size_of},
    ptr,
};

#[repr(C)]
struct Node {
    header: Object,
    next: *mut Object,
    text: *mut Object,
    value: i32,
}
static OFFSETS: [u32; 2] = [offset_of!(Node, next) as u32, offset_of!(Node, text) as u32];
static NODE_CLASS: ClassDescriptor = ClassDescriptor {
    name: c"Node".as_ptr().cast(),
    name_len: 4,
    parent: ptr::null(),
    instance_size: size_of::<Node>() as u32,
    pointer_offsets: OFFSETS.as_ptr(),
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

unsafe fn assert_bytes(object: *mut Object, expected: &[u8]) {
    assert_eq!(
        (*object.cast::<StringObject>()).length as usize,
        expected.len()
    );
    assert_eq!(
        core::slice::from_raw_parts(
            object.cast::<u8>().add(string_data_offset()),
            expected.len()
        ),
        expected
    );
}

#[test]
fn overlapping_slides_preserve_cycles_payloads_and_poison_only_free_tail() {
    with_heap(512, || unsafe {
        POISON = true;
        let _hole = lo_alloc(&LEAF_CLASS);
        let a = lo_alloc(&NODE_CLASS);
        let b = lo_alloc(&NODE_CLASS);
        let text = lo_string_new(b"hello".as_ptr(), 5);
        (*a.cast::<Node>()).next = b;
        (*a.cast::<Node>()).text = text;
        (*a.cast::<Node>()).value = 123;
        (*a).flags = 0x44;
        (*b.cast::<Node>()).next = a;
        (*b.cast::<Node>()).text = ptr::addr_of!(LO_EMPTY_STRING).cast_mut().cast();
        (*b.cast::<Node>()).value = 456;
        let mut roots = Frame::new([a]);
        roots.push();
        let before = heap_used();
        lo_gc_collect();
        let moved_a = roots.roots[0];
        assert_ne!(moved_a, a);
        assert_eq!(moved_a.cast::<u8>(), allocated_range().0);
        let moved_b = (*moved_a.cast::<Node>()).next;
        assert_eq!((*moved_b.cast::<Node>()).next, moved_a);
        assert_eq!((*moved_a.cast::<Node>()).value, 123);
        assert_eq!((*moved_b.cast::<Node>()).value, 456);
        assert_eq!((*moved_a).flags, 0x44);
        assert_eq!((*moved_a).class_descriptor, &NODE_CLASS as *const _);
        assert_eq!(
            (*moved_b.cast::<Node>()).text,
            ptr::addr_of!(LO_EMPTY_STRING).cast_mut().cast()
        );
        let moved_text = (*moved_a.cast::<Node>()).text;
        assert_bytes(moved_text, b"hello");
        for object in [moved_a, moved_b, moved_text] {
            assert_eq!((*object).gc_bits, 0);
        }
        assert_eq!(
            heap_used(),
            before - layout::fixed_size(LEAF_CLASS.instance_size)
        );
        let (_, free) = allocated_range();
        let tail = heap_limit() as usize - free as usize;
        assert!(core::slice::from_raw_parts(free, tail)
            .iter()
            .all(|byte| *byte == 0xEF));
        // No holes on the next cycle: survivors stay put, marks must still reset.
        lo_gc_collect();
        assert_eq!(roots.roots[0], moved_a);
        assert_bytes((*roots.roots[0].cast::<Node>()).text, b"hello");
        lo_pop_frame();
    });
}

#[test]
fn allocation_pressure_reclaims_garbage_and_preserves_caller_roots() {
    with_heap(256, || unsafe {
        POISON = true;
        let _dead = lo_alloc(&LEAF_CLASS);
        let live = lo_alloc(&NODE_CLASS);
        (*live.cast::<Node>()).value = 789;
        (*live.cast::<Node>()).next = live;
        let mut caller = Frame::new([live]);
        caller.push();
        let mut callee = Frame::new([]);
        callee.push();
        for _ in 0..2000 {
            let _garbage = lo_alloc(&NODE_CLASS);
        }
        let survivor = caller.roots[0];
        assert_ne!(survivor, live);
        assert_eq!((*survivor.cast::<Node>()).value, 789);
        assert_eq!((*survivor.cast::<Node>()).next, survivor);
        assert_eq!((*survivor).gc_bits, 0);
        lo_pop_frame();
        lo_pop_frame();
    });
}

#[test]
fn full_live_heap_and_empty_heap_collect_without_changing_identity() {
    with_heap(64, || unsafe {
        lo_gc_collect();
        assert_eq!(heap_used(), 0);
        let objects = core::array::from_fn::<_, 4, _>(|_| lo_alloc(&LEAF_CLASS));
        let mut roots = Frame::new(objects);
        roots.push();
        for _ in 0..3 {
            lo_gc_collect();
            assert_eq!(roots.roots, objects);
            assert_eq!(heap_used(), 64);
            for object in roots.roots {
                assert_eq!((*object).gc_bits, 0);
            }
        }
        lo_pop_frame();
        lo_gc_collect();
        assert_eq!(heap_used(), 0);
        let replacement = lo_alloc(&LEAF_CLASS);
        assert_eq!(replacement, objects[0]);
        assert_eq!((*replacement).gc_bits, 0);
    });
}

#[test]
fn unreachable_cycles_are_reclaimed_and_reused_memory_is_zeroed() {
    with_heap(256, || unsafe {
        POISON = true;
        let a = lo_alloc(&NODE_CLASS);
        let b = lo_alloc(&NODE_CLASS);
        (*a.cast::<Node>()).next = b;
        (*b.cast::<Node>()).next = a;
        lo_gc_collect();
        assert_eq!(heap_used(), 0);
        let replacement = lo_alloc(&NODE_CLASS);
        assert_eq!(replacement, a);
        assert!((*replacement.cast::<Node>()).next.is_null());
        assert!((*replacement.cast::<Node>()).text.is_null());
        assert_eq!((*replacement.cast::<Node>()).value, 0);
    });
}

#[test]
fn allocating_string_operations_root_inputs_across_many_collections() {
    with_heap(256, || unsafe {
        POISON = true;
        let a = lo_string_new(b"ab".as_ptr(), 2);
        let mut roots = Frame::new([a, ptr::null_mut(), ptr::null_mut()]);
        roots.push();
        let other = "c🍂".as_bytes();
        roots.roots[1] = lo_string_new(other.as_ptr(), other.len() as u32);
        for _ in 0..200 {
            roots.roots[2] = lo_string_concat(roots.roots[0], roots.roots[1]);
            assert_bytes(roots.roots[2], "abc🍂".as_bytes());
            roots.roots[2] = lo_string_reverse(roots.roots[2]);
            assert_bytes(roots.roots[2], "🍂cba".as_bytes());
            roots.roots[2] = lo_string_repeat(roots.roots[0], 3);
            assert_bytes(roots.roots[2], b"ababab");
        }
        lo_gc_collect();
        assert_bytes(roots.roots[0], b"ab");
        assert_bytes(roots.roots[1], other);
        assert_bytes(roots.roots[2], b"ababab");
        lo_pop_frame();
    });
}
