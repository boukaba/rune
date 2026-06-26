# Multi-Shape Dispatch for Trace JIT — Design

> **Status:** Draft for review
> **Scope:** v0.2 — polymorphic property access within compiled traces
> **Target:** `poly_prop_10shapes_1M`: 722 ms → ~80–100 ms (gap: 173× → ~25×)
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
| **Expected poly_prop** | 722 → ~80 ms |

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

**SIMD width choice:** N = 4. ARM64 NEON `CMEQ` on 16-byte registers can compare 2 u64 shape_ids per instruction (8 per register pair). With 4 entries, we need 2 `CMEQ` ops (16 bytes × 2 = 32 bytes = 4 u64s). If we later need N = 8, it's 4 `CMEQ` ops — still under 5 ns on M4.

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

/// Fixed-size dispatch table, N=4 entries.
/// ADR-loadable from JIT code.
#[repr(C, align(16))]
struct TraceIcTable {
    entries: [TraceIcEntry; 4],
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

// Step 3: Load 4 entries (16 bytes each → 64 bytes total)
ldp     q5, q6, [x4]      // q5 = entries[0..1], q6 = entries[2..3]
ldp     q7, q8, [x4, 32]  // q7 = shape_id fields of entries, q8 = slot_offset fields

// Step 4: Broadcast shape_id into a NEON register
dup     v9.2d, x3         // v9 = {shape_id, shape_id}

// Step 5: Compare all 4 shape_ids in parallel
cmeq    v10.2d, v9.2d, v5.2d  // v10 = {shape_id==e0, shape_id==e1}
cmeq    v11.2d, v9.2d, v6.2d  // v11 = {shape_id==e2, shape_id==e3}

// Step 6: Extract comparison results
xtn     v10.4h, v10.4s    // narrow to 16-bit lanes
xtn     v11.4h, v11.4s
orr     v10.8b, v10.8b, v11.8b  // combine
umaxv   h10, v10.8b       // h10 = lane max (non-zero if any match)
smov    x5, v10.b[0]      // x5 = match mask
cbz     x5, _miss         // if no match, bail to interpreter

// Step 7: Find the matching entry index (use slot_offset from that entry)
// For N=4, use a small switch:
//   mask bit 0 → index 0, bit 1 → index 1, bit 2 → index 2, bit 3 → index 3
// Use CLZ or table lookup; simplest: AND mask to isolate first set bit.
rbitt   x5, x5            // reverse bits → x5 bit N corresponds to match at index 3-N
clz     x6, x5            // x6 = index of first match (0..3)
lsl     x6, x6, 4         // x6 = index * 16 (offset into TraceIcTable)
add     x7, x4, x6        // x7 = &entries[match_idx]
ldr     x8, [x7, 8]       // x8 = slot_offset (offset 8 in TraceIcEntry)

// Step 8: Walk prototype chain if proto_depth > 0
// proto_depth stored at offset 12 in TraceIcEntry
ldrb    w9, [x7, 12]
cbz     w9, _load_prop
// (emit unrolled loop: proto_depth steps of ldr x1, [x1, 24])

_load_prop:
// Step 9: Load property from [x1 + 32 + slot_offset]
add     x10, x1, #32
add     x10, x10, x8, lsl #3  // x10 = x1 + 32 + slot_offset * 8
ldr     x0, [x10]
// ... continue with push / store

_miss:
// Call bailout_helper as before
```

**ADDR patching:** The ADR instruction at step 2 encodes a signed PC-relative offset (±1MB). At trace compilation time, we know the final code size. We emit a placeholder ADR and record the offset for patching after the `TraceIcTable` address is known.

### 4.4 Interaction with Existing Systems

| System | Interaction |
|---|---|
| **SIDT / InlineCache** | Vector IC is a read-only snapshot of the IC at recording time. IC continues to be updated by the interpreter. No changes to IC. |
| **AFPC** | `TraceIcTable` is serialized as part of the trace. AFPC serializes/deserializes the table alongside the trace code. |
| **Trace recording** | Recording logic captures IC snapshot at each LoadPropertyIC/StorePropertyIC opcode boundary. No change to recording of other opcodes. |
| **Bailout** | If no entry matches (N+1th shape), bail to interpreter at the same bc_idx. Same bailout mechanism as current shape miss. |
| **Phase F inlining** | Inlined property accesses use the same vector IC; the dispatch table is embedded in the inlined code, not the caller trace. |

---

## 5. The 6 Design Questions

### Q1: What's the data structure?

`TraceIcTable` — a fixed-size `[TraceIcEntry; 4]` array embedded as literal data in the JIT code page. Each entry contains `shape_id`, `slot_offset`, `proto_depth`. Aligned to 16 bytes for NEON load. Chosen over per-loop-head HashMap because:
- Zero dispatch-side allocation (embedded in code page)
- Single-cycle SIMD compare vs hash lookup
- No eviction policy (bail to interpreter for >N shapes)
- Snapshot semantics: captures IC state at recording time, not live

### Q2: When do you record?

At trace recording time, for each `LoadPropertyIC`/`StorePropertyIC` bytecode encountered:
- Read the interpreter's `InlineCache` for this `(func_idx, bc_pc)`
- Copy the top 4 entries (sorted by hit count) into a `TraceIcTable`
- If the IC has <4 entries, pad with `{shape_id: 0}` (never matches)
- Record the bytecode index and a placeholder ADR offset

Re-recording happens when the current trace misses on shape (≥1 miss → increment counter → at threshold, re-record with updated IC snapshot).

### Q3: Eviction policy?

No eviction within the trace. If all 4 slots are occupied and a 5th shape appears, the trace bails to interpreter (same as current behavior). The interpreter's IC (no cap, SIDT) continues to handle all shapes correctly. The trace will re-record when the loop reaches the tier-up threshold again, picking up the top 4 shapes from the IC at that point.

This is simpler than LRU eviction and matches the performance model: if >4 shapes are active at a single property access site, the trace JIT won't cover it (rare in practice; V8's JIT also has a polymorphic cap at 4).

### Q4: Bailout semantics?

If no `TraceIcEntry` matches:
1. Clear bailout flag at `jit_stack[63]` (offset 504)
2. Push object and key back onto JIT stack (same as current miss path)
3. Call `bailout_helper(vm_ptr, bc_idx, jit_sp)` — same as current
4. Bailout returns to interpreter at the same bc_idx
5. Interpreter's IC handles the lookup (will succeed, just slower)
6. No penalty beyond the per-bailout cost

### Q5: How does this interact with AFPC?

The `TraceIcTable` is serialized as part of the trace data, alongside bailout points and other metadata. AFPC serialization/deserialization must handle `[TraceIcEntry; 4]`. This is a straightforward `bincode`/custom serialization addition.

AFPC key consideration: the trace's persistent identity is the bytecode hash of the loop body. The IC snapshot is NOT part of the identity — it's loaded from the cached trace and populated from the interpreter's IC at install time. If the persistent cache has a trace for this bytecode hash, the IC snapshot is refreshed from the current interpreter state (which may have different hot shapes across process invocations).

### Q6: How does this interact with Phase F inlining?

Phase F inlining splices callee bytecode into the caller trace. If the inlined code contains `LoadPropertyIC`/`StorePropertyIC`, those ICs get their own `TraceIcTable` embedded in the inlined code. No special interaction: the vector IC per property access site is independent.

Key consideration: the inline boundary shape guard (checking the callee function object's shape) does NOT use the vector IC — it's a single shape guard (the callee function is monomorphic per callsite in practice). Only property access within the inlined body uses the vector IC.

---

## 6. Implementation Plan

### Phase 1: Data structures and recording (Day 1–2)

1. Define `TraceIcEntry` and `TraceIcTable` in `codegen.rs` or a new module
2. At `TraceCompiler::record_opcode(LoadPropertyIC)`: read the interpreter's IC top-4 entries
3. Store the `TraceIcTable` in the `CompiledTrace` metadata (beside bailout points)
4. Serialize/deserialize in AFPC

### Phase 2: Codegen (Day 3–4)

1. In `codegen_aarch64.rs::emit_opcode(LoadPropertyIC)`:
   - If `TraceIcTable` has 1 entry: emit current code (single shape guard) — no change
   - If >1 entry: emit vector IC dispatch with SIMD compare
   - Emit NEON `cmeq` / `xtn` / `umaxv` sequence
   - Emit ADR to embedded table, patch after code emission
2. Same for `StorePropertyIC`
3. Add NEON instruction emission helpers (`cmeq`, `dup`, `xtn`, `orr`, `umaxv`, `smov`, `rbitt`, `clz`)

### Phase 3: Test and verify (Day 5)

1. `poly_prop_10shapes_1M`: verify bench drops from 722 ms to ~80–150 ms
2. `test_hot_property_mono_1m`: verify no regression (single-entry path unchanged)
3. `proto_chain_lookup_5deep_1M`: verify no regression (proto_depth works with vector IC)
4. All 307 integration tests: no regressions
5. AFPC round-trip test for TraceIcTable serialization

---

## 7. Test Plan

| Test | What it proves |
|---|---|
| `poly_prop_10shapes_1M` | 10-shape polymorphic property access at one callsite |
| `test_hot_property_mono_1m` | Single-shape path unchanged (no regression) |
| `test_ic_polymorphic` | Interpreter IC snapshot populates correctly at record time |
| `test_ic_proto_inherited` | proto_depth works with vector IC |
| New: `poly_prop_4shapes_vs_5shapes` | N=4 cap: 4 shapes should be fast, 5th should degenerate to bail+interpreter |
| New: `poly_prop_shape_rotation` | Hot shapes change over time; trace re-records and picks up new top-K |
| AFPC round-trip | Serialize/deserialize `TraceIcTable`, verify identical dispatch behavior |

---

## 8. Benchmark Impact Estimates

| Benchmark | Before | After (est.) | Improvement |
|---|---|---|---|
| `poly_prop_10shapes_1M` | 722 ms | 80–150 ms | 5–9× |
| `test_hot_property_mono_1m` | ~1.3s | ~1.3s (unchanged) | 0% (expected — single shape ID path) |
| `proto_chain_lookup_5deep_1M` | 106 ms | 90–106 ms | 0–15% (noise — proto_depth path unchanged) |
| `jit_hot_function_1M` | 120 ms | 120 ms (unchanged) | 0% (no property access) |

**poly_prop estimate rationale:**
- 9 remaining gaps to V8 (722 ms / 4.16 ms = 173× current)
- Vector IC eliminates interpreter fallback on 4 of 10 shapes
- Remaining 6 of 10 shapes still bail → ~60% of iterations run in interpreter
- With JIT runs for 4 shapes (40%) + interpreter for 6 (60%), naive estimate: `0.4 × fast + 0.6 × slow` where `fast ≈ ~2ms` (JIT overhead) and `slow ≈ 722ms/10 ≈ 72ms` per shape
- More realistically: 4/10 shapes with steady-state trace → ~50ms, 6/10 shapes in interpreter + bailout overhead → ~100ms → total ~150ms
- If vector IC captures the first 4 shapes encountered (which are the 4 most frequent), the hit rate is higher → ~80ms possible
- **Conservative estimate: 100–150ms** (gap: 173× → ~25–36×)

---

## 9. Open Questions

1. **ADR range:** The `TraceIcTable` may be >1MB from the ADR instruction in large traces with many IC sites. Fallback: use `ADRP + ADD` (4KB page-relative, 4GB range) instead of `ADR` (1MB range).
2. **NEON register pressure:** The dispatch sequence uses 6 NEON registers (v5–v11, v4 for ADR). With the register allocator already using v0–v4 for float64 operations, we may need to spill. Audit register usage before implementing.
3. **M1/M2/M3 NEON compatibility:** `cmeq` is available on all ARMv8.0+ NEON (M1+). No compatibility concern. But `rbitt` (reverse bits) is ARMv8.0+; `clz` is ARMv8.0+. All fine.
4. **Should we support both single-entry and multi-entry codegen paths?** Yes — for the common case (monomorphic, 1 IC entry), emit the existing single-guard code (no SIMD overhead). Only emit vector IC when the IC has >1 entry at recording time.
5. **What about megamorphic (>4 shapes) callsites?** The interpreter handles them. The trace JIT will never cover this case — and it shouldn't (megamorphic sites are rare and the interpreter's O(1) SIDT is already fast).

---

## 10. Decision Summary

**Selected: Option B — Deegen-Style Vector IC**

- Embed a fixed-size `[TraceIcEntry; 4]` table in the JIT code section
- Dispatch via NEON SIMD compare (2 `cmeq` ops + reduce)
- N=4 entries (covers monomorphic through quadromorphic, bails on 5th shape)
- Single trace per loop head, not N traces
- Expected: `poly_prop_10shapes_1M` 722 ms → 80–150 ms

**Implementation effort: 1 week**

Fallback if vector IC proves too complex: Option A (multi-shape traces), re-estimated at 2 weeks.
