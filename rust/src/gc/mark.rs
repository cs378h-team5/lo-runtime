//! Phase 0: iterative reachability marking. No managed or scratch allocation
//! occurs during marking. Destinations, reference slots, and payloads are intact.

use core::{alloc::Layout, mem::size_of, ptr};

use super::{header, trace};
use crate::{alloc, object::Object, shadow_stack};

// Single-threaded runtime state, owned from heap initialization to shutdown.
// Only initialized worklist entries are read. Mark-before-push means each
// object occupies at most one entry, so heap_size / minimum_size is sufficient.
static mut ENTRIES: *mut *mut Object = ptr::null_mut();
static mut CAPACITY: usize = 0;
static mut MARKING: bool = false;
const MIN_OBJECT_SIZE: usize =
    (size_of::<Object>() + header::OBJECT_ALIGN - 1) & !(header::OBJECT_ALIGN - 1);

/// Reserve the traversal stack separately from the managed heap, before GC can
/// run. This is scratch space, not a forwarding table.
pub(crate) fn init(heap_size: usize) {
    let capacity = heap_size / MIN_OBJECT_SIZE;
    let layout = Layout::array::<*mut Object>(capacity).expect("valid mark worklist layout");
    // SAFETY: runtime initialization is single-threaded and the heap holds at
    // least one object. No references to these statics are retained.
    unsafe {
        assert!(ENTRIES.is_null(), "mark worklist already initialized");
        let entries = std::alloc::alloc(layout).cast::<*mut Object>();
        if entries.is_null() {
            crate::abort::runtime_abort("lo_alloc: out of memory", 137);
        }
        ENTRIES = entries;
        CAPACITY = capacity;
    }
}

pub(crate) fn shutdown() {
    // SAFETY: no collection is active; allocation and layout match init.
    unsafe {
        assert!(!MARKING, "cannot shut down during marking");
        if !ENTRIES.is_null() {
            let layout =
                Layout::array::<*mut Object>(CAPACITY).expect("valid mark worklist layout");
            std::alloc::dealloc(ENTRIES.cast(), layout);
            ENTRIES = ptr::null_mut();
            CAPACITY = 0;
        }
    }
}

struct Worklist {
    entries: *mut *mut Object,
    capacity: usize,
    len: usize,
    marked: usize,
    base: usize,
    end: usize,
}

impl Worklist {
    /// Null and non-heap statics are rejected before dereferencing any header.
    /// In-range references must be valid object starts, as required by the ABI.
    unsafe fn discover(&mut self, object: *mut Object) {
        let address = object as usize;
        if address < self.base || address >= self.end || object.is_null() {
            return;
        }
        if header::is_marked(&*object) {
            return;
        }
        assert!(self.len < self.capacity, "mark worklist capacity exceeded");
        // Mark before enqueueing: cycles and duplicate roots never enqueue twice.
        header::mark(&mut *object);
        self.entries.add(self.len).write(object);
        self.len += 1;
        self.marked += 1;
    }
}

struct MarkGuard;
impl Drop for MarkGuard {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the single-threaded marking phase.
        unsafe {
            MARKING = false;
        }
    }
}

/// Mark all objects reachable through registered roots and descriptor maps.
/// Return the number of newly marked objects for direct tests/diagnostics.
///
/// # Safety
/// Runtime and worklist must be initialized, the mutator stopped, and all managed
/// objects initially unmarked. Roots/fields in the allocated prefix must point
/// at valid object starts with intact descriptors; other references must be
/// null or legitimate non-heap statics. No allocation or movement may run until
/// this phase returns. Subsequent phases must eventually clear survivor marks.
pub(crate) unsafe fn mark_reachable() -> usize {
    assert!(!MARKING, "reentrant marking");
    assert!(!ENTRIES.is_null(), "mark worklist is not initialized");
    MARKING = true;
    let _guard = MarkGuard;
    let (base, end) = alloc::allocated_range();
    let mut worklist = Worklist {
        entries: ENTRIES,
        capacity: CAPACITY,
        len: 0,
        marked: 0,
        base: base as usize,
        end: end as usize,
    };
    trace::root_slots(shadow_stack::current_frame(), |slot| {
        worklist.discover(*slot)
    });
    while worklist.len != 0 {
        worklist.len -= 1;
        let object = worklist.entries.add(worklist.len).read();
        trace::object_slots(object, |slot| worklist.discover(*slot));
    }
    worklist.marked
}

#[cfg(test)]
mod tests;
