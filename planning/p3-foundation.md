# Lisp-2 foundation

Cheney stays active until all phases are integrated. The ABI and compiler pipeline
are unchanged. Rust is the shared native/WASM implementation.

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

The activation PR replaces semispaces with the full configured heap and wires
mark -> forwarding -> fixup -> slide into allocation/collect/retry/OOM. Keep
Cheney separately as reference code. Scratch is not instantiated by this PR.
Side metadata uses O(heap capacity) reserved space; lookup is O(log live objects).

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
