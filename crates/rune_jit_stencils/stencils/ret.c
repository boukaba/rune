#include "runtime.h"

// Return from subroutine.
__attribute__((naked))
void ret_stencil(void) {
    __asm__(
        "ret"
        :
        :
        :
    );
}
