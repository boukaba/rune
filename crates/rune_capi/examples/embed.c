/* Build:
 *   cargo build -p rune_capi --release
 *   cc embed.c -Icrates/rune_capi/include -Ltarget/release -lrune_capi \
 *      -Wl,-rpath,$PWD/target/release -o embed
 */
#include "rune.h"
#include <stdio.h>

static RuneValue add(void *user, size_t argc, const RuneValue *argv) {
    RuneValue out = { RUNE_NUMBER, 0, 0.0, NULL, 0 };
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

    char *text = NULL;
    char *error = NULL;
    if (rune_context_eval(ctx, "add(20, 22)", &text, &error) == 0) {
        printf("host: %s\n", text);
    }
    rune_free_string(text);
    rune_free_string(error);

    RuneValue fn, args[2], result;
    text = NULL;
    error = NULL;
    if (rune_context_eval_value(ctx, "(function (a, b) { return a + b; })",
                                &fn, &error) == 0) {
        args[0] = (RuneValue){ RUNE_NUMBER, 0, 20.0, NULL, 0 };
        args[1] = (RuneValue){ RUNE_NUMBER, 0, 22.0, NULL, 0 };
        if (rune_context_call_value(ctx, fn, args, 2, &result, &error) == 0 &&
            rune_context_value_to_string(ctx, result, &text) == 0) {
            printf("call: %s\n", text);
        }
        rune_context_release_value(ctx, fn);
        rune_context_release_value(ctx, result);
    } else {
        printf("error: %s\n", error);
    }
    rune_free_string(text);
    rune_free_string(error);

    RuneValue obj, prop;
    text = NULL;
    error = NULL;
    if (rune_context_eval_value(ctx, "({ x: 42 })", &obj, &error) == 0 &&
        rune_context_get_property(ctx, obj, "x", &prop, &error) == 0 &&
        rune_context_value_to_string(ctx, prop, &text) == 0) {
        printf("prop: %s\n", text);
    }
    rune_free_string(text);
    rune_free_string(error);

    RuneValue new_value = { RUNE_NUMBER, 0, 43.0, NULL, 0 };
    prop.handle = 0;
    text = NULL;
    error = NULL;
    if (rune_context_set_property(ctx, obj, "x", new_value, &error) == 0 &&
        rune_context_get_property(ctx, obj, "x", &prop, &error) == 0 &&
        rune_context_value_to_string(ctx, prop, &text) == 0) {
        printf("set: %s\n", text);
    }
    rune_free_string(text);
    rune_free_string(error);
    rune_context_release_value(ctx, obj);
    rune_context_release_value(ctx, prop);
    rune_context_destroy(ctx);
    return 0;
}
