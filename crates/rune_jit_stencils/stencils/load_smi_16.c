#include "runtime.h"

// LoadSmi(imm16): push Smi(imm16) onto JIT stack.
// imm16 is a placeholder — patched at build runtime.
__attribute__((naked))
void load_smi_16(void) {
    __asm__(
        "mov x0, #0xDEAD\n\t"
        "str x0, [x22]\n\t"
        "add x22, x22, #8\n\t"
        "ret"
        :
        :
        : "x0", "x22", "memory"
    );
}
