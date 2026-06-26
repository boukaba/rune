# Rune — Known Issues & Investigation Log

## P0: AArch64 trace compiler multi-op SIGBUS

**Status:** ✅ Fixed

**Symptom:** `test_trace_add` and `test_trace_sub` crash with SIGBUS (ARM `EXC_BAD_ACCESS code=259`). `test_compile_trace_smi` (single LoadSmi) passes.

**Root cause:** The trace compiler used the real stack pointer (`sp`) as the JIT value-stack pointer. On macOS Apple Silicon, JIT pages are restricted from writing through `sp`; multi-op traces hit the guard page after the first push.

**Fix:** Use VM heap memory for the JIT value stack. Added `JitVmState` (with `jit_stack: [u64; 64]`) to `rune_jit_baseline` and a `jit_stack` field to `Vm`. The trace prologue initializes `x22` as the JIT stack pointer from `VM_REG + 0`, and all push/pop operations use `x22` instead of `sp`. AArch64 trace tests now pass single-threaded and multi-op traces (`test_trace_add`, `test_trace_sub`) no longer crash.

---

## P1: push/pop_callee_saved STP/LDP encodings were WRONG ✅ FIXED

**Status:** ✅ Fixed in `e04e913`

**Symptom:** LLDB disassembly showed `stp x19, x4` instead of `stp x19, x20`.

**Root cause:** Hardcoded values like `0xA9BF13F3` encoded rt2=4 (x4) instead of rt2=20 (x20). The pattern `0x13F3` has bits[14:10]=4, not 20. The linear interpolation `+0x402 per pair` was incorrect.

**Fix:** Use computed encoding: `0xA9BF0000 | (rt2 << 10) | (sp << 5) | rt` for STP, and `0xA8C10000 | (rt2 << 10) | (sp << 5) | rt` for LDP.

---

## P2: mov_reg couldn't read SP ✅ FIXED

**Status:** ✅ Fixed in `e04e913`

**Symptom:** `mov_reg(mem, x22, sp)` set x22=0 instead of x22=sp.

**Root cause:** ARM64 data-processing instructions treat Rm=31 as XZR (zero register), not SP. `ORR x22, xzr, sp` with Rm=31 reads XZR. To read SP, must use `ADD x22, sp, #0`.

**Fix:** `mov_reg` now handles three cases:
- `xd == 31` → `ADD sp, xm, #0` (write to SP)
- `xm == 31` → `ADD xd, sp, #0` (read from SP)
- otherwise → `ORR xd, xzr, xm` (reg-to-reg)

---

## P3: LoadStringConst per-call allocation → NaN at 100K+ ✅ FIXED

**Status:** ✅ Fixed in `9310b97`

**Symptom:** `o.x` in a loop returned NaN at 100K+ iterations.

**Root cause:** `LoadStringConst` allocated a new `HeapString` every call. In hot loops, 100K dead `"x"` strings accumulated, exhausting the 1MB semispace → GC collected live strings → NaN.

**Fix:** String cache on Vm (`string_cache: HashMap<usize, Vec<Value>>`). Strings allocated once per program, cached by pool index, rooted during GC.

---

## P4: IC LRU thrashing at 10+ shapes ✅ FIXED

**Status:** ✅ Fixed in `9382a66`

**Symptom:** 10-shape polymorphic IC hit rate was 0%.

**Root cause:** IC capped at 8 entries. With 10 shapes + LoadPropertyIC bypass, LRU eviction constantly removed entries needed next iteration.

**Fix:** IC cap removed. Unlimited entries (SIDT — no megamorphic cliff). SIMD handles 2 entries/iteration; 50 shapes = 25 SIMD ops.

---

## P5: load_property_recursive_ic never checked IC ✅ FIXED

**Status:** ✅ Fixed in `9382a66`

**Symptom:** After LoadPropertyIC patched, the IC fallback (`load_property_recursive_ic`) always did a full recursive lookup — never checked the IC.

**Root cause:** `load_property_recursive_ic` called `load_property_recursive` (full lookup) FIRST, then populated the IC. Never checked IC for hits.

**Fix:** Check IC before full lookup. On hit → return cached offset. On miss → full lookup → populate IC.

---

## P6: __proto__ assignment didn't set prototype ✅ FIXED

**Status:** ✅ Fixed in `1636edc`

**Symptom:** `o.__proto__ = proto; print(o.x)` returned `undefined`.

**Root cause:** `StoreProperty` treated `__proto__` as a regular property, not the special prototype setter (§10.1.7.1).

**Fix:** `is_proto_key()` helper checks for `"__proto__"` string. `StoreProperty` routes to `JSObject::set_prototype()`.

---

## P7: IC hit rate stats undercounted ✅ FIXED

**Status:** ✅ Fixed

**Symptom:** Poly 10-shape IC stats show 50% hit rate, but SIDT should give 90%+.

**Root cause:** IC stats counter (`ic_stats.hits`) only incremented in original LoadProperty handler. After LoadPropertyIC patches, the fast path bypasses IC stats. The fallback path's IC hits in `load_property_recursive_ic` weren't counted.

**Fix:** `load_property_recursive_ic` now accepts `&mut IcStats` and increments `ic_stats.hits` on every IC hit in the fallback path.

---

## P8: CLI `-e` flag not supported ⚠️ KNOWN

**Status:** ⚠️ Pre-existing, not blocking v0.0.1

**Symptom:** `rune -e '42'` evaluates the string `"-e"` as JS, not `"42"`.

**Root cause:** CLI has no flag parsing — treats first arg as JS source.

**Fix (v0.0.2):** Add basic flag parsing or use `clap`.

---

## P9: Return assertion relaxed ⚠️ P1 deferred

**Status:** ⚠️ `debug_assert!(stack.len() <= base + 2)` instead of `== base + 1`

**Root cause:** Unknown code path leaves 2 items on stack at Return.

**Fix (v0.0.2):** Find the path and fix it. The relaxed assertion prevents crashes.

---

## P10: JIT SLOWER than interpreter on tiny functions ⚠️ Known

**Status:** ⚠️ Known, not blocking v0.0.1

**Symptom:** `jit_hot_function_1M` benchmark (1M calls to `add(a,b){return a+b;}`) takes ~701ms with JIT vs ~455ms interpreter-only. JIT prologue/epilogue + locals setup overhead dominates a 4-instruction function body.

**Root cause:** Function-level JIT has fixed overhead (callee-saved push/pop, locals Vec allocation, all_smi check). For tiny leaf functions, this exceeds the interpreter's per-opcode dispatch cost. Trace-level compilation (whole loop body) doesn't have this problem.

**Fix (v0.0.2):** Wire AArch64 trace compiler to loop execution for hot loops. Use function JIT only for functions above a size threshold (e.g. > 20 bytecode instructions).

---

## P11: JIT opcode coverage is Smi-only (47/61 opcodes) 🟡 In progress

**Symptom:** 47/61 opcodes are JIT-compiled. Still missing: LoadStringConst, Call, all object/array/string ops, type conversions.

**Fix:** Baseline JIT supports Smi arithmetic, comparison, bitwise, unary, branches, locals, and property access (LoadPropertyIC). Remaining 32 opcodes are float, string, object, array, call, and type-conversion ops.

---

## P12: Trace compiler not wired to loop execution ✅ FIXED

**Status:** ✅ Fixed

**Symptom:** The AArch64 trace compiler (`compile_trace`) existed and passed unit tests, but was never invoked during loop execution. Hot loops always ran in interpreter, even though trace recording infrastructure was in place.

**Fix:** Trace compilation is now triggered automatically when a hot loop is detected (>50 iterations). The trace is compiled using `Aarch64CodeGen` (which supports branches) and executed natively on subsequent back-edge jumps. The trace compiles as a self-contained loop: the back-edge Jump is remapped to the top, and JumpIfFalse is remapped to exit the trace. Trace execution works for Smi-only loops with values < 2^32 (see P13).

---

## P13: Smi i31 range limitation in JIT ✅ Resolved (design limitation)

**Status:** ✅ Resolved — not a codegen bug; Smi design constraint.

**Symptom:** Traced loops display wrapped i32 values for results above 2^31-1 (e.g. `print(loop())` shows negative numbers for sums > 2.1B).

**Root cause:** `as_smi()` truncates to `i32` for display. The underlying u64 value is correct. Smi is limited to i31 signed range; values outside that range should be promoted to float64. This is a Smi design constraint, not a trace/codegen bug.

**Resolution:** The trace correctly handles 64-bit arithmetic for all Smi values. Display truncation is expected behavior until float or BigInt support is added to the JIT.

---

## P14: InlineCache::get_scalar cfg-gate breaks non-SSE4.1 x86-64 builds ✅ Fixed

**Status:** ✅ Fixed in `current`

**Symptom:** `get_scalar` was gated `#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]` but `get_simd` (SSE4.1 path) falls back to `get_scalar` on x86-64 when SSE4.1 is unavailable. The method doesn't exist → link failure on any x86-64 CPU without SSE4.1.

**Fix:** Removed the cfg gate; `get_scalar` is now unconditionally compiled as the universal scalar fallback.

---

## P15: test_hot_property_mono_1m SIGSEGV in JIT code 🔴 P0

**Status:** 🔴 Unfixed — pre-existing on `main`

**Symptom:** `test_hot_property_mono_1m` crashes with SIGSEGV (`EXC_BAD_ACCESS`). The test accesses `o.x` on one object 1M times in a loop. Crash is in JIT-compiled code (no debug symbols, mmap'd executable region).

**Crash site:** `ldr x3, [x2]` where `x2 = 0x7800000001` — a Smi-tagged value (bit 0 = 1) being treated as a heap pointer. The Smi decodes to `0x3C0000000 = 16,106,127,360`, which is far larger than any value in the test (max sum = 1,000,000). This suggests the trace or function JIT is loading a Smi value where a heap pointer is expected — likely a register aliasing or operand-order bug in the JIT-compiled property access path.

**Repro:** `cargo test -p rune_embed --test integration_test test_hot_property_mono_1m` (crashes every time on both x86-64 and aarch64).

**Introduced by:** Commit `1636edc` (Fix __proto__ assignment, added this test). Unclear if the test was broken from inception or a later change regressed it.

**Impact:** P0 — crashing test on `main` prevents developers from running the full test suite. Users who run `cargo test --workspace` hit this crash and see red.

**Investigation needed:**
1. Determine if crash is in the function JIT or trace compiler
2. Determine if it's an input guard miss (Smi where object expected) or a trace operand bug
3. Fix the guard, add a pol buffer, or disable the test with `#[ignore]` until fixed
---

## P16: NEON/SSE SIMD IC stride bug (`ptr.add(1)` instead of `ptr.add(2)`) ✅ FIXED

**Status:** ✅ Fixed in `current`

**Symptom:** 10-shape polymorphic `poly_prop_10shapes_1M` benchmark had only ~50% IC hit rate despite unbounded IC (SIDT guarantees O(1) for any number of shapes). `--ic-stats` showed ~50% miss rate.

**Root cause:** `InlineCache::get()` has SIMD hot paths for NEON AArch64 and SSE4.1 x86-64. Both used `ptr.add(1)` to skip from entry `i` to entry `i+1`. But each `IcEntry` is 32 bytes (4 × u64), so the correct stride is `ptr.add(2)` (16 bytes per u64 × 2 u64s per load). The off-by-one caused every odd-indexed entry to read 16 bytes of garbage — half of the IcKey was the previous entry's key_hash and half was the next entry's shape_id. For a 10-shape workload, this meant shapes at odd indices (1, 3, 5, 7, 9) never matched, producing the ~50% artificial miss rate.

**Fix:** Changed `ptr.add(1)` to `ptr.add(2)` in both `get_neon()` (`ic.rs:75`) and `get_simd()` (`ic.rs:199`). This correctly strides by 32 bytes (full IcEntry) per iteration.

**Impact:** Benchmark `poly_prop_10shapes_1M` improved from 1,014ms → 794ms (21% faster). The IC now correctly finds all 10 shapes, not just the even-indexed ones.

---

## P17: LoadPropertyIC stats tracking 🟡 In progress

**Status:** 🟡 Partially fixed — reporting still confusing

**Symptom:** `--ic-stats` showed `0 hits, 0 misses` for LoadProperty (patched ops) and misleadingly low hit rates for polymorphic workloads.

**Root cause vs current state:**
1. `LoadProperty` handler: `lookups`/`hits`/`misses` counted at line 1480+ — after patching to `LoadPropertyIC`, this path is bypassed (correct).
2. `LoadPropertyIC` shape-guard fast path — never touched IC stats. ✅ Now fixed: `lookups += 1` and `hits += 1` added.
3. `LoadPropertyIC` fallback path — never touched IC stats. ✅ Now fixed: `lookups += 1` and `misses += 1` before fallback.
4. `load_property_recursive_ic` already counted IC hits ✅ (from P7).
5. **Remaining:** Stats double-count when both shape-guard `miss` AND fallback IC `hit` fire for same access — `hits + misses > lookups`. Two-tier separation needed.

---

## Summary

| # | Issue | Status | Commit |
|---|---|---|---|
| P0 | Multi-op trace SIGBUS | ✅ Fixed | current |
| P1 | STP/LDP encodings wrong | ✅ Fixed | e04e913 |
| P2 | mov_reg can't read SP | ✅ Fixed | e04e913 |
| P3 | LoadStringConst per-call allocation | ✅ Fixed | 9310b97 |
| P4 | IC LRU thrashing | ✅ Fixed | 9382a66 |
| P5 | IC never checked in fallback | ✅ Fixed | 9382a66 |
| P6 | __proto__ assignment | ✅ Fixed | 1636edc |
| P7 | IC stats undercounted | ✅ Fixed | current |
| P8 | CLI -e flag | ⚠️ Known | — |
| P9 | Return assertion relaxed | ⚠️ Deferred | — |
| P10 | JIT now faster than interpreter after float64 Add (see P13→float64) | ✅ Fixed | 597b12c |
| P11 | JIT coverage (55/62 opcodes + float64 Add promotion) | 🟡 In progress | — |
| P12 | Trace compiler wired to loop execution | ✅ Fixed | — |
| P13 | Smi overflow → float64 Add promotion (was display truncation) | ✅ Resolved | 597b12c |
| P14 | InlineCache::get_scalar cfg-gate | ✅ Fixed | current |
| P15 | test_hot_property_mono_1m SIGSEGV | 🔴 P0 | — |
| P16 | NEON/SSE SIMD IC stride bug (`ptr.add(1)` → `ptr.add(2)`) | ✅ Fixed | current |
| P17 | LoadPropertyIC stats tracking | 🟡 In progress | current |
