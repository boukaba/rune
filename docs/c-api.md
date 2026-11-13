# C API

`rune_capi` exposes a small, panic-safe C ABI for embedding Rune. The public
header is [`crates/rune_capi/include/rune.h`](../crates/rune_capi/include/rune.h).

## Build

```sh
cargo build -p rune_capi --release
cc app.c -Icrates/rune_capi/include -Ltarget/release -lrune_capi \
   -Wl,-rpath,$PWD/target/release -o app
```

`rune_capi` builds a `cdylib`, `staticlib`, and `rlib`. The ABI version is
`RUNE_ABI_VERSION` (currently 1).

## Contract

| Function | Behavior |
|---|---|
| `rune_version()` | Static NUL-terminated version string. |
| `rune_context_create()` | 16 MiB context; NULL only on allocation failure. |
| `rune_context_create_with_heap(bytes)` | Explicit semispace; NULL for `bytes == 0`. |
| `rune_context_destroy(ctx)` | Frees the context; NULL is a no-op. |
| `rune_context_set_instruction_budget(ctx, n)` | Catchable RangeError once; disables JIT while active. |
| `rune_context_set_time_budget_ms(ctx, ms)` | Deadline checked every 4096 instructions; disables JIT. |
| `rune_context_set_jit(ctx, enabled)` | Opt-in JIT (off by default). |
| `rune_context_eval(ctx, src, &result, &error)` | 0 success, 1 parse/runtime/panic error, -1 invalid argument. |
| `rune_free_string(text)` | Frees result/error strings. |

Results are returned as JavaScript string representations. Callers own result
and error strings; pass NULL output slots if a value is not needed. Rust panics
are caught and reported as `"engine panic"` rather than unwinding across FFI.

## Limitations

- No host-function callback bridge or `Value` handle API yet; only evaluation
  and string results are exposed.
- Timers, `fetch`, and the event-loop jobs are not installed by the C API.
- Contexts are single-threaded; do not share one context across threads.
- Raw invalid pointers are undefined behavior (standard C ABI rules); the API
  null-checks but cannot validate arbitrary addresses.
