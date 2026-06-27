# Phase F: Inlining for JIT-Compiled Traces — Design

> **Status:** Approved — 5 concerns resolved (see below)
> **Scope:** v0.2 — inline hot callee JIT code into caller JIT code (traces + function JIT)
> **Target:** `jit_hot_function_1M`: 129 ms → ~25–70 ms (gap 40× → ~10–22×, unverified — see §6)
> **Pre-reqs:** `9b1a385` (N=16 IC table, 309 tests passing, clippy-clean)
> **Depends on:** Phase E JIT Call (`7540163`), bailout mechanism (`152bc8f`), trace compiler (`b5d11a0`)

---

## 1. Goal & Non-Goals

### Goal
Eliminate the `blr` round-trip overhead for JIT-to-JIT calls in hot loops. When a trace (or function JIT) calls a callee that is itself JIT-compiled and hot, inline the callee's body directly into the caller's compiled code instead of going through `rune_jit_call_helper` + callee entry + return.

### Non-Goals (v0.2)
- **Cross-module inlining.** Inlining across separate `BytecodeProgram` boundaries (e.g., builtins like `Array.push`) is deferred.
- **Recursive inlining.** Inlining a function that (transitively) calls itself. Requires guard against infinite inlining depth.
- **Polymorphic inlining.** Inlining different callees at the same call site based on observed types. V8 does this; Rune will not for v0.2.
- **Inlining into interpreter code.** Phase F only inlines from one JIT context into another. Interpretation always uses the existing `call_helper` path.
- **Speculative inlining.** Always inline based on profile data, not speculation. If the callee is hot at the call site, inline it.

---

## 2. Existing Infrastructure

| Component | Where | What it gives us |
|---|---|---|
| `rune_jit_call_helper` | `vm.rs:4565-4728` | The call path Phase F replaces. 80 lines of frame setup + arg copy + BLR + bailout check. |
| `needs_frame` flag | `vm.rs:4635-4638` | Determines whether the callee needs a `Frame` (has lexical-scope opcodes). Callees with `needs_frame = false` are inlineable without frame manipulation. |
| `jit_locals_buffer` | `vm.rs:217` | `Vec<Value>` used as scratch space for callee locals. Phase F eliminates the copy for inlined callees — the caller's locals ARE the callee's locals (offset by frame size). |
| `BailoutPoint` / `BailoutTable` | `lib.rs:30-41` | Side table per compiled JIT function recording (bc_pc, stack_depth, reason). Phase F appends inlined callee's bailout points with remapped coordinates. |
| `record_bailout_point` | `codegen_aarch64.rs:315` | Called during codegen for every instruction that can bail. Phase F calls this for each inlined callee instruction. |
| `bc_to_native` | `codegen_aarch64.rs:259` | Forward map from bytecode PC to native offset. Phase F extends this mapping for inlined instructions. |
| `TraceIcTable` | `ic.rs:16` | Embedded IC table for property access within traces. Inlined callee property access gets appended IC entries. |
| `ic_table_patches` | `codegen_aarch64.rs:265` | Post-process IC table data emission + ADR fixup. Phase F appends inlined callee IC entries. |
| `emit_prologue` / `emit_epilogue` | `codegen_aarch64.rs:413-429` | Current prologue saves callee-saved regs and sets up Vm/Gc/Locals/JitStack regs. Inlined callee does NOT emit its own prologue/epilogue. |

### What's NOT ready
- **Per-call-site profile data.** No infrastructure tracks "how many times does this `Call` opcode call this particular callee?" We only have JIT entry counts (function-level) and overall call counts (opcode-level). Phase F needs per-call-site heatmaps.
- **Callee JIT code reuse.** When a callee is inlined at multiple call sites, the inlined code is duplicated. For v0.2 this is acceptable — code size is small; AFPC caches the combined result.
- **Inlining budget tracking.** No mechanism limits total inlining depth or code size growth. Phase F adds a simple threshold.

**Verification:** `needs_frame` has been verified empirically for the target benchmark function `function add(a, b) { return a + b; }`. The emitter only emits `MakeArgumentsArray` when the function body uses `arguments` (see `emitter.rs:172`). `add(a,b)` compiles to `LoadLocal 0, LoadLocal 1, Add, Return` — none of the `needs_frame`-triggering opcodes. The existing test `test_jit_no_bail_on_simple_fn` confirms the JIT path works without a frame. This means the headline benchmark IS inlineable — the `needs_frame` concern is resolved.

---

## 3. Data Structures

### 3.1 `InlineProfile` — per-call-site profile data

```rust
/// Profile data for one call site within a loop trace or function JIT.
/// Collected during trace recording / function JIT compilation.
#[derive(Clone, Debug)]
pub struct InlineProfile {
    /// Bytecode PC of the Call instruction.
    pub call_pc: usize,
    /// Number of times this call site has been executed.
    pub hit_count: u64,
    /// Number of times the callee was JIT-compiled at this site.
    pub jit_count: u64,
    /// The callee's Func* if monomorphic at this site.
    pub callee_func: Option<*const u8>,
    /// Callee's JIT entry point, if monomorphic and JIT-compiled.
    pub callee_jit_entry: Option<*const u8>,
    /// Whether the callee needs a Frame (lexical-scope opcodes).
    pub callee_needs_frame: bool,
    /// Size of callee body in bytecode instructions.
    pub callee_bytecode_size: u32,
}
```

### 3.2 `InlinedCallee` — per-inlined-callee state during codegen

```rust
/// State for one inlined callee during codegen.
pub struct InlinedCallee {
    /// The callee's original bytecode program (for reading opcodes).
    pub prog: &'static BytecodeProgram,
    /// Offset applied to all callee bytecode PCs to make them unique
    /// in the combined bailout table (e.g., `callee_pc + CALLER_MAX_PC`).
    pub pc_offset: usize,
    /// Number of callee instructions emitted so far.
    pub emitted_count: u32,
    /// Callee's original JIT stack depth delta (number of pushes
    /// minus pops). Used to compute the merged stack depth.
    pub stack_delta: i32,
    /// Callee's IC tables to embed (for LoadPropertyIC in callee body).
    pub ic_tables: Vec<TraceIcTable>,
    /// Callee's `needs_frame` flag — if true, inlining is prohibited.
    pub needs_frame: bool,
}
```

### 3.3 Extensions to existing structures

**`CodeGen`** (both `Aarch64CodeGen` and x86-64 `CodeGen`):
- Add `inline_profiles: Vec<InlineProfile>` — populated during trace recording.
- Add `inline_depth: u32` — current inlining depth (guard against recursive/multi-level inlining).

**`LoopTrace`:**
- Add `inline_profiles: Vec<InlineProfile>` — collected during trace recording.
- Add `enable_inlining: bool` — feature flag, set by `--inline`/`--no-inline` CLI flag.

**`Config`:**
- Add `enable_inlining: bool` — default `true`. Set via `--no-inline` CLI flag to A/B test or rollback without reverting.

**`BailoutReason`:**
- Add variant `InlinedBail = 5` — bailout from inlined callee code.

**`Aarch64CodeGen::compile`:**
- Accept `inline_profiles: &[InlineProfile]` parameter.

---

## 4. Design

### 4.1 Overview

Phase F replaces the `call_helper` BLR round-trip with direct inlining for hot, monomorphic, frame-less callees:

```
Before (Phase E):
  Caller trace: ... setup args → call_helper BLR → callee JIT → RET → pop → continue
  Overhead per call: arg copy (jit_sp → jit_locals_buffer) + BLR (4 cycles) +
                     callee prologue/epilogue (6 instrs) + return

After (Phase F):
  Caller trace: ... setup args → [inlined callee body] → continue
  Overhead per call: 0 (args are already on the JIT stack)
```

### 4.2 Inlining Eligibility

A call site is eligible for inlining when ALL of the following hold:

1. **Monomorphic callee.** The same `Func` has been observed at this call site for the last K consecutive invocations (K ≥ 50, matching the trace recording threshold).

2. **Callee is JIT-compiled.** `callee.jit_entry` is non-null.

3. **Callee does not need a frame.** `needs_frame` check passes — the callee's body has no `BlockEnter`, `BlockLeave`, `DeclareLet`, `DeclareConst`, `LoadLexical`, `StoreLexical`, or `LoadThis` opcodes. These require frame-level state that inlining cannot provide without frame manipulation. (Matches the existing `Vm::needs_frame` check at `vm.rs:4627-4638`.)

4. **Callee body is small.** Bytecode instruction count ≤ 50 (configurable). This prevents code bloat from inlining large functions.

5. **Inlining depth ≤ 2.** Prevents exponential code growth from nested inlining.

**Non-eligible callees** fall through to the existing `call_helper` path (correct, just slower).

### 4.3 Inlining Mechanics

When a `Call` opcode is encountered during trace recording (or function JIT compilation) and the callee is eligible:

**Step 1 — Remap callee bytecode PCs.**
The callee's bytecode instructions are assigned unique PCs by adding `CALLER_MAX_PC + 1` (or a similar offset) to each callee PC. This ensures unique mapping in the bailout table and trace-to-original-pc mapping.

**Step 2 — Emit callee body inline.**
Each callee instruction is emitted using the same `emit_instruction` path as caller instructions, with these differences:
- `LoadLocal`/`StoreLocal` offsets are adjusted by the caller's local count (locals stack grows).
- `Return` opcode is converted to a jump to "after the call site" — not an actual return instruction.
- Callee's `this` and args are already on the JIT stack (pushed by the Call setup). No arg copy needed.
- Callee's result (what `Return` would push) stays on the stack for the caller to consume.
- Callee `Frame` is NOT allocated — the callee runs in the caller's frame context.

**Step 3 — Merge IC tables.**
If the callee body contains `LoadPropertyIC`/`StorePropertyIC` instructions, their IC data is appended to the caller's `trace_ic_tables`. The `ic_table_patches` mechanism handles the ADR fixup automatically.

**Step 4 — Merge bailout points.**
Each callee instruction that can bail gets a `BailoutPoint` with the remapped PC (caller-relative). The `stack_depth` is adjusted by the caller's current stack depth plus any callee-local stack delta.

**Step 5 — Adjust caller stack bookkeeping.**
The callee's stack delta (net pushes minus pops) is added to the caller's `stack_depth` after the call site. The caller's `pop` of `argc + 2` (args + callee + this) is skipped — the callee's inlined body already consumed them as locals.

### 4.4 Bailout from Inlined Code

Bailouts from inlined code bail the **entire trace** — the caller + callee go back to the interpreter. No attempt is made at partial recovery (deferred to v0.3 if inlined bailout rates prove high).

When a bailout fires from an inlined callee instruction:

1. The bailout helper (`rune_jit_bailout_helper`) captures the JIT stack snapshot as usual.
2. The `bc_pc` in `JitBailoutState` refers to the remapped (caller-relative) PC.
3. The interpreter-side unwind code detects the remapped PC (PC > `CALLER_MAX_PC` threshold), extracts the original callee PC and call-site PC, and unwinds the callee's stack effects (see §4.4.1).
4. The interpreter reconstructs state and resumes from the call site's PC, executing the callee via the normal interpreter path.

**Why this is safe:** Bailouts from inlined code are expected to be rare — shape stability is already verified by the trace recording phase (iteration 50+), and the callee's bytecode runs unmodified, just in native code. If an inlined bailout does occur, the trace will re-record without inlining (the non-eligible path) and stabilizes.

### 4.4.1 Stack Unwinding on Inlined Bailout

When the inlined callee bails, the JIT stack contains the callee's intermediate values, not the caller's state at the call site. The bailout helper must unwind this before the interpreter resumes.

**Required data per `InlinedCallee` (recorded during codegen):**
- `stack_delta: i32` — net stack change caused by the callee (pushes minus pops).
- `call_site_pc: usize` — bytecode PC of the `Call` instruction in the caller.
- `callee_arg_count: u8` — number of arguments passed to the callee.

**Unwind procedure:**
1. Pop `stack_delta` values from the JIT stack (the callee's intermediate state).
2. Push back `callee_arg_count + 2` values (args + callee function + `this`) to restore the pre-call stack depth.
3. Set the interpreter PC to `call_site_pc` (re-executing the `Call` opcode in the interpreter, which will do a normal interpreted call).
4. Do NOT push a `Frame` for the callee (it was inlined — no frame exists in the JIT trace).

**Simplification for v0.2:** The inlining eligibility criteria (§4.2, criterion 3) requires `needs_frame == false`. This means the callee does not create any lexical-scope state (`BlockEnter`, `DeclareLet`, etc.). Therefore, the unwind only needs to reverse stack value changes — no scope state to clean up.

**Verification:** Phase F-3 adds `test_jit_inline_bail` which forces a bailout from an inlined callee (e.g., Smi overflow) and asserts correct interpreter state afterward.

### 4.5 Interaction with Trace Recording

During trace recording (`vm.rs:656-701`), when a `Call` opcode is encountered:

1. Check `InlineProfile` for this call site.
2. If eligible (monomorphic, JIT-compiled, no frame, small body):
   - Append a special "inline boundary" marker to the recorded trace.
   - Append the callee's bytecode instructions (with remapped PCs).
   - Collect the callee's IC entries for property access.
3. If not eligible:
   - Record the `Call` opcode as-is (existing behavior — goes through `call_helper`).

The trace compiler (`compile_trace_native`) handles the inline boundary marker:
- For the boundary marker: skip emitting (it's metadata, not an instruction).
- For the inlined instructions: compile normally with IC and bailout tracking.
- For `Return` opcodes: convert to `Jump` past the call site.

### 4.6 Interaction with AFPC Cache

Inlined code is cached as part of the trace. When the trace is saved to `rune-afpc`:

- The entire inlined body is part of the trace's bytecode program.
- The callee's original `BytecodeProgram` is NOT duplicated — only the inlined instructions are stored.
- On cache load: the trace is re-compiled from the cached bytecode. The inlining decision is re-evaluated: if the callee profile no longer matches (e.g., the callee function changed), the trace re-records naturally via the bail-and-re-record mechanism.

**Key implication:** Inlining decisions are baked into the cached trace. If the callee function is never called again at that site (e.g., the program changed), the trace's inlined code is dead — but it doesn't cause incorrect behavior because the trace's shape guards prevent it from running on incompatible shapes. The trace will bail, re-record, and the new trace will not inline.

---

## 5. Implementation Plan

**Total estimate:** 14 days (optimistic), 3–4 weeks (realistic — past JIT features consistently exceeded initial estimates by 1.5–2×).

### Phase F-0: Feature Flag + Eligibility Verification (1 day)

1. Add `--inline`/`--no-inline` CLI flag to `rune_cli` and `rune_embed`. **Default: `--no-inline`** (flipped to `--inline` when F-2 lands and inlining actually works — prevents confusion during F-0/F-1 where inlining is infrastructure-only).
2. Add `enable_inlining: bool` to `CodeGen` and `LoopTrace`. When `false`, call sites skip inlining and use the `call_helper` path.
3. Write `test_jit_needs_frame_verification` — parameterized test over various function shapes (arrow, closure, generator, function with `let`, function with `arguments`). Print `needs_frame` for each. Assert the target `add(a,b)` shape reports `false`.
4. Write `test_jit_inline_feature_flag` — same program runs identically under both `--inline` and `--no-inline` (verifies no behavior change when flag is inert).

**Deliverable:** Feature flag exists and is wired through. Eligibility check exists as a pure function (no JIT integration yet). `needs_frame` matrix empirically verified. No behavior change. `test_jit_inline_skip_noneligible` (JIT-stats verification) and `test_jit_inline_no_bail` (inline + 0 bailouts) are deferred to F-2 when inlining actually runs.

### Phase F-1: Profile Collection (2 days)

1. Add `InlineProfile` to `LoopTrace` and `CodeGen`.
2. During trace recording, when a `Call` opcode is hit:
   - Record the callee `Func*`, `JitEntry`, `needs_frame`, bytecode size.
   - Increment `hit_count` for this call site.
3. During trace compilation (`compile_trace_native`):
   - Pass `inline_profiles` to `Aarch64CodeGen::compile`.

**Deliverable:** Profile data is collected but unused. No behavior change. Verify with `--trace-stats` output.

### Phase F-2: Inlining Engine (6 days)

1. In `Aarch64CodeGen`, add `inline_depth`, `enable_inlining`, and `inline_profiles`.
2. When emitting a `Call` opcode:
   - If `enable_inlining == false`: emit `call_helper` path (no change).
   - Check eligibility against `InlineProfile` for this call site.
   - If eligible: load callee `BytecodeProgram`, iterate callee instructions, emit them with remapped PCs.
   - Convert callee `Return` → caller jump-past-site.
   - Merge IC tables and bailout tables (including `stack_delta` for §4.4.1).
   - Skip the `call_helper` BLR + bailout check entirely.
3. Handle `needs_frame == true` case: skip inlining, emit `call_helper` path as before.
4. Handle multi-level inlining: guard `inline_depth ≤ 2`.

**Deliverable:** `jit_hot_function_1M` runs with inlined `add(a,b)`. Benchmark shows improvement. 0 bailouts.

### Phase F-3: Bailout Semantics + Stack Unwinding (3 days)

1. Handle inlined bailout by bailing the entire trace (§4.4).
2. Implement stack unwinding (§4.4.1) — pop callee `stack_delta`, push back pre-call stack state, set interpreter PC to `call_site_pc`.
3. Add `BailoutReason::InlinedBail` variant.
4. Add test: `test_jit_inline_bail` — a trace that inlines a callee, then triggers a bailout (e.g., Smi overflow in the inlined body), verifies the interpreter handles it correctly (correct stack state, correct interpreter PC).
5. Verify `--no-inline` disables inlining and all tests pass with both `--inline` and `--no-inline`.

**Deliverable:** Inlined bailout is safe and stack-unwinding produces correct interpreter state. Feature flag verified for both settings.

### Phase F-4: Testing + AFPC (2 days)

1. Verify `jit_hot_function_1M` on aarch64. Measure actual speedup (should be 25–70 ms range).
2. Add `test_jit_inline_monomorphic` — a hot function that calls the same callee 1M times, verify 0 bailouts and `jit_entry_count ≈ 1`.
3. AFPC cache round-trip test: save a trace with inlined code, reload, verify it produces correct results.
4. Verify no regressions on `poly_prop`, `proto_chain`, `loop_sum`, `array_push`.
5. Run full test suite with `--no-inline` to confirm no behavior change when inlining is disabled.

**Deliverable:** All 309+ tests pass on aarch64. Measured speedup documented.

**Note on x86-64:** x86-64 inlining is **deferred to v0.3**. x86-64 has no users, no benchmarks, and currently bails-on-entry for JIT-to-JIT calls rather than executing native code. The 2 days saved are better spent on F-3 stack unwinding and F-4 testing. The x86-64 inliner will be added in v0.3 alongside the copy-and-patch backend rewrite (which makes backend portability free).

---

## 6. Expected Impact

### Primary target: `jit_hot_function_1M`

Current: 129 ms, 999,952 JIT entries, 0 bailouts.
Breakdown:
- `blr` round-trip (call_helper + callee prologue/epilogue): ~90 ns per call × 1M = ~90 ms
- `add()` body (full JIT): ~39 ns per call × 1M = ~39 ms
- Total: ~129 ms

After inlining:
- `blr` round-trip: 0 ns (eliminated)
- `add()` body (inlined): **15–30 ns** per call × 1M — prologue/epilogue/arg-copy removed, but untagging, Smi overflow check, and retagging remain
- Additional: trace body overhead (loop counter, branch, etc.) unchanged at ~10 ns per iter × 1M = ~10 ms
- Total: **~25–40 ms**

**CAUTION:** The 15–30 ns per-inlined-call estimate is **unverified**. The 39 ns figure above is full JIT execution of `add()` including prologue, arg setup, epilogue. After inlining, those are gone, but the remaining cost depends on:
- Register pressure from the merged caller+callee locals stack (may cause spills)
- IC table size growth (the caller's table now includes callee entries)
- Instruction cache effects from larger trace body

The 15 ns lower bound assumes clean register allocation with no spills. The 30 ns upper bound assumes moderate spill pressure. The honest expected range is **~25–70 ms** (gap ~10–22×). Will be measured empirically after Phase F-2.

### Secondary impact: `loop_sum_smi_1M`

No change. The loop body is 11 opcodes with no function calls. Inlining does not apply.

### Secondary impact: `poly_prop_10shapes_1M`

No direct change. The property access loop has no function calls. However, if the warmup phase (`objs.push(o)`) is inlined, the overall trace compile time decreases. Negligible effect on benchmark wall time.

### Secondary impact: `proto_chain_lookup_5deep_1M`

No change. No function calls in the hot loop.

---

## 7. Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Inline candidate is monomorphic during recording but polymorphic at runtime | Bailout from inlined code (entire trace bails → interpreter). Correct but slow. | Rare in practice: the trace records at iteration 50, by which point the callee is almost always stable. If it does polymorph, the trace bails, re-records, and the new trace does not inline (non-eligible). |
| Inlined code size blows up for large callees | Traces become large → more icache misses, higher compile time. | Size threshold (50 instructions) prevents this. Can be lowered if needed. |
| `needs_frame` is too conservative (rejects inlineable callees) | Phase F misses some candidates. | `needs_frame` can be refined in a follow-up. For v0.2, being conservative is correct. |
| Remapped PC space exhausted | `pc_offset` overflows into non-unique space. | `CALLER_MAX_PC` is bounded by trace length (typically < 100 ops). Callee offset `100` + callee size (max 50) = 150. Combined PC space fits in `u32`. No overflow risk. |
| AFPC cache contains stale inlined code | Inlined code runs with a callee that no longer matches. | Trace shape guards prevent this: if the callee's shape (object properties) changes, the guard fails, the trace bails, and re-records. Inlined code is never stale in a way that produces wrong results — only suboptimal. |

---

## 8. What Could Go Wrong

Beyond the enumerated risks above, here are the failure modes that are most likely based on patterns from Phase D (vector IC) and Phase E (JIT Call):

### 8.1 Correctness: Inliner produces wrong code silently

The most dangerous failure mode. The inliner splices callee instructions into the caller's trace. If PC remapping, local slot adjustment, or return-to-jump conversion is wrong, the trace executes incorrect opcodes with no observable error until the wrong result propagates.

**Guard:** The eligibility test plan (§5 F-0, `test_jit_needs_frame_verification`, `test_jit_inline_skip_noneligible`) catches this at compile time (wrong callee selected). The bailout test (`test_jit_inline_bail`) catches stack corruption at runtime. The feature flag (`--no-inline`) ensures we can disable inlining without reverting, isolating regressions.

**Rollback plan:** If the inliner produces incorrect results on any benchmark:
1. Set `--no-inline` as the CLI default (reverts to Phase E behavior).
2. Fix the bug, add a test that would have caught it, then re-enable.

### 8.2 Stack unwinding bug on inlined bailout

The stack unwinding procedure (§4.4.1) is the most subtle piece. If `stack_delta` is wrong, the interpreter resumes with corrupted stack depth and silently produces wrong results or crashes.

**Guard:** The explicit `stack_delta` field on `InlinedCallee` forces the codegen phase to track it. `test_jit_inline_bail` forces bailout in an inlined callee and verifies: (a) correct stack depth, (b) correct interpreter PC, (c) correct result. Run this test under both `--inline` and `--no-inline` to confirm they produce identical results.

**Rollback plan:** Use `--no-inline` and debug `stack_delta` computation.

### 8.3 Phase F doesn't improve the target benchmark

If `needs_frame` were `true` (it's not — verified above), or if the inlined body doesn't run faster in practice (register pressure dominates), Phase F delivers no benefit to `jit_hot_function_1M`.

**Mitigation:** The feature flag ensures we can measure A/B. If the speedup is < 10%, the design should be revisited (profile-guided vs. aggressive inlining) rather than shipping the complexity.

### 8.4 Code size explosion from deep inlining

The 50-instruction-per-callee cap plus max depth 2 limits worst-case trace growth to ~150 instrs (50 caller + 2 × 50 callee). Realistic traces are ~30 caller + ~10 callee = ~40 instrs. No explosion risk.

### 8.5 Test Plan for Inlining Eligibility

Before any inlining code runs, the following tests must pass (Phase F-0):

| Test | What it verifies | Written in |
|---|---|---|---|
| `test_jit_needs_frame_verification` | Parameterized — asserts `needs_frame` for various function shapes (arrow, closure, generator, `let`, `arguments`). Confirms `add(a,b)` target is inlineable. | F-0 |
| `test_jit_inline_feature_flag` | Toggle `--inline`/`--no-inline`. Verify the same program produces identical results. | F-0 |
| `test_jit_inline_skip_noneligible` | Callee with `needs_frame == true` or body ≥ 50 instrs falls through to `call_helper`. F-0 version tests the pure eligibility function. F-2 version verifies JIT stats show no inlining. | F-0 (pure fn) + F-2 (integration) |
| `test_jit_inline_no_bail` | Hot function with inlined callee. 1M iterations. 0 bailouts. Result matches interpreter. Inlining doesn't exist until F-2, so this is written there. | F-2 |

The F-0 tests are written and passing **before** F-1 starts. This is the non-negotiable foundation — the pattern that would have caught the vector IC's N=8 assumption before a week of implementation.

---

## 9. Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Bailout strategy for inlined code | Bail the entire trace | Simpler to implement and correct. Inlined bailout should be rare (profile-guided inlining). |
| Inlining threshold | 50 bytecode instructions | Matches the trace recording threshold. Keeps code size manageable. |
| Maximum inlining depth | 2 levels | Prevents exponential code growth. Real-world hot call chains rarely exceed 2. |
| Frame-less callee only | `needs_frame == false` as prerequisite | Inlining with frame manipulation adds significant complexity. Deferred. |
| Profile collection | During trace recording only | No separate profiling pass needed. The trace recording already observes callee behavior at the call site. |
| AFPC interaction | Inlining baked into cached trace | Simplest approach. Re-recording handles profile drift naturally. |
| Feature flag | `--inline`/`--no-inline` (default: `--no-inline` for F-0/F-1, flipped to `--inline` when F-2 lands) | Default is `--no-inline` during infrastructure phases to prevent "is inlining on?" confusion. Flipped in F-2 commit. Enables A/B testing and rollback without reverting. |
| x86-64 backend priority | **Deferred to v0.3** | x86-64 has no users, no benchmarks, no JIT-to-JIT calls today. v0.2 focuses on aarch64 (M4 Pro). The copy-and-patch rewrite in v0.3 makes backend portability free. |
| Stack unwinding complexity | Explicit `stack_delta` + `call_site_pc` + `callee_arg_count` recorded per `InlinedCallee` | Forces codegen to track this explicitly rather than reconstructing it at bailout time. The 3 fields are the minimum needed for correct unwinding (§4.4.1). |

---

## 10. Future Work (Post-v0.2)

- **Cross-module inlining:** Inlining builtins (`Array.push`, `Math.max`, etc.) into JIT code. Requires `needs_frame` relaxation or frame synthesis.
- **Polymorphic inlining:** Multiple callee versions at the same call site. Requires dispatch table + callee-specific traces.
- **Speculative inlining:** Inlining based on probabilistic profiling (not just observed monomorphic). Higher risk, higher reward.
- **Frame-ful inlining:** Inlining callees with lexical-scope opcodes. Requires synthesizing a `Frame` within the inlined body.
- **Inlining into function JIT (not just traces):** Currently Phase F only inlines during trace recording. Function JIT inlining is a separate effort.
- **x86-64 inlining:** Mirror the aarch64 inliner in `codegen.rs`. Deferred from v0.2 — x86-64 has no users or benchmarks, and the copy-and-patch rewrite in v0.3 makes backend portability free.
