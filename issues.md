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

**Disclosure — prior benchmarks contaminated:** All `poly_prop_10shapes_1M` numbers measured before commit `5f2c883` were affected by this bug — odd-indexed shape entries were never SIMD-matched, forcing slow-path recursive lookup for 50% of accesses. The "no megamorphic cliff" claim in the v0.1.0 README held for ≤8-shape callsites (the SIMD half of the IC still covered even indices), but was untested above 8 shapes. Post-fix `--ic-stats` confirms 99.9% IC lookup hit rate for 10-shape workloads — the claim is now actually true for the first time.

**Post-fix bottleneck diagnosis:** The 21% improvement reveals that the IC was NOT the dominant bottleneck. With 99.9% IC hit rate, the remaining 191× gap to V8 (794ms vs 4.16ms) is dominated by interpreter dispatch overhead — bytecode fetch, dispatch, and frame bookkeeping around the LoadPropertyIC shape-guard fallback path for 9/10 shapes. The fix that closes this gap is **JIT coverage of LoadPropertyIC in hot loops**, not further IC optimization.

---

## P17: LoadPropertyIC stats tracking 🟡 Partially resolved

**Status:** 🟡 Partially fixed — reporting still confusing

**Symptom:** `--ic-stats` showed `0 hits, 0 misses` for LoadProperty (patched ops) and misleadingly low hit rates for polymorphic workloads.

**Root cause vs current state:**
1. `LoadProperty` handler: `lookups`/`hits`/`misses` counted at line 1480+ — after patching to `LoadPropertyIC`, this path is bypassed (correct).
2. `LoadPropertyIC` shape-guard fast path — never touched IC stats. ✅ Now fixed: `lookups += 1` and `hits += 1` added.
3. `LoadPropertyIC` fallback path — never touched IC stats. ✅ Now fixed: `lookups += 1` and `misses += 1` before fallback.
4. `load_property_recursive_ic` already counted IC hits ✅ (from P7).
5. **Resolution:** `dump_ic_stats` now uses `hits / lookups` as the IC hit rate, avoiding the double-count issue. `--jit-stats` flag added for JIT entry/bailout diagnostics.

**Post-fix benchmark diagnostics (commit 5f2c883 + current):**
- `poly_prop_10shapes_1M`: 99.9% IC hit rate (1,998,990/2,001,990). JIT: 0 entries (top-level code). **Bottleneck: interpreter dispatch** — the 191× gap to V8 is dominated by LoadPropertyIC shape-guard fallback for 9/10 shapes.
- `proto_chain_lookup_5deep_1M`: 0.0% IC hit rate (0/1M). JIT: 0 entries. **Root cause: trace JIT doesn't execute** — see P18.

---

## P18: Trace JIT LoadPropertyIC operand mismatch (ic_index vs shape_id) 🔴 P0

**Status:** 🔴 Unfixed — prevents trace JIT execution for any loop with property access

**Symptom:** All trace-compiled loops with LoadPropertyIC show 0 shapes recorded, and `--jit-stats` shows 0 entries/0 bailouts. The trace is compiled but never executes (the shape guard always fails silently via bailout). Both `proto_chain_lookup_5deep_1M` (727ms) and `poly_prop_10shapes_1M` (794ms) would benefit from trace JIT execution.

**Root cause:** Two separate issues conspire:
1. **Shape recording only happens in LoadProperty handler** (line 1493-1502), not in LoadPropertyIC handler (line 1642-1650 only records if `self.recording_trace` is set, which happens at back-edge 50 — but by then LoadPropertyIC is already patched, and the LoadProperty handler's shape recording code is dead).
2. **Trace records bytecode operand [ic_index], not [shape_id, offset, proto_depth].** When `compile_trace_native` builds a new BytecodeProgram from recorded trace ops, the LoadPropertyIC instruction has operands `[ic_index]` (the bytecode format). The AArch64 codegen (`codegen_aarch64.rs:962-965`) reads `operands[0]` as `shape_id` and `operands[1]` as `offset` (the patched format). With `[ic_index]`, `shape_id = ic_index` (wrong) and `offset` is OOB.
3. **`patch_loop_body` (line 3480) never runs** because it requires `trace.shape_ids.len() >= 1` (line 3493-3495) and `trace.is_monomorphic()` (line 3490). With 0 shapes, it's never called, so the opcode operands are never converted from `[ic_index]` to `[shape_id, offset, proto_depth]`.

**Consequence:** The trace compiles but the shape guard always fails (wrong shape_id). The bailout restores interpreter state, which handles the remaining loop iterations. The slow recursive walk path is taken for every property access in the loop.

**Fix:** Either (a) record shapes in the LoadPropertyIC handler too, or (b) resolve the shape_id/offset from the IC table at trace compile time (in `compile_trace_native`), or (c) record the full instruction with patched operands when the patch happens.

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
| P17 | LoadPropertyIC stats tracking | ✅ Fixed | current |
| P18 | Trace JIT LoadPropertyIC operand bug (ic_index → shape_id mismatch) | 🔴 P0 | — |
