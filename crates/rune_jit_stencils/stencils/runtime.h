// copy-and-patch stencils for the Rune baseline JIT.
//
// Each stencil is a naked C function whose body is a fixed instruction
// sequence with placeholder immediates. At JIT compile time, the
// placeholders are patched with runtime values.
//
// The function uses __attribute__((naked)) so the compiler emits only
// the inline asm — no prologue/epilogue is generated.
//
// Calling convention: x22 = JIT stack pointer. Stencils may clobber
// x0-x17 (scratch/caller-saved). They must preserve x19-x29.
