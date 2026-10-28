# Lisp-2 foundation

Lisp-2 is now active. The ABI and compiler pipeline are unchanged. Rust is the
shared native/WASM implementation. `rust/reference/` preserves Cheney and its
semispace allocator as uncompiled reference code.

## Phase contracts

1. Initialize one contiguous managed heap and reserve `compaction::Scratch`
   outside it. Capacity is bounded by heap size / minimum object header size.
   Route reservation failure through the runtime OOM path. Never grow scratch
   vectors during collection; check capacity before push. Clear them between cycles.
2. Mark via shadow-stack roots and descriptor pointer offsets. Mark before enqueue
   so each object enters the worklist once. Use `gc_bits`, preserving `flags` and
   descriptor pointers. A re-entry guard belongs in the collection orchestrator.
3. Walk the allocated prefix using `heap_layout` sizes. Append live-object
   `Relocation` entries in old-address order, assigning packed destinations.
   The table uses heap-relative offsets and exact-start binary search.
4. Rewrite roots and fields of live objects before moving bytes. Null and valid
   immortal references remain unchanged. Descriptors and code pointers never move.
5. Slide in ascending address order using overlapping copies, clear collector
   metadata, set the bump pointer, then poison the freed tail in debug mode.

The collector uses the full configured heap and wires mark -> forwarding ->
fixup -> slide into allocation/collect/retry/OOM. Scratch is reserved at init,
reused between collections, and released at shutdown. Collection never grows it.
Side metadata uses O(heap capacity) reserved space; lookup is O(log live objects).
On native, the worst-case reservation is approximately twice the managed heap
(one worklist offset and three relocation words per minimum-size object).
This trades memory for intact descriptors and straightforward reference fixup.

## Runtime safety

String operations copy inputs into fallibly allocated unmanaged buffers before
managed allocation. Validate result length and aligned size before reserving.
Managed slices must not be retained across allocation. Preserve the descriptor-chain
cast implementation and its null/abort behavior. No new root abstraction is needed
for this copy-before-allocation strategy.

## Validation

Cover checked sizes, Unicode, empty repeat, aliases/cycles, repeated relocation,
small heaps and documented aborts on native and WASM. Each collector owner supplies
phase tests. The integration owner runs both full suites against our runtime.
WASM does not automatically receive host environment variables: confirm the course
LO_HEAP_SIZE mechanism before introducing a new host import.

Run `WASMRUN=/path/to/wasmrun bash tests/run-collector.sh` in the course container
with Rust's `wasm32-unknown-unknown` target installed. The shared probe exercises
nested frames, aliased roots, cycles, static/null references, overlapping string
moves, repeated collection, allocation pressure, and zeroing after reclamation
on both targets. Native tests also check a live set exceeding half the heap,
tiny heaps with tail poisoning, and exact OOM message/status. WASM pressure
exceeds the default heap; it does not assume host environment propagation.

## Remaining P3 integration work

- Confirm the course mechanism for small-heap configuration on WASM.
- Run compiler-generated conformance and stress programs against our runtime
  on both targets, including string/type abort paths and WASM OOM trapping.
- Add at least five stress programs in the suite's contributed-test format.
- Add the recommended debug heap checker and collection diagnostics. Current
  bounds assertions are not a complete pointer-validity checker.
- Write the final 1–2 page design note, including an actual bug investigation,
  and update submission README instructions. This foundation note is not that
  final deliverable.
