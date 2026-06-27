#include "runtime.h"

// LoadLocal(offset): load from [x21 + offset] and push onto JIT stack.
// Compiled as: MOVZ W0, #0xDEAD ; B _rune_load_local
// Value hole at MOVZ imm16 (byte 0, bits 20:5), link hole at B (byte 4).
// Offset is byte offset = local_index * 8.
void load_local(void) {
    rune_load_local(0xDEAD);
}
