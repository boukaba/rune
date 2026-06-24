# Rune v0.0.1 — Technology Preview

> ⚠️ **NOT FOR PRODUCTION USE.** Rune can run algorithmic JavaScript today. It cannot run npm packages, Node.js apps, or code that depends on the standard library (Map, Set, Promise, JSON, RegExp, modules, classes, async/await).

A Rust-native JavaScript engine with **Shape-Indexed Dispatch Tables (SIDT)** and immutable shapes. Cold start 5× faster than Node.js. Designed for serverless/edge embedding where predictable latency and small memory footprint matter more than peak throughput.

## Architecture

Rune is the only Rust-native JS engine with a property-access specialization architecture. Every other Rust option wraps a C/C++ engine through FFI.

| Crate | Purpose |
|---|---|
| `rune_core` | Value types (tagged Smi/heap), semi-space GC, immutable shapes, objects, strings |
| `rune_bytecode` | Bytecode opcodes, instructions, program representation, CFG/liveness analysis |
| `rune_parser` | JavaScript lexer, recursive-descent parser, bytecode emitter |
| `rune_interpreter` | Stack-based VM with inline caches (SIDT), call frames, generators, builtins, try/catch/finally |
| `rune_jit_baseline` | Baseline JIT (x86-64, Smi-only) + AArch64 trace compiler foundation + ARM NEON / SSE4.1 SIMD IC |
| `rune_embed` | High-level embedding API (`Context::eval`) for Rust applications |
| `rune_cli` | CLI binary (`rune`) with `--snapshot`, `--ic-stats`, `--trace-stats` flags |
| `rune_bench` | Criterion benchmarks with V8 comparison scripts |

## Quick Start

### CLI

```sh
# Evaluate JavaScript
rune 'var o = {x: 1}; print(o.x + 2);'

# Cold start: 7ms (vs Node 33ms — 5× faster)
time rune '1'

# With snapshot cache: first run 340ms, subsequent 50ms (6.8× faster)
rune --snapshot 'var s=0;for(var i=0;i<100000;i++)s+=i;s'
```

### Rust Embedding

```rust
use rune_embed::Context;

let mut ctx = Context::new_small(); // 1MB heap, ~7ms cold start
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
- **Error objects:** TypeError, ReferenceError with proper .name/.message
- **Prototype chains:** __proto__, Object.create, instanceof
- **GC:** Cheney semi-space, sound at 500K+ allocations
- **Closures:** heap-allocated environment chain, full capture + mutation

## What Doesn't Work (Yet)

- **Standard library:** No Map, Set, Promise, JSON, RegExp, Date, TypedArray, WeakRef
- **String methods:** Only charAt, slice, length, fromCharCode
- **Array methods:** Only push, pop, length, isArray
- **Modules:** No import/export (ESM)
- **Classes:** No class syntax, super, getters/setters
- **Async/await:** No async, await, for...of
- **Optimizing JIT:** Baseline only — 5–230× slower than V8 on hot loops
- **Debugger:** No CDP/DevTools

## Performance

| Benchmark | Rune | V8 (Node.js v22) | Ratio |
|---|---|---|---|
| **Cold start** (`rune '1'` / `node -e '1'`) | **7ms** | 33ms | **Rune 5× faster** |
| `array_push_grow_100k` | 70ms | 3ms | 26× slower |
| `o.x` 1M (monomorphic) | 480ms | 4ms | 120× slower |
| `o.x` 10 shapes (SIDT) | 590ms | 5ms | 116× slower |
| `loop_sum_smi_1M` | 440ms | 3ms | 147× slower |

**Snapshot cache (AFPC):** First run 340ms → cached 50ms (6.8× faster).

Hardware: MacBook Pro M4 Pro (aarch64). Rune: interpreter with SIDT + SIMD IC (NEON).

### SIDT Architecture

Rune's Shape-Indexed Dispatch Tables guarantee O(1) property access regardless of shape count — no megamorphic cliff.

| Callsite | IC Stats |
|---|---|
| Monomorphic `o.x` | 9 IC lookups / 1M accesses (LoadPropertyIC shape guard) |
| 10-shape polymorphic | Unlimited entries, NEON SIMD (2 shape_ids/cycle) |
| Loop body patching | All LoadProperty → LoadPropertyIC after 8 hits |

### AFPC: AOT-First Persistent Compilation (planned)

Rune's immutable shapes enable a genuinely novel execution model: compile to native once, cache forever, skip warmup on every restart. No engine in production or research does this. `--snapshot` demonstrates the first step: source caching reduces parse+emit overhead by 6.8×. Full native code caching (Phase 5) targets cold start <2ms and hot loops within 5–15× of V8.

## Development

```sh
# Run tests (297 integration tests)
cargo test --workspace

# Format + lint
cargo fmt --all && cargo clippy -- -D warnings

# Enable pre-commit checks
git config core.hooksPath .githooks
```

## Roadmap

| Release | Focus |
|---|---|
| **v0.0.1** | Language core + baseline JIT + SIDT IC |
| **v0.0.2** | Classes, async/await, iterators |
| **v0.1.0** | Standard library (Map/Set/Promise/JSON/RegExp), ESM modules |
| **v0.2.0** | AFPC: AOT-first native compilation, delta JIT, rkyv persistence |
| **v1.0.0** | Fuzzed, production-ready, Test262 >95% |

## License

MIT OR Apache-2.0
