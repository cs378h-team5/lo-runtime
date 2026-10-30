//! Phase 1: compute packed destinations in increasing original-address order.
//! No movement, reference repair, allocation, or heap-cursor changes occur here.

use super::{header, layout};
use crate::object::Object;

/// Encode every survivor's destination and return the future packed boundary.
/// A boundary equal to the heap limit is valid, even though an object destination
/// must be strictly below the encoding limit.
///
/// # Safety
/// `[base, original_free)` must be the initialized heap's original packed prefix,
/// with valid descriptors and String lengths on dead objects as well as live
/// ones. Marking must be complete. The mutator must remain stopped until all
/// compaction phases finish; neither this pass nor callers may commit the new
/// boundary before pointer repair and movement are complete.
pub(crate) unsafe fn plan_destinations(base: *mut u8, original_free: *mut u8) -> *mut u8 {
    let used = (original_free as usize)
        .checked_sub(base as usize)
        .expect("allocated boundary precedes heap");
    assert!(
        used <= header::MAX_HEAP_SIZE,
        "heap exceeds destination encoding"
    );
    assert_eq!(
        used % header::OBJECT_ALIGN,
        0,
        "unaligned allocated boundary"
    );
    if used == 0 {
        return base;
    }
    let mut source = 0;
    let mut destination = 0;
    while source < used {
        let object = base.add(source).cast::<Object>();
        // Always advance by the original size, including for dead Strings.
        let size = layout::object_size(object);
        assert!(
            size != 0 && size <= used - source,
            "object exceeds allocated prefix"
        );
        if header::is_marked(&*object) {
            header::set_destination(&mut *object, destination)
                .expect("packed destination must fit heap encoding");
            destination += size;
        }
        source += size;
    }
    base.add(destination)
}

#[cfg(test)]
mod tests;
