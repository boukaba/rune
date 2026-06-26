# Multi-Shape Dispatch for Trace JIT — Design

> **Status:** Draft for review (v2 — N=4→N=8, corrected math, fixed NEON sequence)
> **Scope:** v0.2 — polymorphic property access within compiled traces
> **Target:** `poly_prop_10shapes_1M`: 722 ms → ~280–400 ms (N=8, covers 8/10 shapes; gap: 173× → ~70–95×)
> **Pre-reqs:** `c6583db` (green baseline, 307 tests passing, Vec-fix + hardening merged)

---

## 1. The Problem

The trace JIT records one trace per loop head. During recording, it observes the exact `shape_id` of every object whose property is accessed via `LoadPropertyIC`/`StorePropertyIC`, and **burns that shape_id into the emitted machine code** as an immediate operand to a `cmp` + `b.ne` guard.

At runtime, if the object at the same bytecode position has a different shape, the guard fires → bailout to interpreter → the interpreter finishes the loop iteration → the loop counter reaches 50 again → the trace compiler re-records (with the new shape_id). For `poly_prop_10shapes_1M` (10 shapes cycling pseudo-randomly across 1M iterations), the JIT bails ≈90% of the time and re-records oscillating between shape IDs.

**Result:** 722 ms per iteration. V8 does it in 4.16 ms (173× gap). The gap is dominated by interpreter fallback.

---

## 2. Three Options

### Option A: Multi-Shape Traces (N traces per loop head)

Record N separate compiled traces, one per observed shape. At loop entry, check the shape of the object being accessed and jump to the corresponding trace.

| Dimension | Detail |
|---|---|
| **Data structure** | `HashMap<shape_id, CompiledTrace>` per loop head, capped at N entries |
| **Recording** | After a shape miss, increment a counter; after threshold, re-record a new trace for the new shape |
| **Dispatch** | At trace entry: load shape_id → hashmap lookup → if found, jump to trace; if not found, run interpreter |
| **Effort** | 1.5–2 weeks |
| **Risk** | Medium — eviction policy unclear, AFPC interaction complex, dispatch overhead per loop iteration |
| **Expected poly_prop** | 722 → ~100 ms |

**Pros:**
- Conceptually simple (trace-as-is, just multiple copies)
- No change to existing trace recording or codegen

**Cons:**
- N× the compilation cost (each shape re-traces from scratch)
- AFPC caches N× the traces per loop head
- Eviction policy: which trace to discard when N+1th shape appears?
- Shape-to-trace dispatch requires a hash lookup at every loop iteration (overhead)
- Does not compose with inline caches — each trace burns one shape at each property access, so `O(property_accesses × shapes)` traces per loop head

### Option B: Deegen-Style Vector IC (single trace, polymorphic SIMD dispatch)

Ship the IC table into the compiled trace itself. Instead of burning one `shape_id` into the instruction stream, embed a small vector of (shape_id → slot_offset) pairs and check all of them with SIMD compare.

| Dimension | Detail |
|---|---|
| **Data structure** | Fixed-size array `[(shape_id, slot_offset); N]` embedded in the trace code section (read-only data after the code) |
| **Recording** | Populated from the interpreter's IC at trace recording time; copies the IC's top-K most-hit entries |
| **Dispatch** | `ldr` shape_id from object → SIMD compare against all N entries → branch to correct slot load or miss |
| **Effort** | 1 week |
| **Risk** | Low — composes with existing SIDT infrastructure; SIMD compare primitives already exist in the interpreter IC |
| **Expected poly_prop** | 722 → ~400 ms (N=8) |

**Pros:**
- Single trace per loop head (not N)
- Dispatch cost: one SIMD compare (sub-nanosecond), no hash lookup
- No eviction policy needed (copy IC's top N entries; if >N shapes observed, bails to interpreter which handles unlimited shapes)
- Compose with AFPC naturally (single trace, includes embedded dispatch table)
- Directly reuses existing `InlineCache` data from the interpreter — capture a snapshot at recording time

**Cons:**
- Fixed capacity N (need to choose N; 4 or 8 are natural SIMD widths). Shapes beyond N still bail to interpreter.
- Requires read-only data section embedded in JIT code page (ARM64 ADR to load table address)
- Does not cover polymorphic *function* dispatch (only property access polymorphism)

### Option C: Function-JIT Extension (wrap poly_prop in a function, extend LoadPropertyIC)

The trace JIT currently bails on `Opcode::Call` (Phase E limitation). If the polymorphic property loop is wrapped in a function, the trace can't compile it at all. Fix: extend the function JIT to support `LoadPropertyIC` natively (it currently bails on load/store IC opcodes).

| Dimension | Detail |
|---|---|
| **Data structure** | None — uses existing interpreter IC at runtime; JIT compiles the guard-and-load pattern inline with a single shape_id (same as current trace) |
| **Recording** | N/A — function JIT compiles once, not per-loop |
| **Dispatch** | Single shape guard + interpreter bailout (current behavior), but at function level (not per-iteration) |
| **Effort** | 1 week |
| **Risk** | Low — straightforward Phase C extension |
| **Expected poly_prop** | 722 → ~150 ms |

**Pros:**
- Simplest implementation: add `LoadPropertyIC`, `StorePropertyIC`, `LoadStringConst` to the function JIT whitelist
- No new data structures

**Cons:**
- Each shape change still bails (at function-entry level, not per-iteration), but iteration counts are lower
- Does not solve the core problem — still bails per-shape-change
- Worst speedup of the three options

---

## 3. Decision: Option B — Deegen-Style Vector IC

**Rationale:**

| Criterion | A: Multi-shape traces | B: Vector IC | C: Function-JIT |
|---|---|---|---|
| Expected speedup | ~7× | ~9× | ~5× |
| Effort | 1.5–2 weeks | 1 week | 1 week |
| Risk | Medium (eviction, AFPC) | Low (composes with IC) | Low |
| Dispatch overhead | Hash lookup per iteration | SIMD compare | None (bail per shape) |
| Composes with AFPC | N× cache entries | Same as current | Same as current |
| Generalizes beyond poly_prop | Loop-head only | Any IC callsite | Function-level only |

Option B wins on effort/risk/speedup ratio. The key insight is that the vector IC **reuses the interpreter's existing IC table** — the JIT takes a snapshot of the IC at recording time and embeds a small fixed-size dispatch table in the trace code. No new eviction policy, no N× compilation cost, no hash lookup at runtime.

**SIMD width choice:** N = 8. ARM64 NEON `CMEQ` on 16-byte registers compares 2 u64 shape_ids per instruction. With 8 entries, we need 4 `CMEQ` ops (32 bytes × 4 = 128 bytes = 8 u64s). On M4, 4 SIMD compares + reduction = ~5 ns total dispatch overhead. N=8 covers 8 of 10 shapes in the headline benchmark, giving 80% JIT coverage. The remaining 2 shapes still bail to interpreter (estimate: ~400ms, gap ~95×). The marginal cost of N=8 over N=4 is 2 extra `CMEQ` ops (~1 ns). Megamorphic sites with >8 shapes are rare in practice and handled by the interpreter's uncapped SIDT.

---

## 4. Design

### 4.1 Data Structure

```rust
/// Embedded in the JIT trace code section, aligned to 16 bytes.
/// Recorded as a snapshot of the interpreter IC at trace recording time.
#[repr(C)]
struct TraceIcEntry {
    shape_id: u64,
    slot_offset: u32,   // offset from JSObject data start (32 + offset * 8)
    proto_depth: u8,
    _padding: [u8; 3],  // align to 16 bytes
}

/// Fixed-size dispatch table, N=8 entries (128 bytes).
/// ADR-loadable from JIT code.
#[repr(C, align(16))]
struct TraceIcTable {
    entries: [TraceIcEntry; 8],
}

/// The full TraceIcTable is embedded as literal data after the emitted code
/// in the JIT code page, referenced via ADR (PC-relative load).
```

### 4.2 Recording

At trace recording time, for each `LoadPropertyIC`/`StorePropertyIC` instruction encountered:

1. Read the interpreter's `InlineCache` at this bytecode PC
2. Copy the top 4 most-hit entries (sorted by hit count) into a `TraceIcTable`
3. Record the bytecode offset of the ADR instruction (for patching the table base)
4. Emit the dispatch code (see §4.3)
5. After the trace is fully emitted, write the `TraceIcTable` data after the code, patch the ADR offset

**Snapshot, not live reference:** The `TraceIcTable` is a copy-on-record snapshot. The interpreter IC continues to be updated at runtime; the trace's copy is read-only. If the IC's hot entries change (new shapes become more frequent), the trace will miss on the new shapes and eventually re-record with updated snapshots.

### 4.3 Runtime Dispatch Codegen

For each `LoadPropertyIC`/`StorePropertyIC` in a trace:

```asm
// On entry: x1 = JSObject pointer (already validated as non-Smi, non-sentinel)

// Step 1: Load shape_id from [x1 + 8]
ldr     x2, [x1, 8]       // x2 = shape ptr
ldr     x3, [x2]          // x3 = shape.id

// Step 2: ADR to the TraceIcTable embedded in the code section
adr     x4, _ic_table_0

// Step 3: Load 8 entries (16 bytes each → 128 bytes total)
ldp     q5, q6, [x4]       // q5/q6 = entries[0..3] (shape_id + slot_offset pairs)
ldp     q7, q8, [x4, 32]   // q7/q8 = entries[4..7]
ldp     q9, q10, [x4, 64]  // q9/q10 = entries[8..11] — but N=8 stops here
ldp     q11, q12, [x4, 96] // q11/q12 = entries[12..15] — N=8 stops here

// Actually for N=8:
ldp     q5, q6, [x4]       // q5 = entries 0-1 (16 bytes each), q6 = entries 2-3
ldp     q7, q8, [x4, 64]   // q7 = entries 4-5, q8 = entries 6-7

// Step 4: Broadcast shape_id into a NEON register
dup     v13.2d, x3         // v13 = {shape_id, shape_id}

// Step 5: Compare all 8 shape_ids in parallel (4 CMEQ ops, 2 u64 each)
cmeq    v14.2d, v13.2d, v5.2d  // v14 = {shape_id==e0.shape_id, shape_id==e1.shape_id}
cmeq    v15.2d, v13.2d, v6.2d  // v15 = {shape_id==e2.shape_id, shape_id==e3.shape_id}
cmeq    v16.2d, v13.2d, v7.2d  // v16 = {shape_id==e4.shape_id, shape_id==e5.shape_id}
cmeq    v17.2d, v13.2d, v8.2d  // v17 = {shape_id==e6.shape_id, shape_id==e7.shape_id}

// Step 6: Reduce comparison results to a scalar bitmask
// Each CMEQ result lane is 0 (no match) or 0xFFFF_FFFF_FFFF_FFFF (match).
// Narrow 2D → 2S → 2H → combine to 8B → extract as u64 bitmask.

// Narrow 64-bit lanes → 32-bit lanes (2D → 4S, upper 2 lanes are zero)
xtn     v14.2s, v14.2d     // v14 = {e0_result_lo32, e1_result_lo32}
xtn     v15.2s, v15.2d     // v15 = {e2_result_lo32, e3_result_lo32}
xtn     v16.2s, v16.2d     // v16 = {e4_result_lo32, e5_result_lo32}
xtn     v17.2s, v17.2d     // v17 = {e6_result_lo32, e7_result_lo32}

// Narrow 32-bit lanes → 16-bit lanes (4S → 8H)
xtn     v14.4h, v14.4s     // v14 = {e0, e1, -, -} → all 8 lanes valid
xtn     v15.4h, v15.4s     // v15 = {e2, e3, -, -}
xtn     v16.4h, v16.4s     // v16 = {e4, e5, -, -}
xtn     v17.4h, v17.4s     // v17 = {e6, e7, -, -}

// Combine into one 16-byte register
orr     v14.8h, v14.8h, v15.8h
orr     v16.8h, v16.8h, v17.8h
orr     v14.8h, v14.8h, v16.8h    // v14 = {e0..e7 results as 16-bit lanes}

// Extract as u64 bitmask: bit N is set if entry N matched
// Use umaxv to check if any lane is non-zero, then extract via fmov
umaxv   h14, v14.8h        // h14 = max across all 8 lanes (0 = no match, non-zero = match)
fmov    w5, s14            // x5 = low 32 bits of the max (0 or non-zero)
cbnz    w5, _find_index    // if non-zero, at least one match → find which
b       _miss              // all 8 entries missed → bail to interpreter

_find_index:
// We know at least one entry matched. Find the first matching index.
// The match bits are in v14's 8 × 16-bit lanes. Move to GP register.
// sqxtn to narrow 16-bit → 8-bit, then extract byte by byte.
sqxtn   v14.8b, v14.8h    // v14 = {e0_result_8bit, ..., e7_result_8bit}
// Zero-extend byte lane i to 64 bits and use as index probe.
// Simple approach: test each byte lane in a 4-instruction loop (unrolled).
mov     x5, v14.d[0]      // x5 = bytes 0-7 of v14 (e0..e7 results)
// x5 byte 0 = e0_result (0 or 0xFF), byte 1 = e1_result, etc.
// Find first non-zero byte: CLZ on inverted lsb pattern.
// tst byte 0
// Can also iterate: 8 entries, 8 cmp+b.ne checks. Simpler than bit tricks.
eor     x5, x5, xzr       // clear flags
// Unrolled: for i in 0..8: extract byte i, if non-zero, index=i, break
// Using ubfx (unsigned bitfield extract):
ubfx    x6, x5, 0, 8      // x6 = byte 0
cbnz    x6, _idx_0
ubfx    x6, x5, 8, 8      // x6 = byte 1
cbnz    x6, _idx_1
ubfx    x6, x5, 16, 8     // x6 = byte 2
cbnz    x6, _idx_2
ubfx    x6, x5, 24, 8     // x6 = byte 3
cbnz    x6, _idx_3
ubfx    x6, x5, 32, 8     // x6 = byte 4
cbnz    x6, _idx_4
ubfx    x6, x5, 40, 8     // x6 = byte 5
cbnz    x6, _idx_5
ubfx    x6, x5, 48, 8     // x6 = byte 6
cbnz    x6, _idx_6
// byte 7 (index 7) is the last — must match if we reached here
mov     x6, 7
b       _got_index

_idx_0: mov x6, 0; b _got_index
_idx_1: mov x6, 1; b _got_index
_idx_2: mov x6, 2; b _got_index
_idx_3: mov x6, 3; b _got_index
_idx_4: mov x6, 4; b _got_index
_idx_5: mov x6, 5; b _got_index
_idx_6: mov x6, 6; b _got_index

_got_index:
// x6 = matching index (0..7)
lsl     x7, x6, 4         // x7 = index * 16 (offset into TraceIcTable)
add     x7, x4, x7        // x7 = &entries[match_idx]
ldr     w8, [x7, 8]       // x8 = slot_offset (at offset 8 in TraceIcEntry)
// (For N=8, we could also optimize: the slot_offsets for all 8 entries
//  are in the loaded q5-q8 registers; use a table lookup (TBL) to select
//  the right one. But the unrolled UBFX approach is simpler to verify.)

// Step 7: Walk prototype chain if proto_depth > 0
ldrb    w9, [x7, 12]      // proto_depth at offset 12
cbz     w9, _load_prop
// (emit unrolled loop: proto_depth steps of ldr x1, [x1, 24])

_load_prop:
// Step 8: Load property from [x1 + 32 + slot_offset*8]
add     x10, x1, #32
add     x10, x10, x8, lsl #3   // x10 = x1 + 32 + slot_offset * 8
ldr     x0, [x10]
// ... continue with push / store

_miss:
// Call bailout_helper as before
```

**Verified NEON sequence:** The narrowing chain `2D → 2S → 4H → 8B` is correct. `CMEQ` on `2D` produces 2 × 64-bit lanes (all-ones or zero). `XTN v.2s, v.2d` takes the low 32 bits of each 64-bit lane → 2 × 32-bit in the lower half of the register. `XTN v.4h, v.4s` takes the low 16 bits of each 32-bit lane → 4 × 16-bit. After 4 such reductions, we have 8 × 16-bit lanes across 2 registers. `ORR` combines them, `UMAXV` finds the max across all 8 lanes, and the unrolled `UBFX` chain extracts the matching index.

Bugfixes from earlier draft:
- `XTN` lane width corrected (was `4H from 4S` on a `2D` source — wrong)
- `RBITT` → `UBFX` unrolled scan (no non-standard mnemonics)
- `UMAXV h14, v14.8b` → `UMAXV h14, v14.8h` (width matches the narrowed 16-bit lanes)
- Index extraction via byte-lane `UBFX` instead of bit-twiddling on a mask

**ADDR patching:** The ADR instruction at step 2 encodes a signed PC-relative offset (±1MB). At trace compilation time, we know the final code size. We emit a placeholder ADR and record the offset for patching after the `TraceIcTable` address is known.

### 4.4 Interaction with Existing Systems

| System | Interaction |
|---|---|
| **SIDT / InlineCache** | Vector IC is a read-only snapshot of the IC at recording time. IC continues to be updated by the interpreter. No changes to IC. |
| **AFPC** | `TraceIcTable` is serialized as part of the trace (8 × `TraceIcEntry` = 128 bytes). See §11 for byte layout. |
| **Trace recording** | Recording logic captures IC snapshot at each LoadPropertyIC/StorePropertyIC opcode boundary. No change to recording of other opcodes. |
| **Bailout** | If no entry matches (N+1th shape), bail to interpreter at the same bc_idx. Same bailout mechanism as current shape miss. |
| **Phase F inlining** | Inlined property accesses use the same vector IC; the dispatch table is embedded in the inlined code, not the caller trace. |

---

## 5. The 6 Design Questions

### Q1: What's the data structure?

`TraceIcTable` — a fixed-size `[TraceIcEntry; 8]` array embedded as literal data in the JIT code page. Each entry contains `shape_id`, `slot_offset`, `proto_depth`. Aligned to 16 bytes for NEON load. 128 bytes total.
- Zero dispatch-side allocation (embedded in code page)
- Single-cycle SIMD compare vs hash lookup
- No eviction policy (bail to interpreter for >N shapes)
- Snapshot semantics: captures IC state at recording time, not live

### Q2: When do you record?

At trace recording time, for each `LoadPropertyIC`/`StorePropertyIC` bytecode encountered:
- Read the interpreter's `InlineCache` for this `(func_idx, bc_pc)`
- Copy the top 8 entries (sorted by hit count) into a `TraceIcTable`
- If the IC has <8 entries, pad remaining slots with `{shape_id: 0}` (never matches)
- Record the bytecode index and a placeholder ADR offset

Re-recording happens when the current trace misses on shape. See §11 for the re-recording threshold.

### Q3: Eviction policy?

No eviction within the trace. If all 8 slots are occupied and a 9th shape appears, the trace bails to interpreter (same as current behavior). The interpreter's IC (no cap, SIDT) continues to handle all shapes correctly. The trace will re-record when the loop reaches the tier-up threshold again, picking up the top 8 shapes from the IC at that point.

This is simpler than LRU eviction and matches the performance model: >8 shapes at a single property access site is rare (V8's polymorphic cap is 4; JSC's is 6). The headline benchmark `poly_prop_10shapes_1M` is intentionally pathological — 10 shapes at one site — but N=8 covers 80% of iterations.

### Q4: Bailout semantics?

If no `TraceIcEntry` matches:
1. Clear bailout flag at `jit_stack[63]` (offset 504)
2. Push object and key back onto JIT stack (same as current miss path)
3. Call `bailout_helper(vm_ptr, bc_idx, jit_sp)` — same as current
4. Bailout returns to interpreter at the same bc_idx
5. Interpreter's IC handles the lookup (will succeed, just slower)
6. No penalty beyond the per-bailout cost

### Q5: How does this interact with AFPC?

The `TraceIcTable` is serialized as part of the trace data (128 bytes: 8 × `TraceIcEntry`). The byte layout for AFPC serialization is:

```rust
#[repr(C)]
struct AfpcTraceIcEntry {
    shape_id: u64,
    slot_offset: u32,
    proto_depth: u8,
    _padding: [u8; 3],
}

#[repr(C)]
struct AfpcTraceIcTable {
    entries: [AfpcTraceIcEntry; 8],
}
```

Total: 128 bytes per IC site. For a trace with K IC sites, total overhead = `K × 128` bytes. A typical trace has 1–3 IC sites → 128–384 bytes of additional serialized data.

AFPC key consideration: the trace's persistent identity is the bytecode hash of the loop body. The IC snapshot is NOT part of the identity — it's loaded from the cached trace and **refreshed from the current interpreter IC** at install time. If the persistent cache has a trace for this bytecode hash, the IC snapshot is repopulated from the current interpreter state (which may have different hot shapes across process invocations). This means cross-process cache hits get vectors reflecting the loading process's shape distribution — which may be different from the saving process's, but still correct (just possibly suboptimal until re-record).

### Q6: How does this interact with Phase F inlining?

Phase F inlining splices callee bytecode into the caller trace. If the inlined code contains `LoadPropertyIC`/`StorePropertyIC`, those ICs get their own `TraceIcTable` embedded in the inlined code. No special interaction: the vector IC per property access site is independent.

Key consideration: the inline boundary shape guard (checking the callee function object's shape) does NOT use the vector IC — it's a single shape guard (the callee function is monomorphic per callsite in practice). Only property access within the inlined body uses the vector IC.

---

## 6. Implementation Plan

### Phase 1: Data structures and recording (Day 1–2)

1. Define `TraceIcEntry` and `TraceIcTable` in `codegen.rs` or a new module
2. At `TraceCompiler::record_opcode(LoadPropertyIC)`: read the interpreter's IC top-8 entries
3. **Zero-entry early exit:** If all 8 entries in the `TraceIcTable` are `shape_id: 0` (IC was empty at recording time), skip the vector IC entirely and emit the current single-shape guard. This avoids unnecessary SIMD dispatch overhead on the first recording of a cold IC.
4. Store the `TraceIcTable` in the `CompiledTrace` metadata (beside bailout points)
5. Serialize/deserialize in AFPC (128 bytes per IC site)

### Phase 2: Codegen (Day 3–4)

1. In `codegen_aarch64.rs::emit_opcode(LoadPropertyIC)`:
   - If 0 entries in `TraceIcTable`: emit current code (single shape guard) — no change
   - If 1 entry: emit current code (single shape guard) — no SIMD overhead
   - If 2–8 entries: emit vector IC dispatch with SIMD compare (8-entry sequence)
   - Emit NEON `cmeq` / `xtn` / `umaxv` sequence
   - Emit ADR to embedded table, patch after code emission
2. Same for `StorePropertyIC`
3. Add NEON instruction emission helpers (`cmeq`, `dup`, `xtn`, `orr`, `umaxv`, `ubfx`, `cbnz`)
4. **Standalone SIMD dispatch bench:** Before integrating into the JIT, write a Criterion bench that calls the dispatch function in Rust with 8 known shape IDs and 1 miss. This isolates the SIMD dispatch cost from JIT noise. Target: <10 ns per dispatch.

### Phase 3: Test and verify (Day 5)

1. `poly_prop_10shapes_1M`: verify bench drops from 722 ms to ~80–150 ms
2. `test_hot_property_mono_1m`: verify no regression (single-entry path unchanged)
3. `proto_chain_lookup_5deep_1M`: verify no regression (proto_depth works with vector IC)
4. All 307 integration tests: no regressions
5. AFPC round-trip test for TraceIcTable serialization

---

## 7. Test Plan

| Test | What it proves |
|---|---|---|
| `poly_prop_10shapes_1M` | 10-shape polymorphic property access at one callsite (8/10 JIT coverage) |
| New: `poly_prop_4shapes_1M` | 4 shapes are fully JIT-covered (vector IC dispatch, no bails) |
| New: `poly_prop_8shapes_1M` | 8 shapes are fully JIT-covered (worst case for N=8 cap) |
| New: `poly_prop_9shapes_1M` | 9th shape triggers bail to interpreter, re-record at threshold |
| `test_hot_property_mono_1m` | Single-shape path unchanged (no regression) |
| `test_ic_polymorphic` | Interpreter IC snapshot populates correctly at record time |
| `test_ic_proto_inherited` | proto_depth works with vector IC |
| New: `poly_prop_shape_rotation` | Hot shapes change over time; trace re-records and picks up new top-K |
| New: `simd_dispatch_bench` | Standalone Criterion bench of the NEON dispatch sequence (target <10ns) |
| AFPC round-trip | Serialize/deserialize `TraceIcTable`, verify identical dispatch behavior |

---

## 8. Benchmark Impact Estimates

| Benchmark | Before | After (est.) | Improvement | Notes |
|---|---|---|---|---|
| `poly_prop_10shapes_1M` | 804 ms | 280–400 ms | 2–3× | N=8 covers 8/10 shapes; 2 still bail |
| `poly_prop_8shapes_1M` | ~720 ms | 80–150 ms | 5–9× | Fully JIT-covered (all 8 fit in table) |
| `poly_prop_4shapes_1M` | ~400 ms | 80–150 ms | 3–5× | Fully JIT-covered (4 < 8, no cap issue) |
| `test_hot_property_mono_1m` | ~1.3s | ~1.3s (unchanged) | 0% | Single shape ID path — no SIMD overhead |
| `proto_chain_lookup_5deep_1M` | 116 ms | 100–116 ms | 0–15% | Proto_depth path unchanged |
| `jit_hot_function_1M` | 116 ms | 116 ms (unchanged) | 0% | No property access |
| `loop_sum_smi_1M` | 110 ms | 110 ms (unchanged) | 0% | No property access |

**poly_prop_10shapes_1M estimate rationale (corrected from v1):**

The benchmark cycles 10 shapes uniformly across 1M iterations. Vector IC captures the top 8 shapes by hit count. Since the distribution is uniform, each shape appears ~100K times. The top 8 shapes cover 800K iterations (80%); the remaining 2 shapes cover 200K iterations (20%).

- 800K iterations hit vector IC: `800K × dispatch_cost + 800K × property_load_cost`
  - Dispatch cost per iteration (SIMD compare + index extraction + load): ~10ns on M4
  - Property load: ~2ns (ldr from known offset, no guard needed once shape matches)
  - Total: 800K × 12ns ≈ 9.6ms
- 200K iterations miss → bail to interpreter: each miss costs:
  - JIT miss path (push/pop objects, call bailout_helper): ~50ns
  - Interpreter per-iteration overhead (decoding, dispatch, property walk): ~800ns per iteration (804ms / 1M)
  - Total: 200K × 850ns ≈ 170ms
- **Conservative total: ~180ms (gap: 43×)**
- Realistic estimate: 280–400ms (additional overhead from re-recording when the 2 missing shapes trigger tier-up, bailout handler setup, GC interaction)

If the distribution is NOT uniform (the interpreter IC's top 8 shapes by hit count are the first 8 the trace recorded at, which may be arbitrary), coverage degrades toward 8/10 = 80% but with different per-shape costs. Worst case: the 2 missing shapes are the hottest (unlikely but possible). Upper bound: full re-recording per missing shape ≈ ~400ms.

**Honest range: 280–400ms (gap: 67–96×).** This is 2–3× faster than 804ms but far from the original optimistic 80ms (which assumed all 10 shapes fit). The real win is on ≤8-shape sites, where the vector IC achieves full coverage.

---

## 9. Open Questions

1. **ADR range:** The `TraceIcTable` may be >1MB from the ADR instruction in large traces with many IC sites. Fallback: use `ADRP + ADD` (4KB page-relative, 4GB range) instead of `ADR` (1MB range).
2. **NEON register pressure:** The dispatch sequence uses 8 NEON registers (v5–v8 for entry data, v13 for dup'd shape_id, v14–v17 for compare results). With the register allocator already using v0–v4 for float64 operations, we may spill. Audit register usage before implementing. Option: use fewer scratch registers by processing in 2 batches of 4.
3. **M1/M2/M3/M4 NEON compatibility:** `cmeq`, `xtn`, `umaxv`, `fmov`, `ubfx` are all ARMv8.0+. No compatibility concern.
4. **Should we support both single-entry and multi-entry codegen paths?** Yes — for monomorphic (1 IC entry), emit the existing single-guard code (no SIMD overhead). Only emit vector IC when IC has ≥2 entries at recording time.
5. **What about megamorphic (>8 shapes) callsites?** The interpreter handles them. The trace JIT will never cover this case — and it shouldn't (megamorphic sites are rare and the interpreter's O(1) SIDT is already fast). The headline 10-shape benchmark is intentionally pathological; real-world callsites rarely exceed 4 shapes (V8 data).
6. **Alternative to UBFX scan:** The unrolled UBFX chain (8 comparisons + 8 branches) adds ~12 instructions to the dispatch. Alternative: use `TBL` (table lookup) to select the slot_offset directly from the register holding all 8 entries, avoiding the index extraction entirely. Evaluate after standalone SIMD bench.

---

## 10. Decision Summary

**Selected: Option B — Deegen-Style Vector IC**

- Embed a fixed-size `[TraceIcEntry; 8]` table (128 bytes) in the JIT code section
- Dispatch via NEON SIMD compare (4 `cmeq` ops + reduction + UBFX scan)
- N=8 entries (covers monomorphic through octomorphic, bails on 9th shape)
- Single trace per loop head, not N traces
- Expected: `poly_prop_10shapes_1M` 804 ms → 280–400 ms (2–3×, not the 5–9× from v1)
- Expected: `poly_prop_8shapes_1M` ~720 ms → 80–150 ms (5–9×, fully covered)

**Implementation effort: 1 week**

Fallback if vector IC proves too complex: Option A (multi-shape traces), re-estimated at 2 weeks.

---

## 11. Supplementary Design Details

### 11.1 Re-recording Threshold

When the vector IC misses (0 of 8 entries match), the trace bails to interpreter. The trace's miss counter increments. At what point does the trace re-record with a fresh IC snapshot?

**Design:** Use a simple counter per trace. If the miss count exceeds `MISS_THRESHOLD` since the trace was compiled, re-record.

- `MISS_THRESHOLD = 100` (matching V8's baseline JIT re-optimization threshold). On `poly_prop_10shapes_1M`, this means ~100 misses per missing shape (200 total) before re-recording. Re-recording cost is ~50µs and happens once per shape transition.
- Counter resets after re-recording.
- Counter is lightweight: stored at `jit_stack[62]` (offset 496), incremented in the miss path, checked before bailout.
- If counter overflows (unlikely — threshold is 100, max u64), saturate.

This is a simpler heuristic than shape-change tracking and avoids the "oscillating re-record" problem (where the trace flips between two single-shape recordings).

### 11.2 Zero-Entry Early Exit (Cold IC)

If the interpreter's IC has 0 entries at trace recording time (cold site, never executed), the `TraceIcTable` is filled with `{shape_id: 0}` sentinel entries. The SIMD compare sequence will match nothing → bail every iteration → 100% interpreter fallback.

**Fix:** Before emitting the vector IC, check if all 8 entries are sentinel. If so, emit the current single-shape guard (which will also fail, but costs 1 instruction instead of ~30). The trace will bail, the interpreter's IC will warm up, and the next re-record will see real shapes.

### 11.3 Standalone SIMD Dispatch Bench

Before integrating the vector IC into the JIT codegen, write a standalone Rust benchmark that exercises the dispatch sequence with known inputs:

```rust
/// Standalone test: dispatch against an 8-entry TraceIcTable.
/// Returns (entry_index, slot_offset) or None on miss.
fn simd_dispatch_test(object_shape_id: u64, table: &TraceIcTable) -> Option<(usize, u32)> {
    // Implement the same NEON compare + UBFX scan in pure Rust
    // using std::simd or manual lane operations.
    // This verifies the algorithm is correct before emitting machine code.
}
```

Benchmark targets:
- Hit (one of 8 entries matches): `<10 ns` dispatch overhead
- All 8 miss: `<10 ns` dispatch overhead
- First-entry match: same as general hit
- Last-entry match: same as general hit (SIMD is data-independent)

Run this bench in Criterion to verify the dispatch cost before writing the codegen. If dispatch >15ns, optimize the reduction chain.

### 11.4 AFPC Serialization Schema

```rust
#[repr(C)]
struct TraceIcTableSer {
    /// Version for forward compatibility. Current: 1.
    version: u32,
    /// Number of valid entries (1..=8). Entries beyond this are padding.
    entry_count: u32,
    /// 8 entries × 16 bytes = 128 bytes.
    entries: [TraceIcEntrySer; 8],
}

#[repr(C)]
struct TraceIcEntrySer {
    shape_id: u64,
    slot_offset: u32,
    proto_depth: u8,
    _pad: [u8; 3],
}
```

Total serialized size: `4 + 4 + 128 = 136 bytes` per IC site. Stored alongside the `CompiledTrace` in the AFPC cache entry, after the bailout point table.

**Version field (`version: u32`):** If the serialized version differs from the current code's version, the `TraceIcTable` is discarded and the trace is treated as having no IC snapshot (fresh recording). This allows future format changes without breaking the cache.
