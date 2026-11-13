/* Build:
 *   cargo build -p rune_capi --release
 *   cc embed.c -Icrates/rune_capi/include -Ltarget/release -lrune_capi \
 *      -Wl,-rpath,$PWD/target/release -o embed
 */
#include "rune.h"
#include <stdio.h>

static RuneValue add(void *user, size_t argc, const RuneValue *argv) {
    RuneValue out = { RUNE_NUMBER, 0, 0.0, NULL };
    (void)user;
    for (size_t i = 0; i < argc; i++) {
        if (argv[i].kind == RUNE_NUMBER) out.number += argv[i].number;
    }
    return out;
}

int main(void) {
    void *ctx = rune_context_create();
    if (!ctx) return 1;
    rune_context_set_instruction_budget(ctx, 1000000);
    rune_context_register_host_fn(ctx, "add", 2, NULL, add);

    char *result = NULL;
    char *error = NULL;
    if (rune_context_eval(ctx, "add(20, 22)", &result, &error) == 0) {
        printf("result: %s\n", result);
    } else {
        printf("error: %s\n", error);
    }
    rune_free_string(result);
    rune_free_string(error);
    rune_context_destroy(ctx);
    return 0;
}
