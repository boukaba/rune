#include "runtime.h"

// LoadSmi(imm16): push Smi(imm16) onto JIT stack.
// Compiled as: MOV W0, #0xDEAD ; B _rune_push (value hole at [0], link hole at [4])
void load_smi_16(void) {
    rune_push(0xDEAD);
}
