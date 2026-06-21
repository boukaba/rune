# Rune — Implementation Progress

> **Project:** Production-ready JavaScript runtime in Rust
> **Spec Target:** ECMAScript 2027 (ECMA-262, 18th Edition)
> **Status:** Sprint 10 — JIT Smi bail-out ✅ (10A, 10B done)

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

### Task 4A: Prototype Chain Fixes 🔴 — Priority 0 ✅
- [x] `load_property_recursive()`: add `MAX_PROTOTYPE_DEPTH = 256` cycle guard
- [x] `New` opcode: set prototype from `Constructor.prototype` after creating new object (heap-object constructors)
- [x] `Object.create(non_object)` → TypeError per §20.1.2.2 (via panic, exception system deferred)
- [ ] `New` opcode: call constructor body with `this` binding (deferred to Sprint 5)
- [ ] `"prototype"` key interning to avoid HeapString alloc on every `new` (deferred to Sprint 5)
- **Acceptance:** ✅ cycle guard prevents hangs; `new Object()` works; `Object.create(42)` throws

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

### Acceptance — Sprint 4 (partial)
- [x] 142 tests pass across workspace (70 integration + 72 unit)
- [x] Prototype cycle guard and Object.create validation
- [ ] SIDT: IC entries grow unboundedly, no megamorphic performance cliff (deferred to Sprint 5)
- [ ] Dense arrays: `arr[0]` direct load, no shape lookup (deferred to Sprint 5)
- [ ] Array push/pop/length, String charAt/slice, Math.floor/sqrt (deferred to Sprint 5)
- [ ] New Foo() inherits from Foo.prototype (partial — prototype set but constructor body not called)
- [ ] for-in: own keys enumerated (deferred to Sprint 5)
- [ ] Prototype key interning (deferred to Sprint 5)

---

## Sprint 5 — SIDT ICs + Dense Arrays + Builtins

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §10.1 (OrdinaryGet/Set), §11.2.2 ([[Construct]]), §14.7.2 (for-in), §22–24 (Number/Math/String), §26 (Array). Open linked URLs via `webfetch`. No guessing.
>
> **V8-Beating Strategy:** SIDT replaces V8's 4-state IC (uninit→mono→poly→megamorphic cliff) with always-O(1) dispatch table indexed by shape.id. Dense arrays skip shape lookup entirely — single instruction element load in JIT.

### Task 5A: SIDT — Interpreter Inline Caches 🔥 — Priority 1 (V8-beating Innovation #1)
- [x] `InlineCache` struct: `HashMap<u64, IcEntry>` (shape.id → slot offset + proto_depth)
- [x] Attach optional `ic_index` to `LoadProperty`/`StoreProperty` instructions in BytecodeProgram.ics
- [x] Fast path: IC hit → direct slot access (own) or proto-walk (inherited)
- [x] Slow path: full shape + prototype walk → populate IC entry → never megamorphic
- [x] `test_ic_monomorphic`, `test_ic_polymorphic`, `test_ic_proto_inherited`
- **Acceptance:** 10+ shapes at one callsite → still O(1) dispatch, no megamorphic cliff ✅

### Task 5B: Dense Array Implementation 🟡 — Priority 2
- [ ] `TAG_ARRAY = 4` GC tag, separate from TAG_OBJECT
- [ ] Dense array layout: `[GcHeader|shape|length:u32|capacity:u32|proto:*mut u8|elements:Value[]]`
- [ ] `LoadProperty` with numeric index on TAG_ARRAY → direct elements access
- [ ] Array literal `[a, b, c]` allocates dense array with shape + elements

### Task 5C: Array & String Builtins 🟡 — Priority 3
- [ ] Move builtins to `rune_builtins/` crate: `lib.rs`, `object.rs`, `arrays.rs`, `strings.rs`, `math.rs`
- [ ] Builtin signature change: `fn(gc, this: Value, args, &Vm) -> Value`
- [ ] `Array.prototype.push/pop`, `Array.isArray`
- [ ] `String.fromCharCode`, `String.prototype.charAt/length/slice`
- [ ] `Math.floor/ceil/abs/min/max/pow/sqrt/PI/E`

### Task 5D: New Opcode — Call Constructor Body 🟡 — Priority 4
- [ ] Add `this: Value` to Frame struct
- [ ] When `new Foo(args)`: create object → set prototype → call Foo with this=newObj → check result

### Task 5E: CFG & Liveness Analysis 🟢 — Priority 5
- [ ] `block.rs` — Basic block builder, CFG construction
- [ ] `analysis.rs` — Liveness analysis

### Task 5F: Prototype Key Interning 🟢 — Priority 6
- [x] Intern `"prototype"` as a static PropertyKey in `rune_core::shape` to avoid HeapString alloc on every `new` call
- [x] Also apply to any other hot-path string allocations in `New` opcode

### Acceptance — Sprint 5
- [x] 74+ tests pass across workspace (74 integration + 27 unit + 5 core + 5 parser = 111+)
- [x] SIDT: IC entries persist across eval calls; same-shape second execution hits 10/10
- [x] `load_property_recursive_ic` populates IC for all result types (Smi, Float64, heap, undefined)
- [ ] Dense arrays: `arr[0]` direct load via IC
- [ ] Array push/pop/length, String charAt/slice, Math.floor/sqrt
- [ ] New Foo() calls constructor body with this binding
- [ ] For-in: own keys enumerated

---

## Sprint 6 — Dense Arrays + Builtins + Constructor `this`

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §10.1 (OrdinaryGet/Set), §11.2.2 ([[Construct]]), §22–24 (Number/Math/String), §26 (Array). Open linked URLs via `webfetch`. No guessing.
>
> **V8-Beating Strategy:** Dense arrays make ICs useful for the most common JS operation (array element access). `arr[0]` through an IC hit on `TAG_ARRAY` lets the JIT emit a single `mov` instruction — V8 needs multiple shape checks for the same.

### Task 6A: IC Smi Result Fix 🔴 — Priority 0 ✅
- [x] Remove `result.is_heap_object()` guard in `load_property_recursive_ic`
- [x] `test_ic_hits_across_evals` verifies: first eval populates (10 misses), second eval hits (10 hits)

### Task 6B: Dense Array Implementation 🔥 — Priority 1
- [x] `TAG_ARRAY = 4` GC tag in `gc.rs`, `RuneArray` struct in `rune_core/src/array.rs`
- [x] Array layout: `[GcHeader(TAG_ARRAY) | shape_ptr | length: u32 | capacity: u32 | prototype: *mut u8 | elements: Value[]]`
- [x] GC scanning: same as TAG_OBJECT (forward prototype then elements)
- [x] `NewArray` allocates `RuneArray` instead of `JSObject`
- [x] `LoadProperty` numeric-index fast path on `TAG_ARRAY` (bypass shape lookup)
- [x] `StoreProperty` numeric-index set on `TAG_ARRAY`
- [x] `value_to_array_index` helper
- [x] IC integration: numeric index hit populates `IcEntry { offset: index, is_own: true, proto_depth: 0 }`
- [x] `DENSE_ARRAY_SHAPE` shared shape with `is_dense_array: true`
- [x] 4 integration tests: literal, get element, out of bounds, set element

### Task 6C: Array & String Builtins + `this` Binding 🟡 — Priority 2
- [x] `BuiltinFn` signature change: `fn(gc, this: Value, args: &[Value], vm: &Vm) -> Value`
- [x] Prototype method `this` detection: Call opcode pops `this` from stack
- [x] Emitter change: method calls emit `[receiver, method, args...]`, regular calls emit `[undefined, callee, args...]`
- [x] `Frame.this` field: set when calling user-defined functions
- [x] `Array.prototype.push` / `pop` — access `this` as TAG_ARRAY
- [x] `String.prototype.charAt` / `slice` — access `this` as TAG_STRING
- [x] `String.prototype.length` — handled directly in LoadProperty for TAG_STRING
- [x] `Math.floor/ceil/abs/min/max/pow/sqrt` — return Smi when result is integer
- [x] String property access: numeric index → char at index; non-numeric → walk String.prototype
- [x] Array.prototype stored in `Vm::array_prototype`, set on NewArray
- [x] String.prototype stored in `Vm::string_prototype`
- [ ] `Array.isArray` — deferred (needs Array constructor wrapper without conflicting with Array builtin)
- [ ] `String.fromCharCode` — deferred (same issue)
- [ ] Math constants (PI, E) — deferred
- [ ] Move builtins to `rune_builtins/` crate — deferred

### Task 6D: `New` Calls Constructor Body 🟡 — Priority 3
- [ ] `this` field in `Frame` struct
- [ ] `New` sets up frame with `this` = new object
- [ ] Constructor return value handling (object vs primitive)

### Task 6E: `for-in` Loop 🟢 — Priority 4
- [ ] Own enumerable shape entries as string keys
- [ ] Dense array: `0..length-1` as string keys

### Task 6F: CFG & Liveness Analysis 🟢 — Priority 5
- [ ] `block.rs` — Basic block builder, CFG construction
- [ ] `analysis.rs` — Liveness analysis

### Acceptance — Sprint 6
- [x] `arr[0]` via IC hit bypasses shape lookup (JIT-ready: single `mov`)
- [x] `arr.push(1)`, `arr.pop()`, `"hi".charAt(0)`, `Math.floor(3.7)` all work
- [x] `new Foo(name)` calls constructor body with `this` = new object
- [x] `for (var k in obj)` iterates own keys
- [x] 86+ integration tests pass (8 new: push/pop, charAt, slice, length, floor/ceil/abs/sqrt)

---

## Sprint 7/8 — Constructor `this` + `.prototype` + Arrays + For-in

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §11.2.2 ([[Construct]]), §26.1 (Array exotic object), §10.1.7 (OrdinaryGet/Set). Open linked URLs via `webfetch`. No guessing.

### Task 7A: Constructor `this` binding + Parser `new` fix 🔥 — Priority 1 ✅
- [x] `Frame::this` field: populated on `Call` and `New` opcodes
- [x] `New` opcode pushes a full frame for `TAG_FUNC` constructors with `this = obj_val`
- [x] `Return` opcode: if `is_constructor_call` and return value is primitive, use `constructed_object`
- [x] Parser fix: `new Foo()` was incorrectly parsed as `Call(New(Foo), [])` instead of `New(Foo, [])`
- [x] `parse_primary_refactoring`: `parse_primary_inner()` → no postfix; `parse_member_expr()` → member-only postfix (no calls); `new` uses `parse_member_expr()`
- [x] 3 integration tests: basic constructor this binding
- **Acceptance:** ✅ `new Foo(42)` correctly passes `Foo.prototype` object as `this` to Foo body; parser produces correct `New(Foo, [42])` AST

### Task 8A: Constructor `.prototype` property 🟡 — Priority 2 ✅
- [x] `Func` layout extended from 24→32 bytes with `prototype: *mut u8` field
- [x] `MakeFunction` creates a default empty `JSObject` prototype
- [x] `New` opcode reads `Func::prototype()` and sets it as the new object's `[[Prototype]]`
- [x] `StoreProperty`/`LoadProperty` on `TAG_FUNC` handle the `"prototype"` key
- [x] GC `scan_end` for `TAG_FUNC` returns 32 bytes; Cheney scan forwards `TAG_FUNC` prototype pointer
- [x] 6 test assertions: own properties, inheritance, shadowing, dynamic mutation, constructor accessibility

### Task 8B: Array Reallocation (Grow) 🟡 — Priority 3 ✅
- [x] `RuneArray::grow()` — allocate new array with ~1.5x capacity, copy header + elements, zero new slots
- [x] `RuneArray::push()` — now returns `*mut RuneArray` (new pointer if grown), auto-grows on capacity exhaustion
- [x] `RuneArray::shape_ptr()`/`set_shape_ptr()`/`prototype()`/`set_prototype()` accessors for grow copy
- [x] `BuiltinFn` signature: `fn(gc, this, args, vm: &mut Vm)` (was `&Vm`)
- [x] All 21 builtins updated to `&mut Vm` signature
- [x] `Vm::update_heap_reference(old_ptr, new_ptr)` — scans stack, all frame locals, and globals for stale pointers
- [x] `array_push` builtin calls `update_heap_reference` after grow
- [x] 2 integration tests: `test_array_push_grow`, `test_array_push_grow_identity`
- [x] `load_property_recursive` handles `"length"` key on `TAG_ARRAY`
- **Acceptance:** ✅ Array auto-grows on push beyond initial capacity; aliased variables (`var b = a`) point to same grown array

### Task 8C: Deferred Builtin Cleanup 🟢 — Priority 4 ✅
- [x] `Array.isArray` — Array constructor wrapper with `isArray` property in builtin_wrappers
- [x] `String.fromCharCode` — String constructor wrapper with `fromCharCode` property (shadows `String(42)` as callable, consistent with Object wrapper pattern)
- [x] Math constants (PI, E) — HeapFloat64 values in Math object shape slots
- [x] `charAt` OOB returns `""` per §22.1.3.1 (was `undefined`; also fixed bogus `ch == '\0'` guard)
- [x] String `.length` counts UTF-16 code units per §22.1.4.1 via `encode_utf16().count()`

### Task 8D: `for-in` Loop 🟢 — Priority 5 ✅
- [x] Parser: detect `for (var x in obj)` and `for (expr in obj)` in `parse_for()`
- [x] Emitter: `ForInInit` + `ForInNext` opcodes, register loop variable as local
- [x] VM: `ForInInit` pushes obj + smi(0); `ForInNext` iterates shape `key_names` (objects) or `0..length-1` (arrays)
- [x] Shape: `key_names: Vec<String>` field, `key_name_at()` for for-in enumeration
- [x] `add_property`/`intern`/`intern_with_parent` thread key names through
- [x] `Pop` after `StoreLocal` in ForIn emitter (StoreLocal pushes back)
- [x] `value_to_array_index` handles numeric strings for array for-in access
- [x] **IC key fix**: `(shape.id, key_hash)` instead of `shape.id` — computed property access with changing keys (e.g. for-in body `o[k]`) no longer hits stale cache entries
- [x] 4 integration tests: object, array, empty, null
- [x] 170 tests pass (98 integration + 27 interpreter + 10 core + 25 parser + 5 gc + 5 gc acc + 2 spike)

### Task 8E: CFG & Liveness Analysis 🟢 — Priority 6 ✅
- [x] `block.rs` — `build_cfg()`: leader identification, block partitioning, edge computation (Jump, JumpIfTrue/JumpIfFalse, ForInNext, Return, Throw, fall-through)
- [x] `analysis.rs` — `liveness()`: iterative dataflow with per-block use/def sets, live_in/live_out computation
- [x] `BytecodeProgram::build_cfg()` and `::liveness()` convenience methods on `BytecodeProgram`
- [x] 6 unit tests: linear, if-else, loop, ForInNext CFG + multi-block liveness, loop liveness
- [x] 176 tests pass (6 new bytecode + 170 existing)

### Acceptance — Sprint 7
- [x] `new Foo(42)` works with both `this` binding and prototype inheritance
- [x] Array auto-grows on push; `a.length` returns correct length
- [x] 176 tests pass (98 integration + 27 interpreter + 10 core + 25 parser + 6 bytecode + 5 gc + 5 gc acc + 2 spike)
- [x] `Array.isArray([1,2,3])` returns true; `Array.isArray(42)` returns false
- [x] `String.fromCharCode(65)` returns a heap string
- [x] `Math.PI` and `Math.E` are accessible as float64 values
- [x] `charAt` OOB returns empty string; string `.length` counts UTF-16 code units
- [x] `for (var k in obj)` iterates own keys — object properties (shape key_names) and array indices

---

## Phase 3 — Baseline Direct-Emission JIT

> **Spec mandate:** See [`ecma262.md`](./ecma262.md) §11 ([[Call]]/[[Construct]]), §29.3 (generator JIT) — open each linked `https://tc39.es/ecma262/multipage/` URL via `webfetch` for exact call semantics and generator dispatch. No guessing.

**Goal:** Direct-emission JIT for normal + generator functions. Smi-only fast paths. Monomorphic ICs pending.

### `rune_jit_baseline` crate
- [x] `assembler.rs` — ExecutableMemory (mmap MAP_JIT / MAP_ANONYMOUS, mprotect W^X, Drop-unmapped). x86-64 helpers: ret, nop, mov imm64/rm64/mem_disp32, add/sub/cmp imm32, jmp/je/jne/jbe/jb/ja/jae rel32, call/push/pop r64, and/or imm8, add/sub/imul r64 r64, sar/shl by 1, cmp r64 r64, REX.W. 22+ offset tests.
- [x] `codegen.rs` — Walk bytecode → emit native instructions directly (no pre-compiled templates). JitEntryFn = `fn(vm, gc, locals_ptr)`. Prologue saves RBP/R15/R14/R13/RBX, allocates 256-slot JIT value stack. Emits: LoadSmi, LoadUndefined, LoadNull, LoadBoolean, LoadLocal, StoreLocal, Pop, Return, Add/Sub/Mul (Smi), Lt (setl), IncLocal/DecLocal, Jump, JumpIfFalse. Forward jumps via bc_to_native + pending_patches resolution. 22 tests (13 offset + 9 execution cfg-gated x86_64).
- [ ] `ic.rs` — Monomorphic IC stubs (deferred — shape guard comparison in generated code)
- [ ] `templates.rs` — (Not used — direct emission instead of copy-and-patch templates)

### `rune_interpreter` integration
- [x] Trigger JIT → replace entry point with JIT code pointer
- [x] Call counter per function (threshold=50) for hotness detection
- [x] opcode: `is_jit_compatible()` gated on `cfg(all(feature="jit", target_arch="x86_64"))`

### Tests
- [x] JIT `add3` correctness (spike + baseline: Smi arithmetic, variables, branching, loops, conditionals)
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

## Sprint 9: Baseline JIT Foundation 🟢 — Priority 1 (Phase 3 gate)

- [x] **9A: JIT Memory Management + Assembler** — 188 tests passing (+12 new)
  - [x] ExecutableMemory: W^X-compliant allocator (mmap + MAP_JIT/mprotect)
  - [x] x86-64: ret, nop, mov, add/sub/cmp, jmp/je/jne, call, push/pop with REX prefix support
  - [x] 12 unit tests; execution tests cfg-gated to x86_64 (safe on ARM)
  - [x] spike_jit: MAP_JIT conditional for Linux compat
- [x] **9B: Bytecode-to-Native Codegen — Smi Arithmetic** — 190 tests passing (+2 new, +7 cfged)
  - [x] CodeGen struct: prologue/epilogue with callee-saved registers (R15=VM, R14=GC, RBX=JIT stack)
  - [x] Value stack: [rbx]-based push/pop (256 slots on native stack, 2KB)
  - [x] Opcodes: LoadSmi, LoadUndefined, LoadNull, LoadBoolean, Return
  - [x] Smi arithmetic: Add ((a&~1)+b), Sub ((a-b)|1), Mul (decode→imul→encode)
  - [x] 2 offset-verification tests + 7 execution tests (cfg-gated to x86_64)
  - [x] New assembler helpers: and/or imm8, add/sub r64 r64, imul, sar/shl by 1
- [x] **9C: ECMA-262 Spec Compliance — Critical Fixes** — 201 tests passing (+11 new)
  - [x] 9C-1: Lt/Gt/Le/Ge use to_number() for HeapFloat64 + NaN per §12.9–12.11
  - [x] 9C-2: to_number() parses numeric strings per §9.3.1 (empty→0, hex, Infinity, etc.)
  - [x] 9C-3: ++/-- operators — parser (prefix+postfix), AST (Update), emitter, 4 bytecode opcodes (IncLocal, DecLocal, IncGlobal, DecGlobal), VM handlers
  - [x] 9C-4: Neg uses to_number() for all non-numeric types; Smi -(-2^30) overflow → HeapFloat64
  - [x] 9C-5: 11 integration tests (float comparison, NaN, string ToNumber, ++/-- prefix/postfix, for-loop with i++, negate string, negate overflow, negate undefined)
- [x] **9D: JIT Control Flow + Branches** — 19 JIT baseline tests (+5 offset + 4 execution)
  - [x] cmp_r64_r64 (39 /r), jbe/jb/ja/jae rel32 assembler helpers (0F 86/82/87/83)
  - [x] bc_to_native: Vec<usize> mapping bytecode index → native offset
  - [x] pending_patches: Vec<(usize, usize)> for forward branch resolution
  - [x] Jump: emit_jmp_rel32(0) placeholder, record pending patch
  - [x] JumpIfFalse: pop rax, cmp rax 2, jbe target (falsy = undefined/Smi(0)/null)
  - [x] resolve_patches(): rel32 = target_native - (patch_offset + 4) after all instrs
  - [x] 5 offset-verification + 4 execution tests (cfg-gated x86_64): truthy/falsy/undefined conditionals + unconditional jump
  - [x] 208 tests pass across workspace (19 JIT baseline + 109 integration + 52 interpreter + 10 core + 6 bytecode + 5 parser + 5 emitter + 2 spike)
- [x] **9E: JIT Local Variables + Comparison + Loop Execution** — 22 JIT baseline tests (+3 offset + 8 execution)
  - [x] emit_mov_r64_mem_disp32 / emit_mov_mem_disp32_r64 assembler helpers
  - [x] JitEntryFn 3-arg convention: fn(vm, gc, locals_ptr); R13 = locals ptr in prologue/epilogue
  - [x] LoadLocal: mov rax, [r13 + idx*8]; push
  - [x] StoreLocal: pop; mov [r13 + idx*8], rax; push back
  - [x] Pop: discard JIT stack top
  - [x] Lt: setl + movzx + shl + or → Smi(0)=1 or Smi(1)=3
  - [x] IncLocal/DecLocal: load old, add/sub 2 (Smi +1/-1), store back, push new/old
  - [x] Value::from_raw() in rune_core
  - [x] 8 execution tests: local load/store, Lt (true/false/negative), inc postfix, dec prefix, full counting loop sum(0..4)=10
  - [x] 211+ tests pass across workspace (22 JIT baseline + 109 integration + 52 interpreter + 10 core + 6 bytecode + 5 parser + 5 emitter + 2 spike)

## Sprint 10 — JIT Tier-Up: Interpreter Integration

- [x] **10A: Hot Function Detection + JIT Calling Convention**
  - [x] Func layout: 32→48 bytes, add call_count (u32+pad) + jit_entry (u64)
  - [x] GC scan_end TAG_FUNC → 48; jit_entry forwarded as-is (raw pointer)
  - [x] `is_jit_compatible()` in rune_jit_baseline — checks bytecode uses only JIT-supported opcodes
  - [x] `rune_interpreter` optional dep on `rune_jit_baseline` with default `jit` feature (x86_64-gated)
  - [x] Opcode::Call: increment call count per TAG_FUNC call; at threshold 50 compile via CodeGen + store entry
  - [x] Hot function path: transmute JitEntryFn, pass vm/gc/locals_ptr, push result
  - [x] Integration test (x86_64): add() called 100 times, tier-up at 50, sum(0..99)=4950
  - [x] Phase 3 acceptance: interpreter integration gate met ✅
- [x] **10B: JIT Smi Bail-Out — skip JIT for non-Smi inputs**
  - [x] Vm::all_smi() helper — checks all values in a slice are Smi
  - [x] JIT call path guarded: invoke only if all locals/args are Smi
  - [x] Non-Smi values (float64, string, object) fall through to interpreter
  - [x] Integration test (x86_64): add(3.5, 2) bypasses JIT, returns 5.5 via interpreter

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
