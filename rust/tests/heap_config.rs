//! Verify initialization's failure path in an isolated process.

use std::process::{Command, Stdio};

#[test]
fn oversized_heap_is_rejected_before_running_program() {
    let output = Command::new(env!("CARGO_BIN_EXE_lo_io_probe"))
        .env("LO_HEAP_SIZE", "134217729")
        .stdin(Stdio::null())
        .output()
        .expect("run runtime initialization probe");
    assert_eq!(output.status.code(), Some(137));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "lo_runtime_init: LO_HEAP_SIZE exceeds 128 MiB limit\n"
    );
}

#[test]
fn live_set_overflow_reports_exact_oom() {
    assert_oom("live-overflow");
}

#[test]
fn oversized_request_reports_exact_oom_after_collection() {
    assert_oom("oversized-request");
}

fn assert_oom(mode: &str) {
    let output = Command::new(env!("CARGO_BIN_EXE_lo_gc_probe"))
        .arg(mode)
        .env("LO_HEAP_SIZE", "256")
        .env("LO_GC_POISON", "1")
        .output()
        .expect("run isolated GC abort probe");
    assert_eq!(output.status.code(), Some(137));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "lo_alloc: out of memory\n"
    );
}
