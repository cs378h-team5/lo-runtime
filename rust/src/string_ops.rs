//! String operations over inline UTF-8 storage (`runtime-abi.md` §3.2).

use crate::{
    abort::runtime_abort,
    alloc::bump_alloc_string,
    object::{string_data_offset, Object, ShadowFrame, StringObject},
    shadow_stack::{lo_pop_frame, lo_push_frame},
};
use core::{ptr, slice};

unsafe fn bytes<'a>(s: *mut Object) -> &'a [u8] {
    slice::from_raw_parts(
        s.cast::<u8>().add(string_data_offset()),
        (*s.cast::<StringObject>()).length as usize,
    )
}

unsafe fn allocate(len: u32, roots: [*mut Object; 2]) -> (*mut Object, [*mut Object; 2]) {
    #[repr(C)]
    struct Frame {
        header: ShadowFrame,
        roots: [*mut Object; 2],
    }
    let mut frame = Frame {
        header: ShadowFrame {
            parent: ptr::null_mut(),
            num_roots: 2,
            roots: [],
        },
        roots,
    };
    // Allocation may move either input; reload both from the registered frame.
    lo_push_frame(ptr::addr_of_mut!(frame).cast());
    let result = bump_alloc_string(len);
    lo_pop_frame();
    (result, frame.roots)
}

fn length(value: Option<u32>) -> u32 {
    value.unwrap_or_else(|| runtime_abort("lo_alloc: out of memory", 137))
}

/// Copy raw UTF-8 bytes into a new string.
///
/// # Safety
/// `source` must point to `len` readable bytes of valid UTF-8, or may be null for zero length.
#[no_mangle]
pub unsafe extern "C" fn lo_string_new(source: *const u8, len: u32) -> *mut Object {
    // Snapshot before managed allocation in case the source points into a movable object.
    let mut copy = Vec::new();
    copy.try_reserve_exact(len as usize)
        .unwrap_or_else(|_| runtime_abort("lo_alloc: out of memory", 137));
    if len != 0 {
        copy.extend_from_slice(slice::from_raw_parts(source, len as usize));
    }
    let result = bump_alloc_string(len);
    ptr::copy_nonoverlapping(
        copy.as_ptr(),
        result.cast::<u8>().add(string_data_offset()),
        copy.len(),
    );
    result
}

/// Return a new string `a + b`.
///
/// # Safety
/// Both arguments must point to valid StringObjects.
#[no_mangle]
pub unsafe extern "C" fn lo_string_concat(a: *mut Object, b: *mut Object) -> *mut Object {
    let len = length(
        (*a.cast::<StringObject>())
            .length
            .checked_add((*b.cast::<StringObject>()).length),
    );
    let (result, [a, b]) = allocate(len, [a, b]);
    let dest = result.cast::<u8>().add(string_data_offset());
    ptr::copy_nonoverlapping(bytes(a).as_ptr(), dest, bytes(a).len());
    ptr::copy_nonoverlapping(bytes(b).as_ptr(), dest.add(bytes(a).len()), bytes(b).len());
    result
}

/// Repeat a string, aborting with exit 120 for a negative count.
///
/// # Safety
/// `s` must point to a valid StringObject.
#[no_mangle]
pub unsafe extern "C" fn lo_string_repeat(s: *mut Object, n: i32) -> *mut Object {
    if n < 0 {
        runtime_abort(&format!("lo_string_repeat: negative count {n}"), 120);
    }
    let len = length((*s.cast::<StringObject>()).length.checked_mul(n as u32));
    let (result, [s, _]) = allocate(len, [s, ptr::null_mut()]);
    let source = bytes(s);
    if !source.is_empty() {
        let dest =
            slice::from_raw_parts_mut(result.cast::<u8>().add(string_data_offset()), len as usize);
        for chunk in dest.chunks_exact_mut(source.len()) {
            chunk.copy_from_slice(source);
        }
    }
    result
}

/// Compare strings by lexicographic UTF-8 byte order.
///
/// # Safety
/// Both arguments must point to valid StringObjects.
#[no_mangle]
pub unsafe extern "C" fn lo_string_compare(a: *mut Object, b: *mut Object) -> i32 {
    bytes(a).cmp(bytes(b)) as i32
}

/// Reverse Unicode codepoints, preserving each codepoint's UTF-8 encoding.
///
/// # Safety
/// `s` must point to a valid StringObject containing valid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn lo_string_reverse(s: *mut Object) -> *mut Object {
    let len = (*s.cast::<StringObject>()).length;
    let (result, [s, _]) = allocate(len, [s, ptr::null_mut()]);
    let source = core::str::from_utf8_unchecked(bytes(s));
    let dest =
        slice::from_raw_parts_mut(result.cast::<u8>().add(string_data_offset()), len as usize);
    let mut offset = 0;
    for ch in source.chars().rev() {
        offset += ch.encode_utf8(&mut dest[offset..]).len();
    }
    result
}
