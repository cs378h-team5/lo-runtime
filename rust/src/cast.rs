//! Class ancestry checks (`runtime-abi.md` §3.5).

use crate::{
    abort::runtime_abort,
    object::{ClassDescriptor, Object},
};

/// Return the object on a valid cast, including null; otherwise abort with exit 101.
///
/// # Safety
/// Non-null `obj` must be valid; `target` and all ancestry descriptors must be valid.
#[no_mangle]
pub unsafe extern "C" fn lo_cast_check(
    obj: *mut Object,
    target: *const ClassDescriptor,
) -> *mut Object {
    if obj.is_null() || lo_instanceof(obj, target) {
        return obj;
    }
    let name = |class: *const ClassDescriptor| {
        String::from_utf8_lossy(core::slice::from_raw_parts(
            (*class).name,
            (*class).name_len as usize,
        ))
    };
    runtime_abort(
        &format!(
            "lo_cast_check: cannot cast {} to {}",
            name((*obj).class_descriptor),
            name(target)
        ),
        101,
    );
}

/// Test ancestry by descriptor identity; null is not an instance of any class.
///
/// # Safety
/// Non-null `obj` must be valid; `target` and all ancestry descriptors must be valid.
#[no_mangle]
pub unsafe extern "C" fn lo_instanceof(obj: *mut Object, target: *const ClassDescriptor) -> bool {
    if obj.is_null() {
        return false;
    }
    let mut class = (*obj).class_descriptor;
    while !class.is_null() {
        if class == target {
            return true;
        }
        class = (*class).parent;
    }
    false
}
