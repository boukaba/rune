// Runtime helper for LoadLocal: load from [x21 + offset] and push onto JIT stack.
// Body (after stripping prologue/epilogue): LDR x0,[x21,x0]; STR x0,[x22]; ADD x22,x22,#8
// On entry: x0 = offset (byte offset from locals pointer).
// Clobbers: x0 (loaded value), x22 (advanced), memory.

#include "runtime.h"

void rune_load_local(int64_t offset) {
    __asm__(
        "ldr x0, [x21, %[offset]]\n\t"
        "str x0, [x22]\n\t"
        "add x22, x22, #8"
        : : [offset] "r" (offset) : "x0", "x21", "x22", "memory"
    );
}
