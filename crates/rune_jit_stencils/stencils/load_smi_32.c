#include "runtime.h"

// LoadSmi(imm32): push Smi(imm32) onto JIT stack.
// Compiled as: MOVZ W0, #0xBEEF ; MOVK W0, #0xDEAD, LSL #16 ; B _rune_push
// Value holes at [0] and [4], link hole at [8].
void load_smi_32(void) {
    rune_push(0xDEADBEEF);
}
