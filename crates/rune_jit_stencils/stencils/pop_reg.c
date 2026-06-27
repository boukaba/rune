#include "runtime.h"

// Pop from JIT stack into x0.
__attribute__((naked))
void pop_reg(void) {
    __asm__(
        "sub x22, x22, #8\n\t"
        "ldr x0, [x22]\n\t"
        "ret"
        :
        :
        : "x0", "x22", "memory"
    );
}
