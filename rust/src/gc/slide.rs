//! Phase 3: slide in increasing source order using overlap-safe copies.

use super::{header, layout};
use crate::object::Object;

/// Move repaired survivors into their packed destinations and clear GC state.
/// The caller commits `packed_free` only after this pass finishes.
///
/// # Safety
/// All earlier phases must be complete with the mutator stopped. Original
/// headers/descriptors/lengths and encoded destinations must still be intact.
/// `[base, original_free)` is the original allocated prefix; `packed_free` is
/// the boundary returned by planning. All reference slots already name future
/// addresses. No allocation or other heap walk may run during this pass.
pub(crate) unsafe fn slide_objects(base: *mut u8, original_free: *mut u8, packed_free: *mut u8) {
    let used = (original_free as usize)
        .checked_sub(base as usize)
        .expect("allocated boundary precedes heap");
    let packed_size = (packed_free as usize)
        .checked_sub(base as usize)
        .expect("packed boundary precedes heap");
    assert!(packed_size <= used && used <= header::MAX_HEAP_SIZE);
    let mut source = 0;
    let mut packed = 0;
    while source < used {
        let object = base.add(source).cast::<Object>();
        let size = layout::object_size(object);
        assert!(
            size != 0 && size <= used - source,
            "object exceeds allocated prefix"
        );
        // Save the next source and destination before copying can overwrite the
        // source header. Earlier copies never reach a later object's header.
        let next_source = source + size;
        if header::is_marked(&*object) {
            let destination = header::destination_offset(&*object);
            assert_eq!(
                destination, packed,
                "destinations must form a packed prefix"
            );
            assert!(destination <= source && size <= packed_size - packed);
            let target = base.add(destination);
            core::ptr::copy(object.cast::<u8>(), target, size);
            header::clear(&mut *target.cast::<Object>());
            packed += size;
        }
        source = next_source;
    }
    assert_eq!(
        packed, packed_size,
        "live bytes must match planned boundary"
    );
}
