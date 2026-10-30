//! L17 slide 9: low eight bits are GC state; high 24 bits encode an aligned
//! destination offset from the heap base. Descriptors and reserved flags survive.

use crate::object::Object;

pub(crate) const OBJECT_ALIGN: usize = 8;
pub(crate) const MAX_HEAP_SIZE: usize = (1 << 24) * OBJECT_ALIGN;
const STATE_BITS: u32 = 8;
const STATE_MASK: u32 = (1 << STATE_BITS) - 1;
const MARK_BIT: u32 = 1;

pub(crate) fn is_marked(object: &Object) -> bool {
    object.gc_bits & MARK_BIT != 0
}

pub(crate) fn mark(object: &mut Object) {
    object.gc_bits |= MARK_BIT;
}

/// Set a heap-relative destination, preserving state. Failure leaves the header
/// unchanged. Offset zero is valid; the mark bit determines liveness.
pub(crate) fn set_destination(object: &mut Object, offset: usize) -> Result<(), &'static str> {
    if offset >= MAX_HEAP_SIZE || offset & (OBJECT_ALIGN - 1) != 0 {
        return Err("destination must be aligned and below 128 MiB");
    }
    object.gc_bits = ((offset / OBJECT_ALIGN) as u32) << STATE_BITS | (object.gc_bits & STATE_MASK);
    Ok(())
}

/// Read only after the destination-planning pass has assigned this live object.
/// The caller reconstructs the pointer with `heap_base.add(destination_offset)`.
pub(crate) fn destination_offset(object: &Object) -> usize {
    (object.gc_bits >> STATE_BITS) as usize * OBJECT_ALIGN
}

pub(crate) fn clear(object: &mut Object) {
    object.gc_bits = 0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptors::LO_INT_BOX_CLASS;

    #[test]
    fn destination_round_trips_preserving_descriptor_flags_and_state() {
        let descriptor = &LO_INT_BOX_CLASS as *const _;
        let mut object = Object {
            class_descriptor: descriptor,
            gc_bits: 0x80,
            flags: 0x1234,
        };
        for offset in [0, 8, 0x098, 16 * 1024 * 1024, MAX_HEAP_SIZE - OBJECT_ALIGN] {
            mark(&mut object);
            set_destination(&mut object, offset).unwrap();
            assert!(is_marked(&object));
            assert_eq!(destination_offset(&object), offset);
            assert_eq!(object.gc_bits & STATE_MASK, 0x81);
            assert_eq!(object.class_descriptor, descriptor);
            assert_eq!(object.flags, 0x1234);
        }
        clear(&mut object);
        assert!(!is_marked(&object));
        assert_eq!(object.gc_bits, 0);
        assert_eq!(object.class_descriptor, descriptor);
        assert_eq!(object.flags, 0x1234);
    }

    #[test]
    fn invalid_destinations_do_not_truncate_or_change_header() {
        let mut object = Object {
            class_descriptor: &LO_INT_BOX_CLASS,
            gc_bits: MARK_BIT,
            flags: 0,
        };
        for offset in [1, MAX_HEAP_SIZE, MAX_HEAP_SIZE + OBJECT_ALIGN, usize::MAX] {
            assert!(set_destination(&mut object, offset).is_err());
            assert_eq!(object.gc_bits, MARK_BIT);
        }
    }
}
