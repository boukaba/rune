#ifndef RUNE_H
#define RUNE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ABI version for the C surface. */
#define RUNE_ABI_VERSION 2

/* Value kinds passed to host callbacks. */
#define RUNE_UNDEFINED 0
#define RUNE_NULL 1
#define RUNE_BOOLEAN 2
#define RUNE_NUMBER 3
#define RUNE_STRING 4
#define RUNE_OBJECT 5
#define RUNE_FUNCTION 6

/* Fixed-layout value. In callback arguments `string` is borrowed for the
 * duration of the call. Returned strings are copied immediately. Objects are
 * currently opaque and unsupported. */
typedef struct {
    uint32_t kind;
    int32_t boolean;
    double number;
    const char *string;
    /* Non-zero for a retained heap value (string/object/array/function).
     * Release with rune_context_release_value. */
    uint64_t handle;
} RuneValue;

/* Host callback: `user` is the pointer given to rune_context_register_host_fn.
 * Arguments are borrowed; the return value is copied by the engine. */
typedef RuneValue (*RuneHostCallback)(void *user, size_t argc,
                                      const RuneValue *argv);

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

/* Register a global host function. Returns 0 on success, -1 on invalid
 * arguments. The callback and user pointer must outlive the context. */
int rune_context_register_host_fn(void *ctx, const char *name, uint32_t length,
                                  void *user, RuneHostCallback callback);

/* Evaluate and return a value. Heap results carry a retained handle in
 * .handle; release it with rune_context_release_value. */
int rune_context_eval_value(void *ctx, const char *source, RuneValue *result,
                            char **error);

/* Stringify a primitive or retained value. */
int rune_context_value_to_string(void *ctx, RuneValue value, char **out);

/* Read a data property from a retained object/array. */
int rune_context_get_property(void *ctx, RuneValue object, const char *key,
                              RuneValue *result, char **error);

/* Write a data property; value may be a primitive or retained handle. */
int rune_context_set_property(void *ctx, RuneValue object, const char *key,
                              RuneValue value, char **error);

/* Release a retained value handle (handle == 0 is a no-op). */
int rune_context_release_value(void *ctx, RuneValue value);

/* Install timers/promise jobs (idempotent). */
int rune_context_install_runtime(void *ctx);

/* Run due timers and queued promise jobs. */
int rune_context_run_jobs(void *ctx, char **error);

/* Call a retained JavaScript function. Arguments may be primitives or
 * retained handles; the result has the same ownership rules as eval_value. */
int rune_context_call_value(void *ctx, RuneValue func, const RuneValue *args,
                            size_t argc, RuneValue *result, char **error);

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
