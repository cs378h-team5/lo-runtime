//! Enumerate reference slots without choosing a marking or repair policy.

use crate::object::{shadow_frame_roots_offset, Object, ShadowFrame};

/// Visit every inline root slot, including caller frames.
///
/// # Safety
/// The frame chain and root arrays must be valid and remain registered for the
/// walk. The callback must not move objects or mutate the frame chain.
pub(super) unsafe fn root_slots(
    mut frame: *mut ShadowFrame,
    mut visit: impl FnMut(*mut *mut Object),
) {
    while !frame.is_null() {
        let roots = frame
            .cast::<u8>()
            .add(shadow_frame_roots_offset())
            .cast::<*mut Object>();
        for index in 0..(*frame).num_roots as usize {
            visit(roots.add(index));
        }
        frame = (*frame).parent;
    }
}

/// Visit only fields listed in the descriptor's pointer map, including inherited
/// fields. Scalars, the descriptor itself, and String bytes are not references.
///
/// # Safety
/// `object` and its descriptor must be valid. Pointer-map offsets must address
/// aligned, initialized object-reference slots. The callback must not move the
/// object or change its descriptor during the walk.
pub(super) unsafe fn object_slots(object: *mut Object, mut visit: impl FnMut(*mut *mut Object)) {
    let class = (*object).class_descriptor;
    for index in 0..(*class).pointer_count as usize {
        let offset = *(*class).pointer_offsets.add(index) as usize;
        visit(object.cast::<u8>().add(offset).cast::<*mut Object>());
    }
}
