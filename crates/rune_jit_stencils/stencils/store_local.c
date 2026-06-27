#include "runtime.h"

// StoreLocal(offset): pop from JIT stack and store to [x21 + offset].
// Compiled as: MOVZ W0, #0xDEAD ; B _rune_store_local
// Value hole at MOVZ imm16 (byte 0, bits 20:5), link hole at B (byte 4).
// Offset is byte offset = local_index * 8.
void store_local(void) {
    rune_store_local(0xDEAD);
}
