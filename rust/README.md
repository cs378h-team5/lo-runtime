# LO Runtime — Rust

Rust implementation of the [LO runtime ABI](../runtime-abi.md), producing native
and WASM libraries from one source. Strings, casts, and the sliding mark-compact
collector are implemented.

## Collection and heap configuration

The single managed region defaults to 16 MiB. Native `LO_HEAP_SIZE` overrides it
at initialization, up to 128 MiB. Larger requests report
`lo_runtime_init: LO_HEAP_SIZE exceeds 128 MiB limit` and exit 137 on native.
Malformed and sub-minimum values retain the skeleton's default fallback.

Collection follows L17: mark reachable objects, encode packed destinations,
repair roots and live fields at their original locations, slide in increasing
source order with overlap-safe copies, clear survivor GC state, and commit the
new allocation boundary. `gc_bits` uses its low eight bits for state and high
24 bits for destination offsets divided by eight. Descriptors and reserved
flags remain intact. There is no forwarding side table.

The traversal stack is reserved at heap initialization, not during collection.
Its worst-case reservation at the default managed capacity is 8 MiB on native
64-bit and 4 MiB on WASM32, additional to the managed heap. Collection allocates
neither managed objects nor scratch storage. A reentrancy assertion protects the
whole sequence. After an exhausted allocation, the allocator collects and retries
once, then emits exactly `lo_alloc: out of memory` with native exit 137 or the
specified WASM message/trap if there is still insufficient space.

Native `LO_GC_POISON=1` fills the entire free tail with `0xEF` after each collection.
Configuration is read during initialization so debug environment reads cannot
allocate during collection. A stale pointer into another live object may still
evade poisoning; poisoning complements reference tests rather than proving that
all roots are correct.

## File structure

| File | Responsibility |
| --- | --- |
| `src/alloc.rs` | Region ownership, bump allocation, original bounds, final commit |
| `src/gc.rs` | Runtime ABI entry points, phase order, reentrancy, poisoning |
| `src/gc/header.rs` | Marks and checked destination encoding |
| `src/gc/layout.rs` | Shared fixed-object and variable-String sizing |
| `src/gc/mark.rs` | Iterative marking and traversal stack lifecycle |
| `src/gc/plan.rs` | Packed destination assignment |
| `src/gc/repair.rs` | Root and live-field reference fixup |
| `src/gc/slide.rs` | Ordered overlapping moves and GC-state reset |
| `src/gc/trace.rs` | Shared reference-slot enumeration |
| `src/gc/cheney.rs` | Unchanged provided reference, excluded from compilation |
| `src/gc/tests.rs` | Complete-collection and allocation-pressure tests |
| `src/gc/*/tests.rs` | Direct phase tests |
| `src/bin/lo_gc_probe.rs` | Isolated OOM test driver, not runtime library code |

Other modules provide ABI types/layout locks, built-in descriptors, shadow roots,
I/O, strings, casts, initialization, and abort messages. `unsafe` pointer walks
rely on valid descriptors, pointer maps, object starts, and registered roots per
the ABI. Ordinary native registers/spills are not GC roots.

## Build and test

From this directory:

```sh
cargo build --release --lib
rustup target add wasm32-unknown-unknown
cargo build --release --lib --target wasm32-unknown-unknown
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The native archive is `target/release/liblo_runtime.a`; the WASM archive is
`target/wasm32-unknown-unknown/release/liblo_runtime.a`. Native execution for the
course targets Linux x86-64; macOS ARM64 unit tests are additional checks.

From the **compiler repository root** (not its parent, which also contains a
separate runtime checkout), run Linux x86-64 runtime tests on an ARM Mac:

```sh
docker run --rm --platform linux/amd64 -v "$PWD":/work -w /work rust:1-slim \
  cargo test --offline --release --manifest-path lo-runtime/rust/Cargo.toml \
  --target-dir target/p3-runtime-linux
```

The 38 runtime tests cover phase invariants, the L17 shelter graph, nested roots,
cycles/sharing, exact worklist capacity, a 30,000-node chain, dead String sizing,
overlapping movement, repeated small-heap collections, all-live/all-dead heaps,
UTF-8 string operations under pressure, freed-tail poisoning and zeroing on reuse,
and exact live-set/oversized-request OOM behavior in separate processes. These
pass on macOS ARM64 and Linux x86-64 (release). End-to-end validation passes
135/135 native/interpreter/TAC/WASM comparisons plus the large allocation-pressure
fixture, with native using an 8 KiB heap and WASM using its default heap.

For end-to-end native checks with poisoning and an 8 KiB heap:

```sh
python3 -B codegen-testing/native.py --docker --heap-size 8192
```

To build the WASM archive inside an amd64 container without changing host Rust:

```sh
docker run --rm --platform linux/amd64 -v "$PWD":/work -w /work rust:1-slim sh -c \
  'rustup target add wasm32-unknown-unknown && cargo build --offline --release --lib --target wasm32-unknown-unknown --manifest-path lo-runtime/rust/Cargo.toml --target-dir target/p3-runtime-linux --config profile.release.lto=false'
```

Then compare native, WASM, interpreter, and TAC:

```sh
python3 -B codegen-testing/native.py --docker --heap-size 8192 \
  --wasm-runtime target/p3-runtime-linux/wasm32-unknown-unknown/release/liblo_runtime.a
python3 -B codegen-testing/native.py --docker --heap-size 8192 --timeout 120 \
  --wasm-runtime target/p3-runtime-linux/wasm32-unknown-unknown/release/liblo_runtime.a \
  codegen-testing/gc
```

The GC fixture's 300,000 iterations allocate beyond the default WASM heap, forcing
collection without pretending host environment variables configure the guest.
WASM reduced-heap configuration and direct execution of every runtime fixture on
WASM remain separate validation work. No P3 submission-completeness claim follows
from these checks; the design note and contributed-test requirements still apply.
