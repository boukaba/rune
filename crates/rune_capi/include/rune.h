#ifndef RUNE_H
#define RUNE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ABI version for the C surface. */
#define RUNE_ABI_VERSION 1

/* Human-readable engine version (static storage). */
const char *rune_version(void);

/* Create/destroy an isolated JavaScript context. `bytes` is the semispace
 * size; use 16 MiB for typical workloads. Returns NULL on invalid size. */
void *rune_context_create(void);
void *rune_context_create_with_heap(size_t bytes);
void rune_context_destroy(void *ctx);

/* Execution limits. Both raise a catchable RangeError once when exhausted.
 * Setting a limit disables the JIT while it is active. */
void rune_context_set_instruction_budget(void *ctx, uint64_t instructions);
void rune_context_set_time_budget_ms(void *ctx, uint64_t milliseconds);

/* Opt-in JIT (disabled by default). */
void rune_context_set_jit(void *ctx, int enabled);

/* Evaluate UTF-8 source. On success returns 0 and stores the JS string
 * representation of the result in *result (may be NULL on error). On a
 * parse/runtime error returns 1 and stores the error text in *error.
 * Returns -1 on invalid arguments. Strings must be freed with
 * rune_free_string. Pointers may be NULL if the caller does not need them. */
int rune_context_eval(void *ctx, const char *source,
                      char **result, char **error);

/* Free a string returned by this API. NULL is a no-op. */
void rune_free_string(char *text);

#ifdef __cplusplus
}
#endif

#endif /* RUNE_H */
