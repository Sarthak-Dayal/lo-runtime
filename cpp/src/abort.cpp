#include "lo_runtime/abort.h"

#include "lo_runtime/exit_codes.h"

#include <cstddef>
#include <cstdio>
#include <cstdlib>
#include <string>

// Runtime aborts (runtime-abi.md §3.8). Native: write the message to stderr and
// exit with the ABI status code. WASM: emit the same message via the host
// `host.write_stderr` import (§3.7), then `__builtin_trap()` — so the
// accompanying stderr is present on both targets, in the same format, for the
// harness to match on (delta D-B3).

#ifdef __wasm__
// Host stderr-write import (runtime-abi.md §3.7): writes `len` bytes of linear
// memory at `ptr` to the process's stderr. The freestanding wasm build is
// compiled with -D__wasm__ and links no libc, so abort.cpp declares the import
// directly (it is not force-included via wasm_imports.h the way io.cpp is).
extern "C" __attribute__((import_module("host"), import_name("host_write_stderr"))) void
host_write_stderr(const char *ptr, int len);

namespace {
// Bounded C-string length — no libc `strlen` on the freestanding wasm build.
int cstr_len(const char *s) {
  int n = 0;
  while (s[n] != '\0' && n < 4096) {
    ++n;
  }
  return n;
}
} // namespace
#endif

namespace lo {

void runtime_abort(const char *msg, int code) {
#ifdef __wasm__
  (void)code;
  // Emit the documented §3.8 message before the trap (delta D-B3) so the WASM
  // stderr matches native; the host buffers it and prints it once.
  host_write_stderr(msg, cstr_len(msg));
  __builtin_trap();
#else
  std::fprintf(stderr, "%s\n", msg);
  std::exit(code);
#endif
}

} // namespace lo

extern "C" void lo_abort_null_receiver(const char *method_name, std::uint32_t method_name_len) {
#ifdef __wasm__
  // Build "lo_abort_null_receiver: cannot dispatch <method>" with no libc (the
  // freestanding wasm build links neither libc nor a usable std::string), then
  // route through runtime_abort, which emits it via the host import (delta D-B3).
  char buf[256];
  const char prefix[] = "lo_abort_null_receiver: cannot dispatch ";
  const std::size_t prefix_len = sizeof(prefix) - 1;
  __builtin_memcpy(buf, prefix, prefix_len);
  std::size_t n = prefix_len;
  if (method_name != nullptr) {
    std::size_t take = method_name_len;
    if (take > sizeof(buf) - n - 1) {
      take = sizeof(buf) - n - 1;
    }
    __builtin_memcpy(buf + n, method_name, take);
    n += take;
  } else {
    const char unknown[] = "<unknown>";
    __builtin_memcpy(buf + n, unknown, sizeof(unknown) - 1);
    n += sizeof(unknown) - 1;
  }
  buf[n] = '\0';
  lo::runtime_abort(buf, lo::kExitNullDispatch);
#else
  std::string msg = "lo_abort_null_receiver: cannot dispatch ";
  if (method_name != nullptr) {
    msg.append(method_name, method_name_len);
  } else {
    msg.append("<unknown>");
  }
  std::fprintf(stderr, "%s\n", msg.c_str());
  std::exit(lo::kExitNullDispatch);
#endif
}
