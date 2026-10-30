//! Single-region bump allocator for the sliding mark-compact collector.
//!
//! The allocated prefix is `[HEAP_BASE, FREE)` and the free tail is
//! `[FREE, HEAP_LIMIT)`. Only this module owns the heap pointers. Collection
//! will pack survivors into the prefix and commit its new end with `commit_free`.
//! Runtime state is process-global and single-threaded.
//!
//! The 16 MiB default and native LO_HEAP_SIZE override are preserved. Destinations
//! are header-encoded, so configurations above 128 MiB are explicitly rejected.

use core::alloc::Layout;
use core::ptr;

use crate::descriptors::LO_STRING_CLASS;
use crate::gc::{
    header::{MAX_HEAP_SIZE, OBJECT_ALIGN},
    layout::{fixed_size, string_size},
};
use crate::object::{ClassDescriptor, Object, StringObject};

/// Default total heap size (16 MiB), overridable via `LO_HEAP_SIZE` at init.
const DEFAULT_HEAP_SIZE: usize = 16 * 1024 * 1024;
/// Backing-region alignment; individual objects are rounded to OBJECT_ALIGN.
const HEAP_ALIGN: usize = 16;

/// Base of the whole backing region (for `dealloc`); null when uninitialized.
static mut HEAP_BASE: *mut u8 = ptr::null_mut();
/// Layout the region was allocated with (for `dealloc`).
static mut HEAP_LAYOUT: Option<Layout> = None;

/// Exclusive end of the single managed region.
static mut HEAP_LIMIT: *mut u8 = ptr::null_mut();
/// End of the allocated prefix; start of the free tail.
static mut FREE: *mut u8 = ptr::null_mut();

// All in-crate tests that touch process-global heap/root state share one lock.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[inline]
const fn align_down(n: usize, align: usize) -> usize {
    n & !(align - 1)
}

/// Resolve configuration without allocating or mutating runtime state.
/// Preserve the skeleton's fallback for malformed/sub-minimum values.
fn configured_heap_size(value: Option<&str>) -> Result<usize, &'static str> {
    let total = value
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 2 * HEAP_ALIGN)
        .unwrap_or(DEFAULT_HEAP_SIZE);
    // Check the requested size before rounding; never silently accept an
    // unsupported request just because alignment would round it down.
    if total > MAX_HEAP_SIZE {
        return Err("lo_runtime_init: LO_HEAP_SIZE exceeds 128 MiB limit");
    }
    Ok(align_down(total, OBJECT_ALIGN))
}

/// Allocate a single managed region. Called from `lo_runtime_init`.
pub(crate) fn heap_init() {
    let value = std::env::var("LO_HEAP_SIZE").ok();
    let size = configured_heap_size(value.as_deref())
        .unwrap_or_else(|message| crate::abort::runtime_abort(message, 137));
    heap_init_with_size(size);
}

/// Internal explicit-size path also lets direct tests control the heap without
/// depending on a process environment (unavailable in freestanding WASM).
pub(crate) fn heap_init_with_size(size: usize) {
    assert!((2 * HEAP_ALIGN..=MAX_HEAP_SIZE).contains(&size));
    assert_eq!(size % OBJECT_ALIGN, 0);
    let layout = Layout::from_size_align(size, HEAP_ALIGN).expect("valid heap layout");
    // SAFETY: nonzero, checked size; initialization and collection are single-threaded.
    unsafe {
        assert!(HEAP_BASE.is_null(), "heap already initialized");
        let base = std::alloc::alloc_zeroed(layout);
        if base.is_null() {
            crate::abort::runtime_abort("lo_alloc: out of memory", 137);
        }
        HEAP_BASE = base;
        HEAP_LAYOUT = Some(layout);
        HEAP_LIMIT = base.add(size);
        FREE = base;
    }
    crate::gc::init(size);
}

/// Release the heap region. Called from `lo_runtime_shutdown`.
pub(crate) fn heap_shutdown() {
    crate::gc::shutdown();
    // SAFETY: by-value reads of the statics; `dealloc` is paired with the
    // `alloc_zeroed` in `heap_init` using the stored layout. Idempotent: a null
    // base (never inited, or already shut down) is a no-op.
    unsafe {
        let base = HEAP_BASE;
        let layout = HEAP_LAYOUT;
        if !base.is_null() {
            if let Some(layout) = layout {
                std::alloc::dealloc(base, layout);
            }
            HEAP_BASE = ptr::null_mut();
            HEAP_LAYOUT = None;
            HEAP_LIMIT = ptr::null_mut();
            FREE = ptr::null_mut();
        }
    }
}

/// Carve `size` zero-initialized bytes from the managed region and stamp the
/// object header. On a no-fit, collect and retry once; abort with exit 137 only
/// if the compacted live prefix still leaves insufficient space.
///
/// # Safety
/// `class` must point at a valid, live `ClassDescriptor`. `size` must be at least
/// `size_of::<Object>()` so the header fits, and a multiple of 8.
unsafe fn alloc_raw(size: usize, class: *const ClassDescriptor) -> *mut Object {
    let bump = bump_if_fits(size);
    let slot = if !bump.is_null() {
        bump
    } else {
        // OOM path: collect, then retry the bump exactly once.
        crate::gc::lo_gc_collect();
        let retry = bump_if_fits(size);
        if retry.is_null() {
            crate::abort::runtime_abort("lo_alloc: out of memory", 137);
        }
        retry
    };

    ptr::write_bytes(slot, 0, size);
    let obj = slot as *mut Object;
    (*obj).class_descriptor = class;
    (*obj).gc_bits = 0;
    (*obj).flags = 0;
    obj
}

/// Try to bump `size` bytes off the managed region. Returns the carved slot on
/// success, or null if it does not fit (or the heap is uninitialized).
///
/// # Safety
/// Reads/writes the allocator statics on the single runtime thread.
unsafe fn bump_if_fits(size: usize) -> *mut u8 {
    let bump = FREE;
    let limit = HEAP_LIMIT;
    // Address arithmetic for the bounds check avoids forming an out-of-bounds
    // pointer when the request would overflow the heap.
    let Some(new_addr) = (bump as usize).checked_add(size) else {
        return ptr::null_mut();
    };
    if bump.is_null() || new_addr > limit as usize {
        return ptr::null_mut();
    }
    FREE = bump.add(size);
    bump
}

/// Allocate a zero-initialized object of `class.instance_size` bytes
/// (`runtime-abi.md` §3.1). Fills in the header. Triggers a collection and
/// retries once on a full heap before reporting genuine exhaustion.
///
/// Codegen is responsible for finalizing String-typed fields to `LO_EMPTY_STRING`
/// immediately after this returns (see `runtime-abi.md` §3.1); `lo_alloc` is
/// deliberately type-agnostic.
///
/// # Safety
/// `class` must point at a valid `ClassDescriptor` whose `instance_size` is at
/// least `size_of::<Object>()`.
#[no_mangle]
pub unsafe extern "C" fn lo_alloc(class: *const ClassDescriptor) -> *mut Object {
    let size = fixed_size((*class).instance_size);
    alloc_raw(size, class)
}

/// Allocate a `StringObject` with room for `len` inline bytes, header stamped to
/// `LO_STRING_CLASS` and `length` set. The caller fills the `len` data bytes
/// (already zeroed) at `base + string_data_offset()`.
///
/// This is the internal variable-size string allocator used by the provided
/// `lo_read_string` and by the team-implemented `lo_string_*` ops. `lo_alloc`
/// cannot size a string itself (it only knows the class's fixed `instance_size`).
///
/// # Safety
/// Must be called after `heap_init`. Returns a valid `*mut Object`/`*mut
/// StringObject` or aborts on OOM.
pub(crate) unsafe fn bump_alloc_string(len: u32) -> *mut Object {
    let size = string_size(len);
    let obj = alloc_raw(size, &LO_STRING_CLASS);
    let so = obj as *mut StringObject;
    (*so).length = len;
    obj
}

// --- Collector interface ----------------------------------------------------

/// Snapshot of the original allocated prefix. The collector must save this
/// boundary before any phase changes the heap's allocation cursor.
///
/// # Safety
/// Read only on the runtime's single thread, outside initialization/shutdown.
pub(crate) unsafe fn allocated_range() -> (*mut u8, *mut u8) {
    (HEAP_BASE, FREE)
}

/// Exclusive end of the managed region; used to poison only the free tail.
///
/// # Safety
/// Read only on the runtime's single thread while the heap is initialized.
pub(crate) unsafe fn heap_limit() -> *mut u8 {
    HEAP_LIMIT
}

/// Install the end of the packed live prefix after all moves complete.
///
/// # Safety
/// `new_free` must be derived from this heap's base, aligned, at or below the
/// previous free pointer, and delimit a valid packed sequence of live objects.
/// No other runtime operation may run during collection.
pub(crate) unsafe fn commit_free(new_free: *mut u8) {
    assert!(!HEAP_BASE.is_null(), "cannot commit an uninitialized heap");
    let offset = (new_free as usize)
        .checked_sub(HEAP_BASE as usize)
        .expect("compacted boundary precedes heap");
    assert!(
        new_free as usize <= FREE as usize,
        "compaction cannot grow the heap"
    );
    assert_eq!(offset % OBJECT_ALIGN, 0, "unaligned compacted boundary");
    FREE = new_free;
}

/// Bytes currently occupied in the allocated prefix, zero before init.
#[doc(hidden)]
pub fn heap_used() -> usize {
    // SAFETY: runtime state is single-threaded; both pointers are null before init.
    unsafe {
        let (base, free) = allocated_range();
        (free as usize) - (base as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptors::LO_INT_BOX_CLASS;
    use crate::object::string_data_offset;

    struct HeapGuard;
    impl HeapGuard {
        fn new(size: usize) -> Self {
            heap_init_with_size(size);
            Self
        }
    }
    impl Drop for HeapGuard {
        fn drop(&mut self) {
            heap_shutdown();
        }
    }

    #[test]
    fn heap_configuration_checks_bound_before_rounding() {
        assert_eq!(configured_heap_size(None), Ok(DEFAULT_HEAP_SIZE));
        assert_eq!(configured_heap_size(Some("invalid")), Ok(DEFAULT_HEAP_SIZE));
        assert_eq!(configured_heap_size(Some("0")), Ok(DEFAULT_HEAP_SIZE));
        assert_eq!(configured_heap_size(Some("4099")), Ok(4096));
        assert_eq!(configured_heap_size(Some("134217728")), Ok(MAX_HEAP_SIZE));
        assert!(configured_heap_size(Some("134217729")).is_err());
    }

    #[test]
    fn allocation_uses_entire_region_and_never_advances_on_failure() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _heap = HeapGuard::new(64);
        unsafe {
            let (base, free) = allocated_range();
            assert_eq!(base, free);
            assert_eq!(bump_if_fits(40), base); // Already beyond old half capacity.
            assert_eq!(bump_if_fits(24), base.add(40));
            assert_eq!(heap_used(), 64);
            assert!(bump_if_fits(8).is_null());
            assert!(bump_if_fits(usize::MAX).is_null());
            assert_eq!(heap_used(), 64);
        }
    }

    #[test]
    fn allocation_sizes_zeroing_and_compaction_commit_agree() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _heap = HeapGuard::new(256);
        unsafe {
            let (base, _) = allocated_range();
            let object = lo_alloc(&LO_INT_BOX_CLASS);
            let fixed_size = fixed_size(LO_INT_BOX_CLASS.instance_size);
            assert_eq!(object.cast::<u8>(), base);
            for offset in core::mem::size_of::<Object>()..fixed_size {
                assert_eq!(*base.add(offset), 0);
            }
            let string = bump_alloc_string(9);
            assert_eq!(string.cast::<u8>(), base.add(fixed_size));
            assert_eq!((*string.cast::<StringObject>()).length, 9);
            let string_size = string_size(9);
            assert_eq!(heap_used(), fixed_size + string_size);
            ptr::write_bytes(string.cast::<u8>().add(string_data_offset()), 0xEF, 9);
            // Discard the final String, leaving the first object as a valid prefix.
            commit_free(base.add(fixed_size));
            assert_eq!(heap_used(), fixed_size);
            let replacement = bump_alloc_string(9);
            assert_eq!(replacement, string);
            let data = replacement.cast::<u8>().add(string_data_offset());
            for offset in 0..9 {
                assert_eq!(*data.add(offset), 0);
            }
        }
    }

    #[test]
    fn shutdown_resets_bounds_and_is_idempotent() {
        let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        {
            let _heap = HeapGuard::new(64);
        }
        heap_shutdown();
        unsafe {
            assert_eq!(allocated_range(), (ptr::null_mut(), ptr::null_mut()));
            assert!(bump_if_fits(8).is_null());
        }
        assert_eq!(heap_used(), 0);
    }
}
