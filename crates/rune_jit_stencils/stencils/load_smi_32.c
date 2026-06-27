#include "runtime.h"

// LoadSmi(imm32): push Smi(imm32) onto JIT stack.
// imm32 placeholder split across MOVZ (lower 16) + MOVK (upper 16).
__attribute__((naked))
void load_smi_32(void) {
    __asm__(
        "mov x0, #0xDEAD\n\t"
        "movk x0, #0xBEEF, lsl #16\n\t"
        "str x0, [x22]\n\t"
        "add x22, x22, #8\n\t"
        "ret"
        :
        :
        : "x0", "x22", "memory"
    );
}
