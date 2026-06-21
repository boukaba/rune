# Rune — Implementation Progress

> **Project:** Production-ready JavaScript runtime in Rust
> **Spec Target:** ECMAScript 2027 (ECMA-262, 18th Edition)
> **Status:** Sprint 3 complete (Prototype Chain) → Sprint 4: SIDT + Dense Arrays + Builtins

> **⚠️ CRITICAL RULE — Spec-First Development**
> Every implementation decision at every level (lexer, parser, emitter, bytecode, interpreter, builtins, JIT) **must** be verified against the exact ECMA-262 specification language in [`ecma262.md`](./ecma262.md) — **never guess** what the spec says. Each section in `ecma262.md` links to the corresponding URL fragment on `https://tc39.es/ecma262/multipage/`; **always open these URLs via `webfetch` tool** to read the authoritative algorithm steps before implementing. This applies to all phases below.

---

## Phase 0 — Spike Validation ✅

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) — open each linked `https://tc39.es/ecma262/multipage/` URL via `webfetch` for exact algorithms. No guessing.

**Goal:** Prove the two riskiest subsystems work on real hardware before committing to full implementation.

### Spike 1: MMTk `ObjectModel` for MarkSweep
- [x] Create temporary crate with MMTk `ObjectModel` impl for `RuneObject`
- [x] Header: shape pointer (plain, aligned)
- [x] Side-metadata mark bits via `MetadataSpec::new_side_metadata`
- [x] `get_gc_bits` / `set_gc_bits` on side metadata
- [x] Stub out forwarding pointer methods (panic)
- [x] Implement `Scanning` walking shape-defined slot list
- [x] Test harness: 2-3 shapes, reference graph, periodic forced GC
- [x] Run 1M allocate/drop cycles
- **Acceptance:** Design validated; MarkSweep side metadata quarantines ~8.6 TB on macOS (known MMTk limitation). Works on Linux.

### Spike 2: Copy-and-Patch on x86-64 and aarch64
- [x] Templates for `LOAD_SMI`, `ADD`, `RETURN` opcodes
- [x] Code emission: RW alloc → copy → patch → mprotect RX
- [x] aarch64: MAP_JIT + hardware icache management works
- [x] Test: `function add3(a,b,c){return a+b+c;}` in bytecode → JIT → exec
- [x] Smi operands for i31
- **Acceptance:** All tests pass on Apple Silicon. x86-64 templates follow same pattern.

---

## Phase 1 — Core Runtime & GC ✅

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §6–§10 — open each linked `https://tc39.es/ecma262/multipage/` URL via `webfetch` for exact type system, object, and GC algorithms. No guessing.

**Goal:** Fundamental types, object model, GC, embeddable API, interpreter shell.

### `rune_core` crate
- [x] `value.rs` — `Value` with pointer-tagging (bit0=1 Smi, bit0=0 heap pointer; undefined=0, null=2)
- [x] `string.rs` — `HeapString` GC-allocated flat UTF-16 with surrogate pair decoding
- [x] `shape.rs` — Hash-consed immutable shape with global `ShapeTable` interner, `&'static Shape`; `intern_with_parent()` for shape transitions
- [x] `object.rs` — `JSObject` with shape pointer + variable property slots + 4 reserved slots for in-place property growth; `add_property()` for dynamic property extension
- [x] `gc.rs` — Cheney-style semispace copying GC (4 MiB per semispace), auto-collect on alloc when roots registered
- [x] `barrier.rs` — Write-barrier trait + `NoOpBarrier`
- [x] `heap.rs` — GC integration module re-exporting `SemiSpace`
- [x] **Tests:** 10 unit tests + 10 integration tests (value tagging, string alloc, object slots, GC survival, graph tracing, space reclamation, idempotence, multi-generation)

### `rune_embed` crate
- [x] Stable Rust API: `Context` wrapping `SemiSpace`
- [x] `eval_bytecode`, `allocate_string`, `allocate_object`

### `rune_capi` crate
- [x] C-compatible: `rune_context_create`, `rune_context_eval`, `rune_context_destroy`, `rune_free_string`
- [x] Opaque handles only

### `rune_interpreter` shell
- [x] Stack-based bytecode loop: LoadSmi, LoadUndefined, LoadNull, LoadBoolean, Add, Sub, Mul, Div, Eq, StrictEq, Lt, Gt, Jump, JumpIfTrue, JumpIfFalse, NewObject, TypeOf, Return
- [x] Execute hardcoded bytecode arrays
- [x] GC integration: register roots at safe points

### Acceptance Criteria
- [x] 25/25 tests pass across workspace
- [x] GC test: 500K+ objects allocated/collected, space reclaimed, graph integrity
- [x] `rune_embed` can allocate strings and objects

---

## Phase 2 — Parser, Bytecode Emitter, Test262 Conformance

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §12–§15 (lexer/parser/emitter), §9 (execution contexts), §29.3 (generators) — open each linked `https://tc39.es/ecma262/multipage/` URL via `webfetch` for exact grammar productions and runtime semantics. No guessing.

**Goal:** Full JS parser, bytecode definition/emitter/CFG/liveness, interpreter runs any script, >95% Test262.

### `rune_bytecode` crate
- [x] `opcode.rs` — 61 opcodes including `LoadFloat64`, `Yield`, `Resume`, `InitGenerator`
- [x] `BytecodeProgram` struct with string + float constant pools
- [ ] Document multi-entry convention: `Resume` only for generators
- [ ] `block.rs` — Basic block builder, CFG construction
- [ ] `analysis.rs` — Liveness analysis (for generator locals), escape analysis

### `rune_parser` crate
- [x] `lexer.rs` — UTF-16 lexer, surrogate pairs, line terminators, ASI
- [x] `parser.rs` — Recursive-descent with precedence climbing, compact AST; `switch/case` statement per §14.12
- [x] `emitter.rs` — On-the-fly bytecode emission with string + float pool interning
- [x] String/template literals emit `LoadStringConst` (GC-allocated HeapString)
- [x] Float literals emit `LoadSmi` (if integer in range) or `LoadFloat64` (GC-allocated HeapFloat64)
- [x] Object literals create shapes with named property keys
- [x] Dot access (`obj.a`) emits property name as string constant
- [ ] Fuzz with `cargo-fuzz`

### `rune_interpreter` crate
- [x] `vm.rs` — Full bytecode interpreter, 61 opcodes
- [x] Shape-based property lookup in `LoadProperty`/`StoreProperty`; `StoreProperty` adds new properties via shape transition
- [x] Object literal creates shape with named entries via string pool
- [x] `HeapString` → `PropertyKey` conversion for runtime property access
- [x] `MakeFunction` / `Call` / `Return` with call frame stack
- [x] Named function binding for recursion (locals[0] = self reference)
- [x] `BytecodeProgram.named_function` flag for self-reference locals
- [x] `Func.prog_ptr` stores creator program pointer for cross-frame function lookup
- [x] `builtins.rs` — Builtins (`print`, `String`, `Object`, `Error`, `Test262Error`, `$DONOTEVALUATE`, `eval`) dispatch via negative Smi handles
- [x] `generator.rs` — `Yield` / `Resume` opcodes, plain functions skip `Resume`
- [x] Stub `YieldStar` runtime helper
- [x] String content comparison for `===`/`!==` (per §7.2.11 SameValueNonNumber)
- [x] String lexicographic comparison for `<`/`>`/`<=`/`>=` (per §7.2.12 IsLessThan)
- [x] `TypeOf` checks GC header tag for `"string"`, `"function"`, and `TAG_FLOAT64 → "number"`
- [x] GC root registration: `Vm::register_roots()` registers stack, locals, try_stack, generators, globals
- [x] Builtin signature includes `&Vm` for access to eval callback and VM state
- [x] **Float64 support**: GC-allocated `HeapFloat64` with `TAG_FLOAT64` (3-bit header tag); `LoadFloat64` opcode; `to_number()`/`number_result()` helpers for float arithmetic; `Add`/`Sub`/`Mul`/`Div`/`Mod`/`Exp`/`Neg` handle float operands; `typeof` returns `"number"`; `-0.0` preserved via `is_sign_negative()` check; `Mod` zero-divisor returns NaN; `Exp` negative exponent works; `ToNumber(null)`→0.0
- [x] **switch/case statement**: `Stmt::Switch` AST variant, `SwitchCase` struct; parser handles `case`/`default` with fall-through; emitter uses two-section architecture (comparison chain + body section) — comparison chain uses `Dup`/`StrictEq`/`JumpIfFalse` with `Jump`-to-body for matches; body section emits case bodies sequentially with natural fall-through; `switch_exit_stack` + `switch_break_jumps` handles break targeting; no-match `Pop` + `Jump` default/after after comparison chain
- [x] **Audited & Verified**: 138/138 tests pass. 5 spec compliance patches confirmed: `5 % 0`→NaN, `2 ** -1`→0.5, `null + 1`→1, `-0.0` preservation, `true + 1`→2 (booleans are Smi(0)/Smi(1) so `to_number` works implicitly). Switch fix: double-patched skip jumps resolved, fall-through working.

### `rune_embed` crate
- [x] `eval()` returns `Result<Value, String>` — parse → emit → execute pipeline
- [x] 66 integration tests: literals, arithmetic, if/while/for, var decl, objects, property get/set, function calls, recursion, generator yield/resume, try/catch/finally, builtins, typeof, float literals, switch/case, spec compliance (mod-zero, exp-negative, null+number, -0, typeof-float)

### `rune_cli` crate
- [x] CLI evaluates JS source strings via `rune_embed::Context::eval`
- [x] `test262.rs` — Full harness: fetch suite, run tests, compare outcomes; skips $DONOTEVALUATE tests; catch_unwind for panic survival
- [x] Test262 results: `typeof` 15/16 (93.75%), `addition` 15/48 (31%), `subtraction` 9/38 (24%)

### Acceptance Criteria
- [ ] >95% Test262 pass rate (excl. Intl, modules, WeakRef, Proxy)
- [x] 138/138 unit + integration tests pass across workspace
- [ ] All opcode unit tests pass
- [x] Generator: yield + resume works manually
- [ ] Non-generator `return 1` has no `Resume` opcode (verify by disassembly)

---

## Sprint 3 — Prototype Chain + Shape-Indexed Dispatch Tables (SIDT)

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §10.1 (ordinary object internal methods), §10.1.7.1 (OrdinaryGet), §10.1.7.3 (OrdinarySet), §14.7.2 (for-in) — open each linked `https://tc39.es/ecma262/multipage/` URL via `webfetch` for exact algorithms. No guessing.
>
> **V8-Beating Strategy:** SIDT replaces V8's 4-state IC (uninit→mono→poly→megamorphic cliff) with an always-O(1) dispatch table indexed by shape.id. No warmup penalty, no megamorphic degradation.

### Task 3A: Prototype Chain 🔴 — Priority 1 ✅
- [x] `JSObject`: add `prototype *mut u8` field at offset 24 → `OBJECT_HEADER_END = 32`
- [x] GC: scan prototype pointer in `TAG_OBJECT` scanning in `gc.rs`
- [x] `LoadProperty` walks prototype chain per §10.1.7.1 OrdinaryGet via `load_property_recursive()`
- [x] `StoreProperty` always sets on receiver per §10.1.7.3 OrdinarySet (already correct)
- [ ] `new Constructor()` sets prototype to `Constructor.prototype` (deferred — needs function property support)
- [x] `Object.create(proto)` builtin — via `object_create_builtin` + Object wrapper with shape {create: builtin_handle}
- [x] 3 integration tests: `test_prototype_chain_get`, `test_prototype_set_own_property`, `test_prototype_shadow`
- **Acceptance:** ✅ prototype chain works for get access; set creates own property on receiver; Object.create creates object with given prototype

### Task 3B: Shape-Indexed Dispatch Tables (SIDT) 🔥 — Priority 2
- [ ] `InlineCache` struct with `HashMap<u64, usize>` (shape.id → slot offset)
- [ ] Attach IC index to `LoadProperty`/`StoreProperty` instructions
- [ ] First access: record shape→slot in IC; subsequent: direct slot access if shape known
- [ ] No megamorphic fallback — entries table grows unboundedly, O(1) HashMap dispatch
- [ ] `test_ic_monomorphic`, `test_ic_polymorphic`, `test_ic_miss` tests
- **Note:** JIT integration deferred to Phase 3; interpreter IC infrastructure only

### Task 3C: for-in Loop 🟡 — Priority 3
- [ ] `IterBegin`/`IterNext` opcodes (or counter-based pattern)
- [ ] Emit `for (var key in obj)` using own enumerable property keys from shape
- [ ] Once 3A lands: extend to enumerate inherited keys per §14.7.2

### Task 3D: Array & String Builtins 🟡 — Priority 4
- [ ] Move builtins to `rune_builtins` crate with `register_all(vm)` API
- [ ] Dense array layout: `[GcHeader|shape|length:u32|capacity:u32|elements:Value[]]`
- [ ] `Array.prototype.push/pop`, `String.fromCharCode/charAt/length/slice`
- [ ] `Math.floor/ceil/abs/min/max/pow/sqrt/PI/E`
- **Architecture:** Dense arrays with shaped objects — `arr[0]` goes through SIDT to direct load

### Task 3E: CFG & Liveness Analysis 🟢 — Priority 5
- [ ] `block.rs` — Basic block builder, CFG construction
- [ ] `analysis.rs` — Liveness analysis (for generator locals), escape analysis

### Acceptance — Sprint 3 ✅
- [x] 141 tests pass across workspace (69 integration + 72 unit)
- [x] Prototype chain: property get walks proto chain; set creates own property
- [ ] SIDT: IC entries grow unboundedly without megamorphic cliff (deferred to Sprint 4)
- [ ] for-in: own keys enumerated (deferred to Sprint 4)
- [ ] Array literal + push/pop + length works (deferred to Sprint 4)
- [ ] String .charAt / .slice / .length works (deferred to Sprint 4)

### Audit — Task 3A Issues (Sprint 4 fixes)
- [ ] 3A-1: `load_property_recursive()` needs MAX_PROTOTYPE_DEPTH=256 cycle guard
- [ ] 3A-2: `New` opcode doesn't set prototype from Constructor.prototype
- [ ] 3A-3: `Object.create(non_object)` should throw TypeError
- [ ] 3A-4: Object constructor ignores argument (documented, acceptable for now)
- [ ] 3A-5: `prototype()` returns raw `*mut u8` — safe currently but fragile

---

## Sprint 4 — SIDT + Dense Arrays + Builtins

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §10.1 (OrdinaryGet/Set), §11.2.2 ([[Construct]]), §14.7.2 (for-in), §22–24 (Number/Math/String), §26 (Array). Open linked URLs via `webfetch`. No guessing.
>
> **V8-Beating Strategy:** SIDT replaces V8's 4-state IC (uninit→mono→poly→megamorphic cliff) with always-O(1) dispatch table indexed by shape.id. Dense arrays skip shape lookup entirely — single instruction element load.

### Task 4A: Prototype Chain Fixes 🔴 — Priority 0
- [ ] `load_property_recursive()`: add `MAX_PROTOTYPE_DEPTH = 256` cycle guard
- [ ] `New` opcode: set prototype from `Constructor.prototype` after creating new object
- [ ] `Object.create(non_object)` → TypeError per §20.1.2.2
- **Acceptance:** `new Foo()` inherits from `Foo.prototype`; cycles don't hang the interpreter; `Object.create(42)` throws

### Task 4B: SIDT — Interpreter Inline Caches 🔥 — Priority 1 (V8-beating Innovation #1)
- [ ] `InlineCache` struct: `HashMap<u64, IcEntry>` (shape.id → slot offset + proto_depth)
- [ ] Attach optional `ic_index` to `LoadProperty`/`StoreProperty` instructions
- [ ] Fast path: IC hit → direct slot access (own) or proto-walk (inherited)
- [ ] Slow path: full shape + prototype walk → populate IC entry → never megamorphic
- [ ] `test_ic_monomorphic`, `test_ic_polymorphic`, `test_ic_proto_inherited`
- **Acceptance:** 10+ shapes at one callsite → still O(1) dispatch, no megamorphic cliff

### Task 4C: Dense Array Implementation 🟡 — Priority 2
- [ ] `TAG_ARRAY = 4` GC tag, separate from TAG_OBJECT
- [ ] Dense array layout: `[GcHeader|shape|length:u32|capacity:u32|proto:*mut u8|elements:Value[]]`
- [ ] `Shape::is_dense_array` flag for shape ID
- [ ] `LoadProperty` with numeric index on TAG_ARRAY → direct elements access
- [ ] Array literal `[a, b, c]` allocates dense array with shape + elements
- **Architecture:** No holes (empty slots = undefined). One instruction load in JIT.

### Task 4D: Array & String Builtins 🟡 — Priority 3
- [ ] Move builtins to `rune_builtins/` crate: `lib.rs`, `object.rs`, `arrays.rs`, `strings.rs`, `math.rs`, `errors.rs`
- [ ] `Array.prototype.push/pop`, `Array.isArray`
- [ ] `String.fromCharCode`, `String.prototype.charAt/length/slice`
- [ ] `Math.floor/ceil/abs/min/max/pow/sqrt/PI/E`
- **Architecture:** Prototype objects in `init_builtin_wrappers()` with method handles
- **Acceptance:** `arr.push(1)`, `"hi".charAt(0)`, `Math.floor(3.7)` all work

### Task 4E: for-in Loop 🟢 — Priority 4
- [ ] Own enumerable keys from shape entries
- [ ] For dense arrays: keys = `"0"`..`"length-1"`
- [ ] `for (var k in obj)` emitter with IterBegin/IterEnd or counter pattern

### Task 4F: CFG & Liveness Analysis 🟢 — Priority 5
- [ ] `block.rs` — Basic block builder, CFG construction
- [ ] `analysis.rs` — Liveness analysis

### Acceptance — Sprint 4
- [ ] 145+ tests pass across workspace
- [ ] SIDT: IC entries grow unboundedly, no megamorphic performance cliff
- [ ] Dense arrays: `arr[0]` direct load, no shape lookup
- [ ] for-in: own keys enumerated
- [ ] Array push/pop/length, String charAt/slice, Math.floor/sqrt
- [ ] New Foo() inherits from Foo.prototype

---

## Phase 3 — Baseline Copy-and-Patch JIT

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §11 ([[Call]]/[[Construct]]), §29.3 (generator JIT) — open each linked `https://tc39.es/ecma262/multipage/` URL via `webfetch` for exact call semantics and generator dispatch. No guessing.

**Goal:** Copy-and-patch JIT for normal + generator functions. Monomorphic ICs. No deoptimisation.

### `rune_jit_baseline` crate
- [ ] `templates.rs` — Pre-compiled binary templates per bytecode (position-independent)
- [ ] Build script generating templates from assembly stubs
- [ ] `assembler.rs` — Memory mgmt: RW alloc → copy → patch → mprotect RX → aarch64 `__clear_cache`
- [ ] Simple patcher for immediates / jump offsets
- [ ] `codegen.rs` — Walk bytecode, select templates, wire CFG, insert IC stubs
- [ ] `ic.rs` — Monomorphic stub: compare shape → load at offset → else polymorphic stub (2-entry) → else runtime
- [ ] IC stubs hand-written in assembly

### `rune_interpreter` integration
- [ ] Call counter per function (threshold=50)
- [ ] Trigger JIT → replace entry point with JIT code pointer
- [ ] Safepoints at function entry for MMTk

### Tests
- [ ] JIT `add3` correctness (like spike)
- [ ] IC hit/miss: different shapes, correct adaptation
- [ ] Generator JIT: `function* g() { yield 1; yield 2; }`
- [ ] Fuzz: random scripts via interpreter vs JIT, compare

### Acceptance Criteria
- [ ] Test262 >95% with JIT enabled
- [ ] No crashes after 1M JIT compilations in stress test
- [ ] Tight loop: ≥1.5× speedup over interpreter

---

## Phase 4 — Generators & Async Generators Runtime

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §15.6 (generator definitions), §29.3 (Generator objects, GeneratorYield, YieldStar) — open each linked `https://tc39.es/ecma262/multipage/` URL via `webfetch` for exact yield/resume/throw semantics. No guessing.

**Goal:** Full heap-frame with try/catch/finally + `yield*` semantics. Async generators.

### `rune_core`
- [ ] `GeneratorFrame` object: state, resume_mode, locals, try_stack
- [ ] Shape for `GeneratorFrame`

### `rune_interpreter/generator.rs`
- [ ] `Resume` opcode: switch on state + resume_mode
- [ ] `Yield` opcode: store state, pack frame, return
- [ ] try_stack push/pop on try block entry/exit
- [ ] `YieldStar` helper (full spec semantics)

### Async generators
- [ ] Extend `GeneratorFrame` with promise for `next()` result
- [ ] Wire through existing Promise builtin

### JIT integration
- [ ] Multi-entry dispatch works in baseline JIT

### Tests
- [ ] Test262 §25.3 generator tests
- [ ] Test262 §25.5 async generator tests
- [ ] Complex: yield inside try/catch, nested try/finally, return() during suspend

### Acceptance Criteria
- [ ] All Test262 generator tests pass

---

## Phase 5 — Cranelift Mid-Tier

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §9 (execution contexts), §11 (calls) — open each linked `https://tc39.es/ecma262/multipage/` URL via `webfetch` for exact semantics preserved under optimisation. No guessing.

**Goal:** Background compilation tier for hot functions (≥10K calls). Escape analysis eliminates short-lived allocations.

### `rune_bytecode/analysis.rs`
- [ ] Escape analysis pass: allocation is replaceable if not stored to heap, passed to unknown call, or returned
- [ ] Transform bytecode: replace allocation with virtual registers, property accesses → direct moves

### `rune_jit_cranelift` crate
- [ ] `lower.rs` — Lower (optionally transformed) bytecode to CLIF via `FunctionBuilder`
- [ ] Shape-check sequences: inline fast path + branch to shared slow path
- [ ] `scalar.rs` — Scalar replacement using escape analysis results
- [ ] `compile.rs` — Background compilation thread, hotness threshold ≥10K calls
- [ ] Atomic hot-swap of function entry point at safepoint (`Ordering::Release`/`Acquire`)

### Testing
- [ ] Correctness: compile hot function, swap, verify vs interpreter
- [ ] Speed: numeric loop benchmark significant improvement over baseline
- [ ] Escape analysis: non-escaping loop allocation → zero heap allocations

### Acceptance Criteria
- [ ] No Test262 regressions
- [ ] Tight loop within 3× of V8's performance
- [ ] Queue with backpressure for background compilation

---

## Phase 6 — Modules, Builtins, Proxy, WeakRef, Regex

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §16–§30 — open each linked `https://tc39.es/ecma262/multipage/` URL via `webfetch` for exact built-in constructor/prototype algorithms. No guessing.

**Goal:** Full built-in library in Rust. ESM modules. Proxy. Linear-time regex.

### `rune_builtins`
- [ ] Object (§17), Function (§18), Boolean (§19), Symbol (§20)
- [ ] Error types (§21): Error, TypeError, RangeError, SyntaxError, ReferenceError, etc.
- [ ] Number + Math (§22)
- [ ] BigInt (§23)
- [ ] String (§24) — all prototype methods
- [ ] Indexed Collections (§26): Array, TypedArrays, DataView
- [ ] Keyed Collections (§27): Map, Set, WeakMap, WeakSet
- [ ] Structured Data (§28): ArrayBuffer, SharedArrayBuffer, JSON, Atomics
- [ ] Control Abstraction (§29): Promise, Iterator
- [ ] WeakRef / FinalizationRegistry (MMTk reference processing)
- [ ] Intl basics (Test262 passable)

### `rune_regex`
- [ ] `parse.rs` — JS regex parser (Unicode flag aware)
- [ ] `nfa.rs` — Thompson NFA construction
- [ ] `pikevm.rs` — Pike VM over `u16`, leftmost-first capture
- [ ] `backtrack.rs` — Bounded backtracker for backreferences/lookbehind (effort cap)
- [ ] Unicode property tables via `unicode-ident` crate

### `rune_module`
- [ ] Loader, linker, evaluation
- [ ] Top-level await via module evaluation loop

### `rune_interpreter`
- [ ] Proxy support: special shape → IC miss → runtime trap handler

### Acceptance Criteria
- [ ] >99% Test262 (excl. Temporal, full Intl, deferred recent features)
- [ ] No ReDoS vulnerabilities (proven by fuzzing)

---

## Phase 7 — GenImmix Upgrade & CDP Debugger

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §6 (types/GC invariants), Annex C (host layering for debugger hooks) — open each linked `https://tc39.es/ecma262/multipage/` URL via `webfetch`. No guessing.

**Goal:** Generational bump-pointer GC (GenImmix). Chrome DevTools Protocol debugger.

### MMTk Upgrade
- [ ] Change MMTk plan to `GenImmix`
- [ ] Forwarding pointer: shape pointer word → new address during evacuation
- [ ] `ObjectModel::get_forwarding_pointer` / `store_forwarding_pointer`
- [ ] Card-table write barrier (replace no-op barrier)
- [ ] GC stress fuzzer: random alloc + mutation + forced collection

### `rune_debugger`
- [ ] WebSocket server (CDP transport)
- [ ] Breakpoints, stepping
- [ ] Call stack inspection, variable inspection
- [ ] Basic profiling integration

### Acceptance Criteria
- [ ] Debugger can pause, inspect, resume
- [ ] No Test262 regressions after GC upgrade
- [ ] Minimal heap fragmentation under long-running workloads

---

## Phase 8 — Fuzzing, Optimization & Stabilization

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §2 (conformance requirements) — open linked `https://tc39.es/ecma262/multipage/` URL via `webfetch`. Every fuzzer finding must be verified against the spec. No guessing.

**Goal:** Continuous fuzzing, performance tuning, community beta.

### Fuzzing
- [ ] Grammar-based JS fuzzer comparing Rune vs V8
- [ ] Bytecode mutator fuzzer (JIT testing)
- [ ] GC stress fuzzer
- [ ] All fuzzers running in CI continuously

### Performance
- [ ] Profile real serverless workloads (React SSR snippet, etc.)
- [ ] Identify and fix bottlenecks
- [ ] Document performance numbers vs QuickJS and Boa

### Documentation
- [ ] Embedder's guide
- [ ] API docs

### Acceptance Criteria
- [ ] Zero unique crashes after 1 month continuous fuzzing
- [ ] Test262 ≥99% stable
- [ ] Performance numbers published

---

## Phase 9 — v2 Features (Stretch)

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) for any spec-level features — open linked `https://tc39.es/ecma262/multipage/` URLs via `webfetch`. No guessing.

- [ ] Heap pointer-compression sandbox (Spectre mitigation)
- [ ] Temporal API
- [ ] Enhanced Intl (full CLDR)
- [ ] WebAssembly module

---

## Global Testing Strategy

> **Spec mandate:** Every test expectation must be traceable to an ECMA-262 algorithm in [`ecma262.md`](./ecma262.md). Open linked `https://tc39.es/ecma262/multipage/` URLs via `webfetch` when writing tests. No guessing — if a test expects `42`, the spec must say so.

- **Unit tests:** every crate; run with `cargo test` + `cargo miri test`
- **Test262:** CI integration; >95% from Phase 2
- **Differential fuzzing:** Rune vs V8 on random programs
- **ASAN/UBSAN:** all development builds
- **Cargo-fuzz:** targets for parser, bytecode, GC
