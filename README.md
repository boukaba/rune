# Rune v0.0.1 — Technology Preview

> ⚠️ **NOT FOR PRODUCTION USE.** Rune can run algorithmic JavaScript today. It cannot run npm packages, Node.js apps, or code that depends on the standard library (Map, Set, Promise, JSON, RegExp, modules, classes, async/await).

A Rust-native JavaScript engine with **Shape-Indexed Dispatch Tables (SIDT)** and immutable shapes. Cold start 4ms — **5× faster than Node.js**. Designed for serverless/edge embedding where predictable latency and small memory footprint matter more than peak throughput.

Rune is the only JS engine with an **AOT-First Persistent Compilation (AFPC)** architecture: compile to native once, cache with rkyv, skip warmup on every restart. Immutable, content-addressed shapes make cached code valid forever — something no other engine can do.

## Architecture

| Crate | Purpose |
|---|---|
| `rune_core` | Tagged Smi/heap values, semi-space GC, immutable shapes, objects, strings |
| `rune_bytecode` | Bytecode opcodes, instructions, program representation, CFG/liveness |
| `rune_parser` | JavaScript lexer, recursive-descent parser, bytecode emitter |
| `rune_interpreter` | Stack-based VM with SIDT inline caches, call frames, generators, builtins |
| `rune_jit_baseline` | Baseline JIT (x86-64 + AArch64 function AOT) + ARM NEON / SSE4.1 SIMD IC |
| `rune_embed` | Embedding API (`Context::eval`), AFPC cache save/load |
| `rune_cli` | CLI binary with `--cache`, `--snapshot`, `--ic-stats`, `--trace-stats` |
| `rune_bench` | Criterion benchmarks with V8 comparison scripts |

## Quick Start

### CLI

```sh
# Evaluate JavaScript
rune 'var o = {x: 1}; print(o.x + 2);'

# Cold start: 4ms (vs Node 33ms — 5× faster)
time rune '1'

# AFPC cache: first run compiles & saves, subsequent runs load native code
rune --cache=/tmp/foo.cache 'function f(n){var s=0;for(var i=0;i<n;i++)s+=i;return s;} f(100);'
```

### Rust Embedding

```rust
use rune_embed::Context;

let mut ctx = Context::new_small(); // 1MB heap, ~4ms cold start
let val = ctx.eval("var x = 1; function inc() { return x = x + 1; } inc() + inc()").unwrap();
assert_eq!(val.as_smi(), Some(5)); // 2 + 3 = 5
```

## What Works

- **Language core:** arithmetic, comparisons, logical operators (loose + strict)
- **Scoping:** var, let, const with block scope and TDZ
- **Functions:** declarations, expressions, arrows, closures, rest/default params
- **Objects:** literals, shorthand, methods, computed keys, spread, destructuring
- **Arrays:** dense arrays, spread, destructuring, rest, push/pop/length
- **Control flow:** if/else, while, do/while, for, for-in, switch, try/catch/finally
- **Destructuring:** object, array, nested, defaults, rest patterns
- **Generators:** function*, yield, next() (basic)
- **Template literals:** substitutions, nested, escapes
- **Error objects:** TypeError, ReferenceError with .name/.message
- **Prototype chains:** \_\_proto\_\_, Object.create, instanceof
- **GC:** Cheney semi-space, sound at 500K+ allocations
- **Closures:** heap-allocated environment chain, full capture + mutation
- **SIDT:** O(1) property access via SIMD inline caches (NEON + SSE4.1), no megamorphic cliff
- **AFPC cache:** rkyv bytecode persistence (13.5× compile speedup), AArch64 + x86-64 native code caching

## What Doesn't Work (Yet)

- **Standard library:** No Map, Set, Promise, JSON, RegExp, Date, TypedArray, WeakRef
- **String methods:** Only charAt, slice, length
- **Array methods:** Only push, pop, length
- **Modules:** No import/export (ESM)
- **Classes:** No class syntax, super, getters/setters
- **Async/await:** No async, await, for...of
- **Baseline JIT (x86-64):** 56/62 opcodes — Smi arithmetic, comparison, bitwise, unary, branches, locals, property access, TypeOf, LoadStringConst, LoadGlobal, StoreGlobal, IncGlobal, DecGlobal. Function tier-up at 50 calls. Input guards + overflow guards + bailout to interpreter.
- **Trace compiler (aarch64):** Loop trace recording + native compilation for hot loops with property access. Limited by global variable rejection (pre-existing IC stack bug).
- **Debugger:** No CDP/DevTools

## Performance (aarch64, M4 Pro)

| Benchmark | Rune | V8 (Node v22) | Ratio |
|---|---|---|---|
| **Cold start** (`rune '1'` / `node -e '1'`) | **4ms** | 33ms | **Rune 5× faster** |
| `loop_sum_smi_1M` | **5.3ms** | 2.2ms | **2.4× slower** |
| `jit_hot_function_1M` | 683ms | 2.4ms | 285× slower |
| `array_push_grow_100k` | 68ms | 6.7ms | 10× slower |
| `poly_prop_10shapes_1M` (SIDT) | 1.05s | 4.1ms | 256× slower |
| `proto_chain_lookup_5deep_1M` | — | 1.5ms | — |

> **Trace compiler (aarch64):** Hot loops with Smi arithmetic and global variables are compiled to native code, reaching **75× speedup** vs the interpreter (`loop_sum_smi_1M`: 397ms → 5.3ms). Traces with property access (`LoadPropertyIC`/`StorePropertyIC`) are not yet compiled — the bailout infrastructure needed for shape-guard miss handling is not set up for traces (the function JIT uses a `BailoutTable` per compiled function). This is the next investment target.
>
> **Function JIT (x86-64):** 56/62 opcodes covered. On aarch64, the function JIT exists via `Aarch64CodeGen` but the `jit_hot_function_1M` benchmark is dominated by Smi-overflow bailout (95% of calls bail after iteration ~46K when the sum exceeds i31 range).

**AFPC cache:** Compile (parse+emit) 355µs → cache load 26µs (**13.5× faster**). End-to-end latency is execution-bound — cache eliminates parse/emit entirely but hot loops still run in interpreter.

### SIDT Architecture

Rune's Shape-Indexed Dispatch Tables guarantee O(1) property access regardless of shape count — no megamorphic cliff.

| Callsite | IC Stats |
|---|---|
| Monomorphic `o.x` | IC lookup bypassed after LoadPropertyIC patching |
| 10-shape polymorphic | Unlimited entries, NEON SIMD (2 shape_ids/cycle) |
| Loop body patching | All LoadProperty → LoadPropertyIC after 8 hits |

### AFPC: AOT-First Persistent Compilation

Immutable, content-addressed shapes make cached native code valid forever — no engine in production or research does this. On first run, Rune compiles to native and persists bytecode + shapes + ICs + native code to disk. On subsequent runs, it mmap's the cache and begins native execution from the first instruction. Delta JIT handles new shapes never seen before.

**Status:** Bytecode + shape + IC persistence shipped (Sprint 16). AArch64 + x86-64 function baseline JIT compiles Smi-only opcode subset. Trace compiler and full opcode coverage are in progress.

## Development

```sh
# Run tests (434 total across workspace)
cargo test --workspace

# Format + lint
cargo fmt --all && cargo clippy -- -D warnings

# Enable pre-commit checks
git config core.hooksPath .githooks
```

## Roadmap

| Release | Focus |
|---|---|
| **v0.0.1** | Language core + baseline JIT + SIDT IC + AFPC bytecode cache |
| **v0.0.2** | Expanding JIT opcode coverage (floats, property access, calls), wire trace compiler |
| **v0.1.0** | Classes, async/await, standard library (Map/Set/Promise/JSON/RegExp), ESM modules |
| **v0.2.0** | Full AFPC: all-opcode JIT, delta JIT for shape deltas, GenImmix GC |
| **v1.0.0** | Fuzzed, production-ready, Test262 >95% |

## License

MIT OR Apache-2.0
