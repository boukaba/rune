# Rune

[![License: MIT/Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.83%2B-orange)](https://www.rust-lang.org)
[![CI](https://github.com/boukaba/rune/actions/workflows/ci.yml/badge.svg)](https://github.com/boukaba/rune/actions)

**A Rust-native JavaScript engine with AOT-first persistent compilation.**  

Cold starts in **~4–7ms** — 5–8× faster than Node.js. Designed for serverless and edge environments where predictable latency, minimal memory, and instant warm boots matter more than peak throughput.

## Why Rune?

| Characteristic | Rune | V8 (Node) |
|---|---|---|
| **Cold start** (empty script) | **~4–7 ms** | ~26–33 ms |
| **Compilation model** | AOT + persistent native cache | JIT-only, re-compiles on every restart |
| **Shape system** | Immutable, content-addressed | Mutable hidden classes (transitions) |
| **Cache validity** | Forever (content-addressed) | None (no cross-run caching) |
| **Property IC** | SIMD (NEON/SSE), no megamorphic cliff | Linear probe, megamorphic cliff |
| **GC** | Semi-space (Cheney) | Generational + concurrent |

## Quick Start

### CLI

```sh
# Evaluate JavaScript
rune 'var o = {x: 1}; print(o.x + 2);'

# Cold start: 4ms (vs Node 33ms)
time rune '1'

# AFPC cache: first run compiles, subsequent runs load native code
rune --cache=/tmp/foo.cache 'function f(n){var s=0;for(var i=0;i<n;i++)s+=i;return s;} f(100);'
```

### Rust Embedding

```rust
use rune_embed::Context;

let mut ctx = Context::new_small(); // 1 MB heap, ~4ms cold start
let val = ctx.eval("var x = 1; function inc() { return x = x + 1; } inc() + inc()").unwrap();
assert_eq!(val.as_smi(), Some(5)); // 2 + 3 = 5
```

## Architecture

| Crate | Purpose |
|---|---|
| `rune_core` | Tagged Smi/heap values, semi-space GC, immutable shapes, objects, strings |
| `rune_bytecode` | Bytecode opcodes, instructions, program representation, CFG/liveness analysis |
| `rune_parser` | JavaScript lexer, recursive-descent parser, bytecode emitter |
| `rune_interpreter` | Stack-based VM with SIDT inline caches, call frames, generators, builtins |
| `rune_jit_baseline` | Baseline JIT (AArch64 + x86-64) — 57 opcodes whitelisted, function tier-up at 50 calls, N=16 vector IC table, **inlining** (Phase F: hot callees spliced inline, ~5% gain) |
| `rune_embed` | Embedding API (`Context::eval`), AFPC cache save/load |
| `rune_cli` | CLI binary with `--cache`, `--snapshot`, `--ic-stats`, `--trace-stats` |
| `rune_bench` | Criterion benchmarks with V8 comparison scripts |

## What Works

- **Language core:** arithmetic, comparisons, logical operators (loose + strict)
- **Scoping:** var, let, const with block scope and TDZ
- **Functions:** declarations, expressions, arrows, closures, rest/default params, destructuring
- **Objects:** literals, shorthand, methods, computed keys, spread, destructuring
- **Arrays:** dense arrays, spread, destructuring, rest, push/pop/length
- **Control flow:** if/else, while, do/while, for, for-in, switch, try/catch/finally
- **Generators:** function*, yield, next() (basic)
- **Template literals:** substitutions, nested, escapes
- **Error objects:** TypeError, ReferenceError with `.name`/`.message`
- **Prototype chains:** `__proto__`, Object.create, instanceof
- **GC:** Cheney semi-space, sound at 500K+ allocations
- **SIDT:** O(1) property access via SIMD inline caches (NEON + SSE4.1), no megamorphic cliff
- **AFPC cache:** rkyv bytecode persistence (13.5× compile speedup), AArch64 + x86-64 native code caching

## What Doesn't Work (Yet)

- **Standard library:** No Map, Set, Promise, JSON, RegExp, Date, TypedArray, WeakRef
- **String methods:** Only charAt, slice, length
- **Array methods:** Only push, pop, length
- **Modules:** No import/export (ESM)
- **Classes:** No class syntax, super, getters/setters
- **Async/await:** No async, await, for...of
- **JIT:** 57 opcodes whitelisted (out of 93 total opcode variants) — missing: float64 Sub/Mul/Div/Mod promotion (only Add has float64), Div/Mod/Exp not in JIT at all (falls to interpreter via bailout). Phase F inlining shipped (5% on `jit_hot_function_1M`).
- **Debugger:** No CDP/DevTools

## Performance (AArch64, M4 Pro)

### Cold Start

| Benchmark | Rune | Node 22 | Ratio |
|---|---|---|---|
| `rune '1'` / `node -e '1'` | **~4–7 ms** | ~26–33 ms | **~5–8× faster** |

### Hot Loops (2026-06-27, post Phase F inlining + N=16 IC table)

All benchmarks verified via `assert_eq!` for correctness. JIT stats collected per benchmark (see `crates/rune_bench/results/`).

| Benchmark | Rune | Node 22 | Ratio | JIT entries | Bailouts | Notes |
|---|---|---|---|---|---|---|
| `loop_sum_smi_1M` | **124 ms** | 2.30 ms | 54× | 1 | 0 | Trace-compiled Smi-only loop |
| `array_push_grow_100k` | **59 ms** | 7.21 ms | 8× | — | — | No JIT for array push (16 MiB semispace) |
| `jit_hot_function_1M` (no-inline) | **129 ms** | 3.19 ms | 40× | ~1M | 0 | Native JIT-to-JIT call (Phase E) |
| `jit_hot_function_1M` (inline) | **124 ms** | 3.19 ms | **39×** | ~1M | 0 | Phase F inlining: -5% (saves ~6ms dispatch overhead) |
| `poly_prop_10shapes_1M` | **169 ms** | 4.16 ms | 41× | 1 | 0 | N=16 IC table covers all 10 shapes; was 269 ms with N=8 cap |
| `proto_chain_lookup_5deep_1M` | **132 ms** | 1.55 ms | 85× | 1 | 0 | Monomorphic trace, 1 shape, 0 bailouts |

### JIT Stats Summary

| Benchmark | Trace type | IC coverage |
|---|---|---|
| `loop_sum_smi_1M` | 1 trace, 16 ops, 0 shape IDs | N/A (Smi-only) |
| `jit_hot_function_1M` | ~1M JIT entries, 0 bailouts; inlined ~5% faster | N/A (function call) |
| `poly_prop_10shapes_1M` | 1 trace, 22 ops, 10 shape IDs, 0 bailouts | 200K IC lookups, 100% hit rate |
| `proto_chain_lookup_5deep_1M` | 1 trace, 18 ops, 1 shape ID, 0 bailouts | 53 IC lookups, 96% hit rate |

### AFPC Cache

| Operation | Time | vs Baseline |
|---|---|---|
| Compile (parse + emit) | 355 µs | 1× |
| Cache load | 26 µs | **13.5× faster** |

### Phase E: Native JIT Call & N=16 IC Table

**Phase E** removed the interpreter round-trip for JIT-to-JIT function calls:
```
jit_hot_function_1M timeline:
  Baseline (interpreter)  ── 578 ms
  + Call IC                ── 559 ms  (3% improvement)
  + float64 Add promotion  ── 559 ms  (95% bailout rate fixed)
  + Phase E T1 (JIT Call)  ── 124 ms  (4.5× improvement)
  + Phase E T3 (Frame)     ── 130 ms  (lexical-scope correctness, ~5% overhead)
```

**N=16 IC table** resolved the poly_prop bottleneck — the trace-embedded IC table was capped at 8 entries, covering only 8 of 10 shapes at a polymorphic callsite. Bumping to 16 allowed the trace to run without bailouts:
```
poly_prop_10shapes_1M timeline:
  Pre-P22 (GC bug)        ── 258 ms  (first honest measurement)
  Post-P22 (GC roots)     ── 269 ms  (still N=8, 99.9995% bailout)
  + N=16 IC table         ── 169 ms  (-37%, 0 bailouts, trace runs natively)
```

**Phase F inlining** shipped at 5% improvement on `jit_hot_function_1M` (129ms → 124ms). The design doc estimated 25-70ms — the gap comes from overestimating call dispatch overhead (actual ~6ns/call vs estimated ~90ns). The inliner is correct (316 tests, AFPC round-trip verified) and found a pre-existing silent data corruption bug (P26: Sub/Mul/Mod Smi-range overflow). Ships behind `--no-inline` flag (default) for safety.

## Key Innovations

### Shape-Indexed Dispatch Tables (SIDT)

Immutable, content-addressed shapes guarantee O(1) property access at any polymorphism depth. The SIMD inline cache (NEON on AArch64, SSE4.1 on x86-64) compares 2 shapes per cycle with no fallback to a linear walk — there is **no megamorphic cliff**.

| Callsite | Behavior |
|---|---|
| Monomorphic `o.x` | Direct `LoadPropertyIC` after 8 hits |
| 10-shape polymorphic | All shapes in IC, no eviction |
| Loop body | `LoadProperty` → `LoadPropertyIC` patching |

### AOT-First Persistent Compilation (AFPC)

Rune is the only JavaScript engine that caches compiled code across restarts with **permanent validity**. Because shapes are immutable and content-addressed, cached native code never needs invalidation:

1. **First run:** Parse → emit → JIT-compile → persist (bytecode + shapes + ICs + native code)
2. **Subsequent runs:** mmap cache → begin native execution immediately
3. **Delta JIT:** New shapes that were never cached before are compiled on-the-fly

This makes Rune uniquely suited for serverless: functions can be compiled once during cold start and cached globally, delivering near-zero warm latency.

## Roadmap

| Milestone | Focus |
|---|---|
| **v0.0.1** ✅ | Language core + baseline JIT + SIDT IC + AFPC bytecode cache |
| **v0.0.2** ✅ | Expanded JIT opcode coverage (floats, property access, calls), trace compiler |
| **v0.1.0** ✅ | Native JIT Call (Phase E, AArch64), property IC traces, trace-compiled loops |
| **v0.2.0** ✅ | Phase F inlining (5% gain), N=16 IC table, AFPC round-trip with JIT |
| **v0.3.0** 🔜 | Copy-and-patch JIT rewrite (arxiv `2011.13127`), float self-tagging (arxiv `2411.16544`), Nofl GC (arxiv `2503.16971`) |
| **v1.0.0** | Test262 >95%, production hardening, fuzzing |

## Development

```sh
# Run tests
cargo test --workspace

# With JIT enabled
cargo test --features jit

# Format + lint
cargo fmt --all && cargo clippy -- -D warnings

# Criterion benchmarks
cargo bench --features jit

# Enable pre-commit hooks
git config core.hooksPath .githooks
```

## License

MIT OR Apache-2.0
