//! Stop-the-world sliding mark-compact collector (L17).
//!
//! `gc/cheney.rs` preserves the supplied reference and its tests, but is not a
//! compiled module: its two-space algorithm cannot run on our single-region heap.

pub(crate) mod header;
pub(crate) mod layout;
pub(crate) mod mark;
mod plan;
mod repair;
mod slide;
mod trace;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests;

use crate::object::Object;

// Single-threaded lifecycle state. Read debug configuration at initialization,
// rather than allocating an environment String during collection.
static mut COLLECTING: bool = false;
static mut POISON: bool = false;

pub(crate) fn init(heap_size: usize) {
    mark::init(heap_size);
    unsafe {
        POISON = std::env::var("LO_GC_POISON")
            .map(|value| !value.is_empty() && value != "0")
            .unwrap_or(false);
    }
}

pub(crate) fn shutdown() {
    unsafe {
        assert!(!COLLECTING, "cannot shut down during collection");
        POISON = false;
    }
    mark::shutdown();
}

struct CollectionGuard;
impl Drop for CollectionGuard {
    fn drop(&mut self) {
        unsafe {
            COLLECTING = false;
        }
    }
}

/// Store a pointer field. Mark-compact needs no remembered set.
///
/// # Safety
/// `obj` must contain a valid pointer field at `field_offset`, and `value` must
/// be null or a valid object pointer. No collection occurs in this operation.
#[no_mangle]
pub unsafe extern "C" fn lo_gc_write_barrier(
    obj: *mut Object,
    field_offset: u32,
    value: *mut Object,
) {
    let slot = obj
        .cast::<u8>()
        .add(field_offset as usize)
        .cast::<*mut Object>();
    *slot = value;
}

/// Mark, plan, repair, slide, then commit the packed allocation boundary.
/// Shadow roots and pointer maps are the complete source of managed references.
#[no_mangle]
pub extern "C" fn lo_gc_collect() {
    // SAFETY: the runtime is single-threaded and the mutator is stopped for this
    // entire call. Every phase runs without allocating and in dependency order.
    unsafe {
        assert!(!COLLECTING, "reentrant collection");
        let (base, original_free) = crate::alloc::allocated_range();
        if base.is_null() {
            return;
        }
        COLLECTING = true;
        let _guard = CollectionGuard;
        mark::mark_reachable();
        let packed_free = plan::plan_destinations(base, original_free);
        repair::repair_references(base, original_free);
        slide::slide_objects(base, original_free, packed_free);
        crate::alloc::commit_free(packed_free);
        if POISON {
            let tail_size = crate::alloc::heap_limit() as usize - packed_free as usize;
            core::ptr::write_bytes(packed_free, 0xEF, tail_size);
        }
    }
}
