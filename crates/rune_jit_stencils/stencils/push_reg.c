#include "runtime.h"

// Push x0 onto JIT stack. x0 is already loaded.
__attribute__((naked))
void push_reg(void) {
    __asm__(
        "str x0, [x22]\n\t"
        "add x22, x22, #8\n\t"
        "ret"
        :
        :
        : "x0", "x22", "memory"
    );
}
