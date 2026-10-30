//! Isolated abort-path driver for the runtime GC integration tests.

use lo_runtime::{Object, ShadowFrame};

#[repr(C)]
struct Frame {
    header: ShadowFrame,
    roots: [*mut Object; 64],
}

fn main() {
    lo_runtime::lo_runtime_init();
    match std::env::args().nth(1).as_deref() {
        Some("live-overflow") => unsafe {
            let mut frame = Frame {
                header: ShadowFrame {
                    parent: core::ptr::null_mut(),
                    num_roots: 64,
                    roots: [],
                },
                roots: [core::ptr::null_mut(); 64],
            };
            lo_runtime::lo_push_frame(core::ptr::addr_of_mut!(frame.header));
            for index in 0..64 {
                frame.roots[index] = lo_runtime::lo_alloc(&lo_runtime::LO_INT_BOX_CLASS);
            }
            std::hint::black_box(&frame);
            lo_runtime::lo_pop_frame();
        },
        Some("oversized-request") => unsafe {
            let string = lo_runtime::lo_string_new(b"x".as_ptr(), 1);
            lo_runtime::lo_string_repeat(string, 1_000_000);
        },
        _ => panic!("expected live-overflow or oversized-request"),
    }
    lo_runtime::lo_runtime_shutdown();
}
