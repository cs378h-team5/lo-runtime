//! Phase 2: repair roots and live fields at their original addresses. Every
//! destination must already be encoded, and no object may move during this pass.

use super::{header, layout, trace};
use crate::{object::Object, shadow_stack};

/// Rewrite one slot by consulting the original target's header exactly once.
/// Never dereference its new value: the destination bytes have not arrived yet.
unsafe fn repair_slot(slot: *mut *mut Object, base: *mut u8, used: usize) {
    let original = slot.read();
    let address = original as usize;
    let Some(offset) = address.checked_sub(base as usize) else {
        return;
    };
    // Null and legitimate non-heap statics retain their values. In particular,
    // do not read or mutate the read-only empty String's GC header.
    if original.is_null() || offset >= used {
        return;
    }
    assert_eq!(
        offset % header::OBJECT_ALIGN,
        0,
        "unaligned object reference"
    );
    assert!(
        header::is_marked(&*original),
        "live reference targets an unmarked object"
    );
    let destination = header::destination_offset(&*original);
    assert!(
        destination < used && destination <= offset,
        "invalid sliding destination"
    );
    slot.write(base.add(destination).cast());
}

/// Repair all shadow roots and pointer fields of live objects, once per slot.
///
/// # Safety
/// `[base, original_free)` must be the original allocated prefix, with intact
/// descriptors and valid String lengths on all objects. Marking and destination
/// planning must be complete. Roots and live fields still hold original object
/// pointers, null, or legitimate static pointers. Each descriptor pointer map
/// lists each slot once. The mutator remains stopped and no movement, allocation,
/// second repair pass, or allocation-boundary commit may occur during this phase.
pub(crate) unsafe fn repair_references(base: *mut u8, original_free: *mut u8) {
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

    trace::root_slots(shadow_stack::current_frame(), |slot| {
        repair_slot(slot, base, used)
    });
    let mut source = 0;
    while source < used {
        // Walk the original prefix, not the repaired roots or field values.
        let object = base.add(source).cast::<Object>();
        let size = layout::object_size(object);
        assert!(
            size != 0 && size <= used - source,
            "object exceeds allocated prefix"
        );
        if header::is_marked(&*object) {
            trace::object_slots(object, |slot| repair_slot(slot, base, used));
        }
        source += size;
    }
}

#[cfg(test)]
mod tests;
