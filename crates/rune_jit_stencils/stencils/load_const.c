#include "runtime.h"

// Load a constant value onto the JIT stack.
// Naked asm so Clang emits exactly MOVZ + STR + ADD — no prologue/epilogue.
// Value hole at MOVZ imm16 (byte 0, bits 20:5).
__attribute__((naked)) void load_const(void) {
    __asm__(
        "movz x0, #0xDEAD\n\t"
        "str x0, [x22]\n\t"
        "add x22, x22, #8"
    );
}
