#include "runtime.h"

// LoadConst(imm16): push a raw 64-bit constant onto JIT stack.
// Compiled as: MOV W0, #0xDEAD ; B _rune_push (value hole at [0], link hole at [4])
// Value is the raw tagged representation (0=undefined, 2=null, 4=false, 6=true, etc.).
// Codegen emits MOVZ bytes, patches to 64-bit (sf=1), then inlines STR+ADD helper body.
void load_const(void) {
    rune_push(0xDEAD);
}
