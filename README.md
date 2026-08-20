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
| `rune_jit_baseline` | Baseline JIT (AArch64 + x86-64) — 57 opcodes whitelisted, function tier-up at 50 calls, N=16 vector IC table, **inlining** (Phase F: hot callees spliced inline, ~5% gain), **float self-tagging** (NaN-boxed Values, 0 heap allocation for floats) |
| `rune_embed` | Embedding API (`Context::eval`), AFPC cache save/load |
| `rune_cli` | CLI binary with `--cache`, `--snapshot`, `--ic-stats`, `--trace-stats` |
| `rune_bench` | Criterion benchmarks with V8 comparison scripts |

## What Works

- **Iteration:** `for..of` over arrays, strings (UTF-16 code points, surrogate pairs), user iterables (JS `@@iterator` factories + JS `next`), array iterators from `.values()`/`.keys()`/`.entries()`; `Symbol.iterator` lookup on arrays/strings; spread via iteration (`[...x]`, `f(...x)`); iterator objects are iterable (`it[Symbol.iterator]() === it`)
- **Symbols:** `Symbol()` constructor (unique symbols, `new Symbol()` throws), descriptions + `toString()`/"Symbol(desc)", `Symbol.for`/`Symbol.keyFor`, 13 well-known symbols (`Symbol.iterator`, `Symbol.match`, `Symbol.replace`, `Symbol.search`, `Symbol.split`, `Symbol.toPrimitive`, `Symbol.hasInstance`, `Symbol.toStringTag`, `Symbol.species`, `Symbol.isConcatSpreadable`, `Symbol.unscopables`, `Symbol.matchAll`, `Symbol.asyncIterator`), symbol-keyed properties (`o[sym] = v` — excluded from for-in/Object.keys/JSON), `typeof sym === "symbol"`, **@@match/@@search/@@split/@@replace dispatch** in String.prototype.match/search/split/replace (GetMethod + callable check, TypeError on non-callable), `String(sym)`/`"a"+sym` throw TypeError
- **Language core:** arithmetic (incl. `**` right-assoc), comparisons, logical operators (loose + strict), `??` nullish coalescing, `&&=`/`||=`/`??=` short-circuit assignment
- **Scoping:** var, let, const with block scope and TDZ
- **Functions:** declarations, expressions, arrows, closures, rest/default params, destructuring
- **Objects:** literals, shorthand, methods, computed keys, spread, destructuring, member `obj.prop++` update
- **Optional chaining:** `a?.b`, `a?.[b]`, `a?.b()`, `a?.()`, `?.#priv`, mixed chains (`a?.b.c?.[d]`) — whole chain short-circuits to undefined; syntax errors for `a?.b = c`, `new a?.b`
- **Arrays:** dense arrays, spread, destructuring (declarations + assignment), rest, push/pop/length
- **Control flow:** if/else, while, do/while (break/continue), for (continue runs the update), for-in, for..of (break/continue, member LHS), switch, try/catch/finally
- **Generators:** function*, yield, next() (basic)
- **Async/await:** `async function`, `async () =>`, `await expr` — generator-based, synchronous until first await, Promise-based continuation
- **Promise:** constructor, resolve/reject, `.then`/`.catch`/`.finally`, `Promise.resolve`/`.reject`/`.all`/`.race`, microtask queue, **thenable unwrapping**
- **Classes:** declarations, expressions (named & anonymous), default constructor, prototype methods, `extends` (heritage), `super()` calls, `super.prop` access, `super.prop = val` assignment, compound assignment `super.prop += val`, `static` methods, getter/setter syntax (`get`/`set`), **computed property names**, **private fields (`#`)**, **private methods + accessors (`#m() {}`, `get #x() {}`)**, **static private fields/methods**, **object-literal accessors (`{ get a() {} }`)**, **`this.prop++`** in methods
- **ESM modules:** `import`/`export` — default/named/namespace imports (`import x from`, `import { a, b as c }`, `import * as ns`), default/named exports, re-exports (`export { x } from`, `export * from`, `export * as ns from`), live bindings (exported `let`/`var`/`const` stay linked — import-time reads see current values), hoisted function/var exports available during circular evaluation, TDZ on imported `let`/`const` (ReferenceError before initialization), single evaluation per specifier (dependency graph resolved once), `Context::eval_module(entry, resolver)` + `Context::module_export(spec, name)`, CLI runs `.mjs` files (and falls back from script-parse errors to module eval)
- **Error objects:** TypeError, ReferenceError with `.name`/`.message`
- **Prototype chains:** `__proto__`, Object.create, instanceof
- **GC:** Cheney semi-space, sound at 500K+ allocations
- **SIDT:** O(1) property access via SIMD inline caches (NEON + SSE4.1), no megamorphic cliff
- **AFPC cache:** rkyv bytecode persistence (13.5× compile speedup), AArch64 + x86-64 native code caching
- **JSON:** `JSON.parse` + `JSON.stringify` (complete round-trip, cycle detection, NaN/Infinity → `null`)
- **RegExp:** Thompson NFA + PikeVM engine, `/pattern/flags` literals, `RegExp()` constructor (new + plain call, RegExp/string patterns, flag validation with SyntaxError), `exec`/`test` with RegExpBuiltinExec semantics (global/sticky `lastIndex` advance, sticky failure reset), match results with `.index`/`.input`, `.source`/`.flags`/`.lastIndex` properties, lookahead `(?=…)`/`(?!…)`, `{n,m}` quantifiers, replace with `$&`/``$` ``/`$'`/`$1..$n` expansion and function replacement (incl. `replaceAll` function replacement)
- **Array methods:** `filter`, `map`, `reduce`, `forEach`, `slice` (callback state machine, GC-safe across 200K elements), `find`, `some`, `every`, `sort`, `flat`, `flatMap`, `includes`, `push`, `pop`, `indexOf`
- **Map / Set:** `new Map(iterable)`/`new Set(iterable)` (AddEntriesFromIterable, SameValueZero), `get`/`set`/`has`/`delete`/`clear`/`forEach`/`entries`/`keys`/`values`/`size`/`add`, user iterables (JS `@@iterator` + JS `next` state machines), iteration with deletion skipping, `instanceof`
- **Date:** `new Date()` (no args/string/Date/number/multi-arg forms), `Date.now`/`parse`/`UTC`, all getters + setters, `toString`/`toDateString`/`toTimeString`/`toUTCString`/`toISOString`/`toJSON`/`toLocale*`/`valueOf`, `getTimezoneOffset`, full ISO parsing + legacy `toString`-format parsing, "Invalid Date", UTC-only timezone (spec-conformant §21.4.1.6), `Date()` plain call returns a string
- **TypedArray family:** `ArrayBuffer` (+ `slice`, `isView`), Uint8Array/Int8Array/Uint8ClampedArray/Int16Array/Uint16Array/Int32Array/Uint32Array/Float32Array/Float64Array — ctor from length/typed array/ArrayBuffer view/array/string, typed indexing with spec conversions (wraparound, truncation, round-half-to-even clamping), `length`/`byteLength`/`byteOffset`/`buffer`/`BYTES_PER_ELEMENT`, `set` (overlapping-safe snapshot), `subarray` (shared buffer), `fill`/`at`/`indexOf`/`includes`/`slice`, iteration + spread via `@@iterator`
- **String methods:** `replace`/`replaceAll` (string + regex), `indexOf`, `charAt`, `slice`, `split` (string + regex separator, limit), `trim`/`trimStart`/`trimEnd`, `toUpperCase`/`toLowerCase`, `charCodeAt`/`codePointAt` (UTF-16 code-unit semantics incl. surrogate pairs), `fromCharCode`, `startsWith`/`endsWith`/`includes`, `padStart`/`padEnd`, `repeat`, `substring`/`substr`/`concat` — all position/index math in UTF-16 code units
- **Global functions:** `parseInt` (radix, hex), `parseFloat` (Infinity, NaN, scientific notation)

## What Doesn't Work (Yet)

- **Standard library:** No WeakRef. Typed arrays: ctor accepts arrays/strings/typed arrays/ArrayBuffers but not general iterables; no BigInt64Array/BigUint64Array/Float16Array; non-numeric own props on typed arrays unsupported; no `Object.prototype.toString` builtin (so `@@toStringTag` is unobservable). Date is UTC-only (no local timezone — spec-conformant, `getTimezoneOffset` always 0) and `toLocaleString` family = `toString` family (no ECMA-402); no `setYear`/`getYear`/`toGMTString` (Annex B); `Date.prototype.toJSON` handles Date receivers only. Map/Set forEach is O(n²) worst case (linear live-presence check per snapshot key). RegExp: `v` flag accepted by the ctor but the literal parser's flag whitelist predates it (`/a/v` literal unsupported); match-result `index`/`input` enumerability is unobservable (no shape enum flags — `Object.keys` includes them); backrefs are no-ops; anchors `^`/`$` are identity; empty-match at end-of-string not found; `{n,m}` captures return the first copy's capture; engine unit is Rust char (no `u`-flag UTF-16 index math). Iteration: no IteratorClose on break/return, no `let`-per-iteration freshness in for..of, `.values()/.keys()/.entries()` need array receivers (no array-likes), destructuring LHS in for..of unsupported. Symbols: `Number(sym)` returns NaN instead of throwing (ToNumber plumbing deferred).
- **String methods:** no `String.fromCodePoint`; `charAt` returns whole code points (V8 returns lone-surrogate halves); lone surrogates decode to U+FFFD (HeapString model)
- **Array methods:** `filter`, `map`, `reduce`, `forEach`, `slice`, `find`, `some`, `every`, `sort`, `flat`, `flatMap`, `includes`, `push`, `pop`, `indexOf`.
- **Modules:** Full ESM (`import`/`export` incl. default/named/namespace/star, live bindings, circular deps, TDZ, `.mjs` CLI). Gaps: no node_modules-style resolution (resolver callback), `import()` dynamic / `import.meta` unsupported, module functions don't JIT (bail to interpreter — LoadGlobal reads the module env)
- **Classes:** Class names don't resolve inside static methods (pre-existing). Static private lookup walks the superclass chain. `o.#x` outside the class body throws at runtime, not parse time.
- **Async/await:** `async function`, `async () =>`, `await expr` — full support with generator-based desugaring, synchronous until first await. 396 tests.
- **JIT:** 57 opcodes whitelisted (out of 93 total opcode variants). Float Self-Tagging (NaN-boxing) eliminates all float heap allocation — all interpreter float paths use inline `Value::from_float64`. JIT has float64 Add promotion; Sub/Div/Mod/Exp bail to interpreter (which handles them via NaN-boxed Values). **Guarded Smi Mul/Mod run natively** with a compile-time `stack_depth` model + `push_raw` bail paths; every guard (shape-miss, Smi-check, overflow, call) bails to the interpreter with a validated pre-opcode snapshot. Phase F inlining shipped (5% on `jit_hot_function_1M`).
- **Debugger:** No CDP/DevTools

## Performance (AArch64, M4 Pro)

### Cold Start

| Benchmark | Rune | Node 22 | Ratio |
|---|---|---|---|
| `rune '1'` / `node -e '1'` | **~4–7 ms** | ~26–33 ms | **~5–8× faster** |

### Hot Loops (2026-06-28, v0.3+ — 392 tests)

All benchmarks verified via `assert_eq!` for correctness. NaN-boxing eliminates all `HeapFloat64` allocation — float operations are register ops. 396 tests pass. JIT stats collected per benchmark (see `crates/rune_bench/results/`).

| Benchmark | Rune | Node 22 | Ratio | JIT entries | Bailouts | Notes |
|---|---|---|---|---|---|---|
| `loop_sum_smi_1M` | **124 ms** | 2.30 ms | 54× | 1 | 0 | Trace-compiled Smi-only loop |
| `array_push_grow_100k` | **59 ms** | 7.21 ms | 8× | — | — | No JIT for array push (16 MiB semispace) |
| `jit_hot_function_1M` (no-inline) | **135 ms** | 3.19 ms | 42× | ~1M | 0 | Native JIT-to-JIT call (Phase E); NaN-boxed floats |
| `jit_hot_function_1M` (inline) | **135 ms** | 3.19 ms | **42×** | ~1M | 0 | Phase F inlining: ~5% gain, NaN-boxed floats |
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

**Float Self-Tagging (NaN-boxing)** eliminated all `HeapFloat64` allocation in v0.3. Every interpreter float path (LoadFloat64, Math constants, Neg, comparisons) now uses `Value::from_float64` — inline NaN-encoded Values with zero GC allocation. The JIT's JumpIfFalse/JumpIfTrue handlers were fixed to check NaN-encoded values directly (removed stale float64 bailout branch). 392/392 tests pass.

**Phase F inlining** shipped at 5% improvement on `jit_hot_function_1M` (129ms → 124ms). The design doc estimated 25-70ms — the gap comes from overestimating call dispatch overhead (actual ~6ns/call vs estimated ~90ns). The inliner is correct (316 tests, AFPC round-trip verified) and found a pre-existing silent data corruption bug (P26: Sub/Mul/Mod Smi-range overflow). Ships behind `--no-inline` flag (default) for safety.

**Standard library (stdlib)** delivered JSON round-trip (`JSON.parse`/`JSON.stringify` with cycle detection, NaN/Infinity → `null`), array callback methods (`filter`/`map`/`reduce`/`forEach` via callback state machine), `Array.prototype.slice`, `String.prototype.split` (string separator), and `parseInt`/`parseFloat` globals. Boolean string coercion in the `Add` opcode fixed (`true + ""` → `"true"`, not `"undefined"`). 392 integration tests pass.

**Sprint 18** extended the callback state machine to support non-TAG_ARRAY objects via `array_like_length`/`array_like_index` helpers — array builtins now work on arguments objects and other array-like receivers. `Function.prototype.call` implemented using the same pending-callback pattern. Builtin exceptions now route through the pending-exception mechanism, making all builtin errors catchable by JS `try/catch`. Test262 harness tracks assert calls and reports spec-conformant human-readable errors. String comparison in `StrictEq` fixed to compare by content, not heap pointer. 392/392 tests pass.

## Key Innovations

### Shape-Indexed Dispatch Tables (SIDT)

Immutable, content-addressed shapes guarantee O(1) property access at any polymorphism depth. The SIMD inline cache (NEON on AArch64, SSE4.1 on x86-64) compares 2 shapes per cycle with no fallback to a linear walk — there is **no megamorphic cliff**.

| Callsite | Behavior |
|---|---|
| Monomorphic `o.x` | Direct `LoadPropertyIC` after 8 hits |
| 10-shape polymorphic | All shapes in IC, no eviction |
| Loop body | `LoadProperty` → `LoadPropertyIC` patching |

### AOT-First Persistent Compilation (AFPC)

Rune caches compiled code across restarts with **permanent validity by construction**: shapes are immutable and content-addressed, so cached native code never needs an invalidation pass (unlike V8/JSC, whose shape transitions invalidate cached dispatch). Cache format is versioned (`AFPC_VERSION = 2`); any format change requires a bump that invalidates old caches.

1. **First run:** Parse → emit → JIT-compile → persist (bytecode + shapes + ICs + native code)
2. **Subsequent runs:** mmap cache → restore shapes/ICs, install native entries, execute immediately (cache load 26µs vs 355µs compile — 13.5×)
3. **Delta JIT:** uncached code encountered at runtime is compiled on-the-fly by the interpreter's tier-up paths (function JIT + trace compiler). New-shape cache **append** is not yet persisted, and compiled traces are not yet part of the cache.

This makes Rune uniquely suited for serverless: functions can be compiled once during cold start and cached globally, delivering near-zero warm latency. Native execution currently runs on AArch64; x86-64 JIT codegen is disabled pending a correctness fix.

## Roadmap

| Milestone | Focus |
|---|---|
| **v0.0.1** ✅ | Language core + baseline JIT + SIDT IC + AFPC bytecode cache |
| **v0.0.2** ✅ | Expanded JIT opcode coverage (floats, property access, calls), trace compiler |
| **v0.1.0** ✅ | Native JIT Call (Phase E, AArch64), property IC traces, trace-compiled loops |
| **v0.2.0** ✅ | Phase F inlining (5% gain), N=16 IC table, AFPC round-trip with JIT |
| **v0.3.0** ✅ | Float self-tagging (NaN-boxing), stdlib (JSON round-trip, array methods, string split, parseInt/parseFloat), boolean coercion fix — 387 tests |
| **v0.4.0** ✅ | 14 builtins: Object.keys/values/entries, Array find/some/every/sort/flat/flatMap/includes/indexOf, String replace/replaceAll, Number(), Function.prototype.call. 393 tests. |
| **v0.5.0** 🚧 | **Promise**: constructor, `.then`/`.catch`/`.finally`, 3-level chaining, `resolve`/`reject`/`all`/`race`, **microtask queue** with reaction storage. **Async/await**: generator-based desugaring, `async function`/`async () =>`/`await`, synchronous until first await. Parser reserved-word fix. Array/String indexOf. **RegExp**: engine (parse→NFA→PikeVM), capture groups, `$1..$n` expansion, `RegExp.prototype.exec`/`.test`, prototype chain. **Class**: declarations, expressions, `extends`, `super()`, static methods, getter/setter, private fields (`#`). **String.prototype.match/search/split** for RegExp (no @@match/@@search/@@split yet, no RegExp constructor). **JIT lexical state**: `let`-loop traces run natively via lexical helper + bailout with preserved lexical envs (Mul-overflow bailout round-trip verified, Σ i² = 114,330,883,345,000). test262 Promise 46%. 482/482 integration tests (3 ignored). |
| **v0.8.0** 🚧 | **ESM modules**: `Context::eval_module(entry, resolver)` compiles the full import graph (single evaluation per specifier, section-ordered: imports → export decls → statements), module programs pin `Pin<Box<BytecodeProgram>>` in the Context (no GC-header copy). Import/export forms: default, named (incl. renames), namespace (`import * as ns`), re-exports (`export {x} from`, `export * from`, `export * as ns from`), `export default` (any expr / decl). **Live bindings**: exports link to module env slots; the importer reads through the module record, so post-import mutation of exported `let`/`var`/`const` is visible. **TDZ**: `ModuleTdz` opcode seeds hoisted `let`/`const`/function bindings with a sentinel; reads before section-3 init throw catchable "ReferenceError: Cannot access '<name>' before initialization" (spec §16.2.1.7, not silent `undefined`). **Circular deps**: dependency-only re-export imports evaluate upstream modules first; hoisted var/function exports are readable mid-cycle, hoisted let/const throw until initialized. **Module functions**: `Func` packs `module_mi` into the existing flags word (GC hardcodes 80-byte Func); LoadGlobal/StoreGlobal inside module functions resolve against their OWN module env (not the entry's — root-caused an infinite recursion); StoreGlobal on an imported binding throws catchable "TypeError: Assignment to constant variable."; module programs and module-created functions skip JIT (LoadGlobal reads the module env, not shared globals — fixes a bailout stack-depth mismatch panic). **CLI**: `.mjs` files route through `eval_module` with an `fs_resolve` filesystem resolver; script parse errors mentioning Import/Export fall back to module eval. 18 new integration tests — 613/613 integration tests (3 ignored), workspace 774. |
| **v0.7.0** 🚧 | **Classes completion**: static private fields (`static #x = 10` — initializer runs as a zero-arg function with `this` = the constructor), private methods + accessors for instance AND static (instance ones compiled into the ctor program), accessor-pair slot merging (getter+setter share one private name — fixes phantom-key writes), private reads/writes throw TypeError on missing keys, private accessor dispatch in the VM, duplicate-private-name early error. **Object-literal accessors** `{ get a() {} }` / `{ set a(v) {} }` (incl. computed keys, nested accessors). `this.prop++` in methods. `let`+`new` class scoping verified. 595/595 integration tests (3 ignored). |
| **v0.6.0** 🚧 | **Symbols**: `Symbol()` ctor, `Symbol.for`/`keyFor`, 13 well-known symbols, symbol-keyed props (excluded from for-in/keys/JSON), `typeof`, **@@match/@@search/@@split/@@replace GetMethod dispatch** in String methods, coercion TypeErrors. Ternary precedence fix. **Iteration protocol + `for..of`**: `Array.prototype.values/keys/entries` + `@@iterator`, `String.prototype[Symbol.iterator]`, user iterables (JS `@@iterator` factories + JS `next` via pending state machines), spread via iteration at all 5 sites (`[...x]`, `f(...x)`), break/continue, member LHS (`for (o.p of …)`), error paths. Fixed `return`-ASI peek-ahead bug and `while` break-target bug (both pre-existing silent miscompiles). **Map / Set**: ctor from iterables (AddEntriesFromIterable, SameValueZero), get/set/has/delete/clear/forEach/entries/keys/values/size/add, user iterables (JS `@@iterator` + `next` state machines), deletion-skipping iteration, `instanceof`. **Date**: `RuneDate` GC type, ctor (all arg forms), `now`/`parse`/`UTC`, 16 getters + 15 setters, toString family, `toISOString`/`toJSON`, ISO + legacy parsing, UTC-only timezone (spec-conformant), `Date()` plain call → string. **RegExp completion**: `RegExp()` constructor (flag validation, plain-call identity), exec RegExpBuiltinExec semantics (global/sticky lastIndex, sticky failure reset), match results with `.index`/`.input` (RuneArray `extra_props`), `replaceAll` function replacement, lookahead + `{n,m}` quantifiers, `instanceof RegExp`. 582/582 integration tests (3 ignored). |
| **Sprint 18** ✅ | Non-TAG_ARRAY refactor, Function.prototype.call, P27 test262 harness (assert tracking + human-readable errors), P29 builtin throws catchable by try/catch, string same-value fix, boolean display fix, string_slice float edge cases, reduce mutation fix — 392 tests |
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
