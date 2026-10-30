use super::*;
use crate::{
    alloc::{allocated_range, bump_alloc_string, heap_used, lo_alloc},
    descriptors::LO_EMPTY_STRING,
    gc::{
        mark::mark_reachable,
        plan::plan_destinations,
        test_support::{with_heap, Frame},
    },
    object::{string_data_offset, ClassDescriptor, StringObject},
    shadow_stack::lo_pop_frame,
};
use core::{
    mem::{offset_of, size_of},
    ptr,
};

#[repr(C)]
struct Node {
    header: Object,
    first: *mut Object,
    scalar: usize,
    second: *mut Object,
}
static OFFSETS: [u32; 2] = [
    offset_of!(Node, first) as u32,
    offset_of!(Node, second) as u32,
];
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

fn empty_string() -> *mut Object {
    ptr::addr_of!(LO_EMPTY_STRING).cast_mut().cast()
}

#[test]
fn repairs_original_targets_once_across_frames_cycles_and_shared_fields() {
    with_heap(4096, || unsafe {
        let _dead_front = lo_alloc(&LEAF_CLASS);
        let a = lo_alloc(&NODE_CLASS);
        let _dead_middle = bump_alloc_string(33);
        let b = lo_alloc(&NODE_CLASS);
        let shared = lo_alloc(&NODE_CLASS);
        let string = bump_alloc_string(9);
        let bytes = string.cast::<u8>().add(string_data_offset());
        ptr::copy_nonoverlapping(b"hello, GC".as_ptr(), bytes, 9);
        let dead_tail = lo_alloc(&NODE_CLASS);
        (*a.cast::<Node>()).first = b;
        (*a.cast::<Node>()).second = shared;
        (*a.cast::<Node>()).scalar = b as usize;
        (*b.cast::<Node>()).first = a;
        (*b.cast::<Node>()).second = shared;
        (*shared.cast::<Node>()).first = string;
        (*shared.cast::<Node>()).second = empty_string();
        (*dead_tail.cast::<Node>()).first = a;
        (*a).flags = 0x1234;
        let mut caller = Frame::new([b, string, empty_string(), ptr::null_mut()]);
        caller.push();
        let mut callee = Frame::new([a, a, shared]);
        callee.push();
        assert_eq!(mark_reachable(), 4);
        let (base, end) = allocated_range();
        let used = heap_used();
        let packed_end = plan_destinations(base, end);
        let future = [a, b, shared, string].map(|object| {
            base.add(header::destination_offset(&*object))
                .cast::<Object>()
        });
        assert_eq!(future[0], base.cast()); // Destination zero remains a real pointer.
        assert!(string as usize >= packed_end as usize); // Use original heap bounds.
        assert_ne!(a, future[0]);
        let before = core::slice::from_raw_parts(base, used).to_vec();

        repair_references(base, end);

        assert_eq!(
            caller.roots,
            [future[1], future[3], empty_string(), ptr::null_mut()]
        );
        assert_eq!(callee.roots, [future[0], future[0], future[2]]);
        // Inspect original object addresses; no destination may be dereferenced.
        assert_eq!((*a.cast::<Node>()).first, future[1]);
        assert_eq!((*a.cast::<Node>()).second, future[2]);
        assert_eq!((*b.cast::<Node>()).first, future[0]);
        assert_eq!((*b.cast::<Node>()).second, future[2]);
        assert_eq!((*shared.cast::<Node>()).first, future[3]);
        assert_eq!((*shared.cast::<Node>()).second, empty_string());
        assert_eq!((*dead_tail.cast::<Node>()).first, a);
        assert_eq!((*a.cast::<Node>()).scalar, b as usize);
        assert_eq!((*a).flags, 0x1234);
        assert_eq!((*string.cast::<StringObject>()).length, 9);
        assert_eq!(core::slice::from_raw_parts(bytes, 9), b"hello, GC");
        assert_eq!(allocated_range(), (base, end));
        assert_eq!(heap_used(), used);

        // Every byte except a live pointer field is unchanged, including all
        // source headers, destination encodings, padding, and dead-object bytes.
        let after = core::slice::from_raw_parts(base, used);
        for index in 0..used {
            let pointer_byte = [a, b, shared].iter().any(|object| {
                OFFSETS.iter().any(|offset| {
                    let start = *object as usize - base as usize + *offset as usize;
                    (start..start + size_of::<*mut Object>()).contains(&index)
                })
            });
            if !pointer_byte {
                assert_eq!(after[index], before[index], "byte {index}");
            }
        }
        lo_pop_frame();
        lo_pop_frame();
    });
}

#[test]
fn stationary_self_cycle_and_static_field_remain_unchanged() {
    with_heap(256, || unsafe {
        let object = lo_alloc(&NODE_CLASS);
        (*object.cast::<Node>()).first = object;
        (*object.cast::<Node>()).second = empty_string();
        let mut roots = Frame::new([object, object, ptr::null_mut()]);
        roots.push();
        assert_eq!(mark_reachable(), 1);
        let (base, end) = allocated_range();
        assert_eq!(plan_destinations(base, end), end);
        let before = core::slice::from_raw_parts(base, heap_used()).to_vec();
        repair_references(base, end);
        assert_eq!(roots.roots, [object, object, ptr::null_mut()]);
        assert_eq!((*object.cast::<Node>()).first, object);
        assert_eq!((*object.cast::<Node>()).second, empty_string());
        assert_eq!(core::slice::from_raw_parts(base, heap_used()), before);
        assert_eq!(LO_EMPTY_STRING.header.gc_bits, 0);
        lo_pop_frame();
    });
}

#[test]
fn empty_and_all_dead_heaps_preserve_static_roots_and_skip_dead_fields() {
    with_heap(256, || unsafe {
        let mut roots = Frame::new([ptr::null_mut(), empty_string()]);
        roots.push();
        let (base, end) = allocated_range();
        assert_eq!(mark_reachable(), 0);
        assert_eq!(plan_destinations(base, end), base);
        repair_references(base, end);
        let dead = lo_alloc(&NODE_CLASS);
        // Dead fields may be stale and must never be traced or repaired.
        (*dead.cast::<Node>()).first = base.add(1).cast();
        let _dead_string = bump_alloc_string(17);
        let (_, end) = allocated_range();
        assert_eq!(mark_reachable(), 0);
        assert_eq!(plan_destinations(base, end), base);
        let before = core::slice::from_raw_parts(base, heap_used()).to_vec();
        repair_references(base, end);
        assert_eq!(roots.roots, [ptr::null_mut(), empty_string()]);
        assert_eq!(core::slice::from_raw_parts(base, heap_used()), before);
        assert_eq!(allocated_range(), (base, end));
        lo_pop_frame();
    });
}
