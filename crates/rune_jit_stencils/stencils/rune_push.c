// Runtime helpers for copy-and-patch stencils.
// Each helper is a regular C function that implements a JIT operation.
// Clang generates the prologue/epilogue; build.rs strips them, keeping just the body.

#include "runtime.h"

// Push val onto the JIT stack at x22, then advance x22.
// Caller may tail-call into this function.
// The inline asm is the "body" of the function that build.rs preserves.
void rune_push(int64_t val) {
    __asm__(
        "str %[val], [x22]\n\t"
        "add x22, x22, #8"
        : : [val] "r" (val) : "x22", "memory"
    );
}
