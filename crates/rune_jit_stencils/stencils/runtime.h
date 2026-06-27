// copy-and-patch stencils for the Rune baseline JIT.
//
// Each stencil is a C function that calls runtime helpers. At build time Clang
// compiles the function into machine code; hookd strips the prologue/epilogue
// and identifies patchable holes. At JIT time the stencil body is memcpy'd into
// the code buffer and the holes are patched with runtime values.
//
// Calling convention: x22 = JIT stack pointer. Stencils may clobber
// x0-x17 (scratch/caller-saved). They must preserve x19-x29.

#include <stdint.h>

// ── Runtime helpers ──────────────────────────────────────────────────────

// Push val onto the JIT stack at x22, then advance x22.
void rune_push(int64_t val);
