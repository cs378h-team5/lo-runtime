use super::*;
use crate::{
    alloc::{allocated_range, bump_alloc_string, heap_used, lo_alloc},
    descriptors::LO_EMPTY_STRING,
    gc::{
        mark::mark_reachable,
        test_support::{with_heap, Frame},
    },
    object::{ClassDescriptor, StringObject},
    shadow_stack::lo_pop_frame,
};
use core::{
    mem::{offset_of, size_of},
    ptr,
};

#[repr(C)]
struct Dog {
    header: Object,
    name: *mut Object,
    age: i32,
    breed: *mut Object,
}
#[repr(C)]
struct Pen {
    header: Object,
    dog: *mut Object,
    next: *mut Object,
    label: *mut Object,
}
static DOG_OFFSETS: [u32; 2] = [offset_of!(Dog, name) as u32, offset_of!(Dog, breed) as u32];
static PEN_OFFSETS: [u32; 3] = [
    offset_of!(Pen, dog) as u32,
    offset_of!(Pen, next) as u32,
    offset_of!(Pen, label) as u32,
];
static DOG_CLASS: ClassDescriptor = ClassDescriptor {
    name: c"Dog".as_ptr().cast(),
    name_len: 3,
    parent: ptr::null(),
    instance_size: size_of::<Dog>() as u32,
    pointer_offsets: DOG_OFFSETS.as_ptr(),
    pointer_count: 2,
    vtable_size: 0,
    vtable: ptr::null(),
};
static PEN_CLASS: ClassDescriptor = ClassDescriptor {
    name: c"Pen".as_ptr().cast(),
    name_len: 3,
    parent: ptr::null(),
    instance_size: size_of::<Pen>() as u32,
    pointer_offsets: PEN_OFFSETS.as_ptr(),
    pointer_count: 3,
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

fn aligned(size: usize) -> usize {
    (size + 7) & !7
}
fn string_bytes(length: usize) -> usize {
    aligned(offset_of!(StringObject, data) + length)
}

unsafe fn string(bytes: &[u8]) -> *mut Object {
    let object = bump_alloc_string(bytes.len() as u32);
    ptr::copy_nonoverlapping(
        bytes.as_ptr(),
        object.cast::<u8>().add(offset_of!(StringObject, data)),
        bytes.len(),
    );
    object
}

#[test]
fn shelter_matches_l17_through_destination_and_repair_passes() {
    with_heap(4096, || unsafe {
        let main = lo_alloc(&LEAF_CLASS);
        let rex_name = string(b"Rex");
        let labrador = string(b"Labrador");
        let rex = lo_alloc(&DOG_CLASS);
        let a = lo_alloc(&PEN_CLASS);
        let mia_name = string(b"Mia");
        let beagle = string(b"Beagle");
        let mia = lo_alloc(&DOG_CLASS);
        let b = lo_alloc(&PEN_CLASS);
        let home = lo_alloc(&PEN_CLASS);
        (*rex.cast::<Dog>()).name = rex_name;
        (*rex.cast::<Dog>()).breed = labrador;
        (*rex.cast::<Dog>()).age = 5;
        (*mia.cast::<Dog>()).name = mia_name;
        (*mia.cast::<Dog>()).breed = beagle;
        (*mia.cast::<Dog>()).age = 3;
        (*a.cast::<Pen>()).dog = rex;
        (*a.cast::<Pen>()).next = b;
        (*b.cast::<Pen>()).dog = mia;
        (*b.cast::<Pen>()).next = a;
        (*home.cast::<Pen>()).dog = rex;
        (*home.cast::<Pen>()).next = home;
        (*home.cast::<Pen>()).label = ptr::addr_of!(LO_EMPTY_STRING).cast_mut().cast();
        (*home).flags = 0x1234;
        (*home).gc_bits = 0x80;
        let mut roots = Frame::new([rex, mia, home]);
        roots.push();
        assert_eq!(mark_reachable(), 7);
        let (base, end) = allocated_range();
        let used = heap_used();
        let before = core::slice::from_raw_parts(base, used).to_vec();
        let live = [rex_name, labrador, rex, mia_name, beagle, mia, home];
        let sizes = [
            string_bytes(3),
            string_bytes(8),
            aligned(size_of::<Dog>()),
            string_bytes(3),
            string_bytes(6),
            aligned(size_of::<Dog>()),
            aligned(size_of::<Pen>()),
        ];

        let packed_end = plan_destinations(base, end);

        let mut expected = 0;
        for (object, size) in live.into_iter().zip(sizes) {
            assert!(header::is_marked(&*object));
            assert_eq!(header::destination_offset(&*object), expected);
            expected += size;
        }
        assert_eq!(packed_end, base.add(expected));
        assert_eq!(allocated_range(), (base, end));
        assert_eq!(heap_used(), used);
        assert_eq!(roots.roots, [rex, mia, home]);
        assert_eq!((*home).gc_bits & 0xFF, 0x81);
        for dead in [main, a, b] {
            assert_eq!((*dead).gc_bits, 0);
        }
        // Only live gc_bits words may change: roots, fields, descriptors, scalar
        // payloads, String bytes, dead objects, and reserved flags are untouched.
        let after = core::slice::from_raw_parts(base, used);
        for index in 0..used {
            let is_destination_word = live.iter().any(|object| {
                let start = *object as usize - base as usize + offset_of!(Object, gc_bits);
                (start..start + size_of::<u32>()).contains(&index)
            });
            if !is_destination_word {
                assert_eq!(after[index], before[index], "byte {index}");
            }
        }
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(used, 0x148);
            assert_eq!(expected, 232);
            assert_eq!(header::destination_offset(&*rex), 0x038);
            assert_eq!(header::destination_offset(&*mia), 0x098);
            assert_eq!(header::destination_offset(&*home), 0x0C0);
            assert_eq!(packed_end, base.add(0x0E8));
        }
        super::super::repair::repair_references(base, end);
        let future = |object: *mut Object| {
            base.add(header::destination_offset(&*object))
                .cast::<Object>()
        };
        assert_eq!(roots.roots, [future(rex), future(mia), future(home)]);
        assert_eq!((*rex.cast::<Dog>()).name, future(rex_name));
        assert_eq!((*rex.cast::<Dog>()).breed, future(labrador));
        assert_eq!((*mia.cast::<Dog>()).name, future(mia_name));
        assert_eq!((*mia.cast::<Dog>()).breed, future(beagle));
        assert_eq!((*home.cast::<Pen>()).dog, future(rex));
        assert_eq!((*home.cast::<Pen>()).next, future(home));
        assert_eq!(
            (*home.cast::<Pen>()).label,
            ptr::addr_of!(LO_EMPTY_STRING).cast_mut().cast()
        );
        assert_eq!(allocated_range(), (base, end));
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!((*mia.cast::<Dog>()).name, base.add(0x060).cast());
            assert_eq!((*mia.cast::<Dog>()).breed, base.add(0x078).cast());
            assert_eq!((*home.cast::<Pen>()).next, base.add(0x0C0).cast());
        }
        lo_pop_frame();
    });
}

#[test]
fn empty_and_all_dead_prefixes_plan_an_empty_live_region() {
    with_heap(256, || unsafe {
        let (base, end) = allocated_range();
        assert_eq!(plan_destinations(base, end), base);
        let dead_string = string(b"unreachable variable-size String");
        let dead_leaf = lo_alloc(&LEAF_CLASS);
        let used = heap_used();
        assert_eq!(mark_reachable(), 0);
        let (_, end) = allocated_range();
        assert_eq!(plan_destinations(base, end), base);
        assert_eq!((*dead_string).gc_bits, 0);
        assert_eq!((*dead_leaf).gc_bits, 0);
        assert_eq!(heap_used(), used); // Planning never reclaims/commits anything.
    });
}

#[test]
fn all_live_full_heap_keeps_destinations_and_allocation_boundary() {
    with_heap(64, || unsafe {
        let objects = core::array::from_fn::<_, 4, _>(|_| lo_alloc(&LEAF_CLASS));
        let mut roots = Frame::new(objects);
        roots.push();
        assert_eq!(mark_reachable(), 4);
        let (base, end) = allocated_range();
        assert_eq!(heap_used(), 64);
        assert_eq!(plan_destinations(base, end), end);
        for object in objects {
            assert_eq!(
                base.add(header::destination_offset(&*object)),
                object.cast::<u8>()
            );
        }
        assert_eq!(roots.roots, objects);
        lo_pop_frame();
    });
}

#[test]
fn walks_dead_strings_by_variable_size_and_replaces_stale_destinations() {
    with_heap(512, || unsafe {
        let _dead_front = string(b"123456789");
        let first = lo_alloc(&LEAF_CLASS);
        let _dead_middle = string(b"12345678901234567");
        let second = string(b"x");
        let _dead_tail = string(b"");
        header::set_destination(&mut *first, 128).unwrap();
        header::set_destination(&mut *second, 256).unwrap();
        let mut roots = Frame::new([first, second]);
        roots.push();
        assert_eq!(mark_reachable(), 2);
        let (base, end) = allocated_range();
        let expected = aligned(size_of::<Object>()) + string_bytes(1);
        assert_eq!(plan_destinations(base, end), base.add(expected));
        assert_eq!(header::destination_offset(&*first), 0);
        assert_eq!(
            header::destination_offset(&*second),
            aligned(size_of::<Object>())
        );
        assert_eq!((*second.cast::<StringObject>()).length, 1);
        assert_eq!(roots.roots, [first, second]);
        lo_pop_frame();
    });
}
