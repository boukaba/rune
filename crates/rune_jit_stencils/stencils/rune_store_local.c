// Runtime helper for StoreLocal: pop from JIT stack and store to [x21 + offset].
// Leaves popped value in x0 so codegen can append a push (str+add) to restore stack.
// Body (after stripping): SUB x22,x22,#8; LDR x0,[x22]; STR x0,[x21,x1]
//
// Uses an early-clobber output (%[val]) to force Clang to allocate a register
// different from %[offset] (x0) for the loaded value. The offset stays in x0
// via the first-arg register and is used by STR as the register offset.
// After helper: x0 = popped value (push-back ready), x22 = decremented.
#include "runtime.h"

void rune_store_local(int64_t offset) {
    int64_t dummy;
    __asm__(
        "sub x22, x22, #8\n\t"
        "ldr %[val], [x22]\n\t"
        "str %[val], [x21, %[offset]]\n\t"
        "str %[val], [x22]\n\t"
        "add x22, x22, #8"
        : [val] "=&r" (dummy)
        : [offset] "r" (offset)
        : "x21", "x22", "memory"
    );
}
