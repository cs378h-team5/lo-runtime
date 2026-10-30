//! Shared fixtures for direct phase tests; serialized around global runtime state.

use crate::{
    alloc::{heap_init_with_size, heap_shutdown, TEST_LOCK},
    object::{shadow_frame_roots_offset, Object, ShadowFrame},
    shadow_stack::{lo_push_frame, reset},
};
use core::{mem::offset_of, ptr};

#[repr(C)]
pub(super) struct Frame<const N: usize> {
    header: ShadowFrame,
    pub(super) roots: [*mut Object; N],
}
impl<const N: usize> Frame<N> {
    pub(super) fn new(roots: [*mut Object; N]) -> Self {
        assert_eq!(offset_of!(Self, roots), shadow_frame_roots_offset());
        Self {
            header: ShadowFrame {
                parent: ptr::null_mut(),
                num_roots: N as u32,
                roots: [],
            },
            roots,
        }
    }

    /// Caller must keep this frame stationary until it is popped.
    pub(super) unsafe fn push(&mut self) {
        lo_push_frame(ptr::addr_of_mut!(self.header));
    }
}

struct HeapGuard;
impl Drop for HeapGuard {
    fn drop(&mut self) {
        reset();
        heap_shutdown();
    }
}

pub(super) fn with_heap(size: usize, run: impl FnOnce()) {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    heap_init_with_size(size);
    reset();
    let _heap = HeapGuard;
    run();
}
