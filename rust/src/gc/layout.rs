//! Shared allocation and heap-walk sizes. Objects are rounded to eight bytes on
//! both targets; flexible String tails start at their field offset, not sizeof.

use super::header::OBJECT_ALIGN;
use crate::{
    descriptors::LO_STRING_CLASS,
    object::{string_data_offset, Object, StringObject},
};

fn align_size(size: usize) -> usize {
    size.checked_add(OBJECT_ALIGN - 1)
        .filter(|size| *size <= isize::MAX as usize)
        .unwrap_or_else(|| crate::abort::runtime_abort("lo_alloc: out of memory", 137))
        & !(OBJECT_ALIGN - 1)
}

pub(crate) fn fixed_size(instance_size: u32) -> usize {
    align_size(instance_size as usize)
}

pub(crate) fn string_size(length: u32) -> usize {
    let size = string_data_offset()
        .checked_add(length as usize)
        .unwrap_or_else(|| crate::abort::runtime_abort("lo_alloc: out of memory", 137));
    align_size(size)
}

/// Size of either a live or dead object in the original packed heap.
///
/// # Safety
/// `object` must point at a valid object with an intact descriptor and, for
/// Strings, an initialized length. The object's header must not be a tombstone.
pub(crate) unsafe fn object_size(object: *const Object) -> usize {
    let class = (*object).class_descriptor;
    if core::ptr::eq(class, &LO_STRING_CLASS) {
        string_size((*object.cast::<StringObject>()).length)
    } else {
        fixed_size((*class).instance_size)
    }
}
