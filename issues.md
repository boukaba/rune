# Rune — Known Issues & Investigation Log

## P0: AArch64 trace compiler multi-op SIGBUS

**Status:** 🔴 Blocked — single-op works, multi-op crashes

**Symptom:** `test_trace_add` and `test_trace_sub` crash with SIGBUS (ARM `EXC_BAD_ACCESS code=259`). `test_compile_trace_smi` (single LoadSmi) passes.

**LLDB-verified crash:**
```
frame #0: 0x000000010017403c
-> str x0, [sp]
EXC_BAD_ACCESS (code=259, address=0x1006a2798)
```

The `str x0, [sp]` instruction at the second LoadSmi push crashes because sp points to a protected page. The first `str` at the first LoadSmi works.

**What we tried (all failed):**

| # | Approach | Result |
|---|---|---|
| 1 | Skip `mprotect` on macOS (icache only) | SIGBUS persists |
| 2 | Revert to original `mprotect` (was working for 1-op) | SIGBUS on multi-op |
| 3 | Use heap writable buffer via LOC_REG (x21) | sp not set correctly; `ADD sp, x21, #0` instruction present but ineffective |
| 4 | Change JIT stack from 128→16, 32, 48, 64, 80, 96, 112 | All SIGBUS |
| 5 | Remove JIT stack entirely (sub_imm=0) | SIGBUS |
| 6 | Remove MAP_JIT, use plain mmap | SIGBUS persists |
| 7 | `pthread_jit_write_protect_np(1)` before mprotect | SIGBUS |
| 8 | `RUST_MIN_STACK=16MB` | SIGBUS persists |
| 9 | `--test-threads=1` (main thread, 8MB stack) | SIGBUS persists |
| 10 | Standalone binary (not test harness) | **Hangs** (different symptom) |
| 11 | `#[ignore]` the tests | Works but doesn't fix |
| 12 | Save sp in x22, use x22 for epilogue restore | x22 corrupted (STP/LDP encoding bug — see P1) |

**Root cause hypothesis:** On macOS Apple Silicon, when JIT code executes from a MAP_JIT page, the kernel restricts writes to the stack pointer region. This is a security feature (W^X enforcement at the page level). Single-op traces work because they make fewer writes before the page gets protected. Multi-op traces hit the limit.

**Fix direction for v0.0.2:** Use VM heap memory (not sp) for JIT value stack. Access via `[x19(VM) + JIT_STACK_OFFSET]` instead of `[sp]`.

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

## P7: IC hit rate stats undercounted ⚠️ KNOWN

**Status:** ⚠️ Documented, not fixed

**Symptom:** Poly 10-shape IC stats show 50% hit rate, but SIDT should give 90%+.

**Root cause:** IC stats counter (`ic_stats.hits`) only incremented in original LoadProperty handler. After LoadPropertyIC patches, the fast path bypasses IC stats. The fallback path's IC hits in `load_property_recursive_ic` aren't counted.

**Fix (v0.0.2):** Add `ic_stats.hits` increment in `load_property_recursive_ic` IC hit path.

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

## Summary

| # | Issue | Status | Commit |
|---|---|---|---|
| P0 | Multi-op trace SIGBUS | 🔴 Blocked | — |
| P1 | STP/LDP encodings wrong | ✅ Fixed | e04e913 |
| P2 | mov_reg can't read SP | ✅ Fixed | e04e913 |
| P3 | LoadStringConst per-call allocation | ✅ Fixed | 9310b97 |
| P4 | IC LRU thrashing | ✅ Fixed | 9382a66 |
| P5 | IC never checked in fallback | ✅ Fixed | 9382a66 |
| P6 | __proto__ assignment | ✅ Fixed | 1636edc |
| P7 | IC stats undercounted | ⚠️ Known | — |
| P8 | CLI -e flag | ⚠️ Known | — |
| P9 | Return assertion relaxed | ⚠️ Deferred | — |
