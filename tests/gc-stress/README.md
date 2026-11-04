# GC stress tests (P3)

The tests we use to hunt collector bugs beyond the course's suites. They come in
two kinds:

- **LO programs** in `tests/gc-stress/LO-3/ValidPrograms/`: five tests in the
  suite's header-contract format (our contributed tests).
- **Rust collector tests** that call the runtime's entry points directly:
  `rust/src/gc.rs` (unit tests), `rust/tests/collector_probe.rs` and
  `rust/tests/string_type_regression.rs`.

## What each kind covers

| Handout category | Where |
|---|---|
| Small-heap runs | The LO tests run natively at their declared heaps (64 KiB to 16 MiB). The Rust tests run at 4 KiB (`gc.rs`) and 1 KiB with freed space poisoned (`collector_probe`, `LO_GC_POISON=1`). |
| Allocation-pressure loops | Every LO test allocates about 3 million objects. `allocation_pressure_triggers_gc` (`gc.rs`) and `collector_stress` (`collector_probe`) allocate far past the heap. |
| Object graphs with cycles | `collector_stress` keeps a two-node cycle, rooted from two frames, alive across repeated collections. |
| Cross-generation pointers | Not applicable: Lisp-2 has one generation. |
| Forced collections (`lo_gc_collect`) | The `gc.rs` tests, `collector_stress` and `unicode_and_type_operations` force collections at chosen points and check what moved. |
| Out of memory | `live_set_overflow_aborts` grows a live list in a 1 KiB heap and checks for exit 137 and `lo_alloc: out of memory`. |

## The five LO tests

Each targets a different collector invariant that no public stress test is
written for, and fails on a copy of our runtime with that bug planted.

| Test | Invariant | Planted bug it catches |
|---|---|---|
| `gc_01_unmoved_anchor_fixup` | Fixup rewrites the fields of every live object, including ones that do not move | Fixup skips objects whose address is unchanged |
| `gc_02_overlapping_slide` | An object that slides down by less than its own size is copied overlap-safely | Copying high-to-low, or copying the tail block first |
| `gc_03_survivors_then_die` | Objects that survived one collection are reclaimed once they die | Mark bits left set after collection |
| `gc_04_deep_list_marking` | Marking uses an explicit worklist, not one stack frame per pointer followed | Recursive marking (stack overflow on a 170,000-link chain) |
| `gc_05_every_pointer_field` | Scanning visits every pointer offset the class descriptor lists, and nothing else | Scanning only the first field, skipping the last, or assuming pointer fields are contiguous |

Each `.lo` file carries the stress header (`// gc stress heap: <bytes>`) and has
a `.expected.out` with its exact stdout. As in the public set, peak live data is
at most a quarter of the declared heap, so a semispace collector also passes.

## Running them

From the repository root, in the course container.

**LO tests.** After `source p3-grading-contract.sh && lo-build`:

```sh
bash tests/gc-stress/run.sh native   # native, at the default heap and at each test's declared heap
bash tests/gc-stress/run.sh wasm     # WASM, at the default heap
bash tests/gc-stress/run.sh          # both
```

**Rust tests.**

```sh
cargo test --manifest-path rust/Cargo.toml                       # native, default and 4 KiB heaps
bash tests/run-collector.sh                                      # also: probe at 1 KiB with poisoning, and the probe on WASM
```

**Small heaps are native only.** A WASM module has no environment, so
`LO_HEAP_SIZE` cannot reach it, and WASM always runs at the default 16 MiB
heap. On WASM, collections come from allocating past that heap and, in the
probe, from calling `lo_gc_collect` directly.
