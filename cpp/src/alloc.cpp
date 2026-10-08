#include "lo_runtime/alloc.h"

#include "lo_runtime/abort.h"
#include "lo_runtime/descriptors.h"
#include "lo_runtime/exit_codes.h"
#include "lo_runtime/gc.h"

#include <cstdlib>
#include <cstring>
#include <vector>

// Semispace bump allocator backing lo_alloc (runtime-abi.md §3.1), with the WS-2
// Cheney collector wired into the OOM path. The managed heap is split into two
// equal semispaces (runbook WS-2 §2.1): exactly one is active (the from-space),
// and allocation is a bump pointer within it. On a no-fit, lo_alloc forces a
// collection and retries once; only a genuine live-set overflow aborts (exit 137).
//
// Total heap size inherits the WS-1 mechanism (decision (B)): 16 MiB default,
// overridable via LO_HEAP_SIZE at init; each semispace is half the total. The
// geometry matches the Rust reference byte-for-byte. Single-threaded by spec.

namespace {

std::vector<std::uint8_t> g_heap; // backing region for both semispaces
std::uint8_t *g_from_base = nullptr;
std::uint8_t *g_from_limit = nullptr;
std::uint8_t *g_free = nullptr;
std::uint8_t *g_to_base = nullptr;
std::uint8_t *g_to_limit = nullptr;

constexpr std::size_t kDefaultHeapSize = std::size_t{16} * 1024 * 1024;
constexpr std::size_t kHeapAlign = 16;
// Every allocation's size is rounded up to this so Object's 8-byte pointer
// field (native) stays aligned within the semispace.
constexpr std::size_t kObjectAlign = 8;

std::size_t align_up(std::size_t n, std::size_t a) { return (n + a - 1) & ~(a - 1); }
std::size_t align_down(std::size_t n, std::size_t a) { return n & ~(a - 1); }

std::size_t read_heap_size() {
  const char *v = std::getenv("LO_HEAP_SIZE");
  if (v != nullptr) {
    char *end = nullptr;
    const unsigned long long n = std::strtoull(v, &end, 10);
    if (end != v && n >= 2 * kHeapAlign) {
      return static_cast<std::size_t>(n);
    }
  }
  return kDefaultHeapSize;
}

// Try to bump `size` bytes off the active semispace; nullptr if it does not fit.
std::uint8_t *bump_if_fits(std::size_t size) {
  if (g_free == nullptr || size > static_cast<std::size_t>(g_from_limit - g_free)) {
    return nullptr;
  }
  std::uint8_t *p = g_free;
  g_free += size;
  return p;
}

Object *alloc_raw(std::size_t size, const ClassDescriptor *cls) {
  std::uint8_t *slot = bump_if_fits(size);
  if (slot == nullptr) {
    // OOM path: collect, then retry the bump exactly once.
    lo_gc_collect();
    slot = bump_if_fits(size);
    if (slot == nullptr) {
      lo::runtime_abort("lo_alloc: out of memory", lo::kExitOom);
    }
  }
  std::memset(slot, 0, size);
  auto *o = reinterpret_cast<Object *>(slot);
  o->class_descriptor = cls;
  o->gc_bits = 0;
  o->flags = 0;
  return o;
}

} // namespace

namespace lo {

void heap_init() { heap_init_with(read_heap_size()); }

void heap_init_with(std::size_t total) {
  // Each semispace is half the total, rounded down to the heap alignment so both
  // semispace bases stay aligned; the backing vector holds `2 * semi` exactly.
  // std::vector<std::uint8_t> storage is suitably aligned for the headers.
  const std::size_t semi = align_down(total / 2, kHeapAlign);
  const std::size_t region = semi * 2;
  g_heap.assign(region, 0);
  std::uint8_t *base = g_heap.data();
  g_from_base = base;
  g_from_limit = base + semi;
  g_free = base;
  g_to_base = base + semi;
  g_to_limit = base + region;
}

void heap_shutdown() {
  g_heap.clear();
  g_heap.shrink_to_fit();
  g_from_base = nullptr;
  g_from_limit = nullptr;
  g_free = nullptr;
  g_to_base = nullptr;
  g_to_limit = nullptr;
}

Object *bump_alloc_string(std::uint32_t len) {
  const std::size_t size = align_up(string_data_offset() + len, kObjectAlign);
  Object *o = alloc_raw(size, &LO_STRING_CLASS);
  auto *so = reinterpret_cast<StringObject *>(o);
  so->length = len;
  return o;
}

std::uint8_t *from_base() { return g_from_base; }
std::uint8_t *from_limit() { return g_from_limit; }
std::uint8_t *to_space_base() { return g_to_base; }

void flip(std::uint8_t *new_free) {
  std::uint8_t *old_from_base = g_from_base;
  std::uint8_t *old_from_limit = g_from_limit;
  g_from_base = g_to_base;
  g_from_limit = g_to_limit;
  g_to_base = old_from_base;
  g_to_limit = old_from_limit;
  g_free = new_free;
}

std::size_t heap_used() {
  return g_from_base == nullptr ? 0 : static_cast<std::size_t>(g_free - g_from_base);
}

} // namespace lo

extern "C" Object *lo_alloc(const ClassDescriptor *cls) {
  const std::size_t size = align_up(static_cast<std::size_t>(cls->instance_size), kObjectAlign);
  return alloc_raw(size, cls);
}
