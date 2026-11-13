/* Build:
 *   cargo build -p rune_capi --release
 *   cc embed.c -Icrates/rune_capi/include -Ltarget/release -lrune_capi \
 *      -Wl,-rpath,$PWD/target/release -o embed
 */
#include "rune.h"
#include <stdio.h>

int main(void) {
    void *ctx = rune_context_create();
    if (!ctx) return 1;
    rune_context_set_instruction_budget(ctx, 1000000);

    char *result = NULL;
    char *error = NULL;
    if (rune_context_eval(ctx, "var n = 40; n + 2", &result, &error) == 0) {
        printf("result: %s\n", result);
    } else {
        printf("error: %s\n", error);
    }
    rune_free_string(result);
    rune_free_string(error);
    rune_context_destroy(ctx);
    return 0;
}
