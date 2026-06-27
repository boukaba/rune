# Phase F: Inlining for JIT-Compiled Traces — Design

> **Status:** Draft for review
> **Scope:** v0.2 — inline hot callee JIT code into caller JIT code (traces + function JIT)
> **Target:** `jit_hot_function_1M`: 129 ms → ~30–50 ms (gap 40× → ~10–15×)
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

3. **Callee does not need a frame.** `needs_frame` check passes — the callee's body has no `BlockEnter`, `DeclareLet`, `DeclareConst`, `LoadLexical`, `StoreLexical`, `MakeEnv`, or `LoadCaptured` opcodes. These require frame-level state that inlining cannot provide without frame manipulation.

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

If a bailout fires from an inlined callee instruction:

1. The bailout helper (`rune_jit_bailout_helper`) captures the JIT stack snapshot as usual.
2. The `bc_pc` in `JitBailoutState` refers to the remapped (caller-relative) PC.
3. The interpreter-side unwind code checks the `bailout_table` to find the original callee PC and determine that this was an inlined call.
4. The interpreter reconstructs state by:
   - Pushing a Frame for the caller (as normal)
   - NOT pushing a Frame for the callee (it was inlined — no frame exists)
   - Restoring the interpreter stack to the correct depth (args + callee + this are still on the stack)

**Alternative (simpler but slower):** Don't attempt partial recovery from inlined bailout. Instead, bail the ENTIRE trace — the inlined caller + callee go back to the interpreter. The interpreter loop continues from the call site's bc_pc, executing the callee via the normal interpreter path. This is simpler to implement and is correct for v0.2. The performance loss from bailing the whole trace is negligible — bailouts from inlined code should be rare (shape stability is already verified by the trace recording phase).

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

### Phase F-1: Profile Collection (3 days)

1. Add `InlineProfile` to `LoopTrace` and `CodeGen`.
2. During trace recording, when a `Call` opcode is hit:
   - Record the callee `Func*`, `JitEntry`, `needs_frame`, bytecode size.
   - Increment `hit_count` for this call site.
3. During trace compilation (`compile_trace_native`):
   - Pass `inline_profiles` to `Aarch64CodeGen::compile`.

**Deliverable:** Profile data is collected but unused. No behavior change. Verify with `--trace-stats` output.

### Phase F-2: Inlining Engine (5 days)

1. In `Aarch64CodeGen`, add `inline_depth` and `inline_profiles`.
2. When emitting a `Call` opcode:
   - Check eligibility against `InlineProfile` for this call site.
   - If eligible: load callee `BytecodeProgram`, iterate callee instructions, emit them with remapped PCs.
   - Convert callee `Return` → caller jump-past-site.
   - Merge IC tables and bailout tables.
   - Skip the `call_helper` BLR + bailout check entirely.
3. Handle `needs_frame == true` case: skip inlining, emit `call_helper` path as before.
4. Handle multi-level inlining: guard `inline_depth ≤ 2`.

**Deliverable:** `jit_hot_function_1M` runs with inlined `add(a,b)`. Benchmark shows improvement. 0 bailouts.

### Phase F-3: Bailout Semantics (2 days)

1. Handle inlined bailout by bailing the entire trace (simpler approach from §4.4).
2. Add `BailoutReason::InlinedBail` variant.
3. In the interpreter bailout path:
   - Detect remapped PC (PC > offset threshold).
   - Extract original callee PC.
   - Do NOT push a callee Frame (it was inlined).
   - Continue interpreter from the call-site's PC (callee will be interpreted normally from here).
4. Add test: `test_jit_inline_bail` — a trace that inlines a callee, then triggers a bailout (e.g., Smi overflow in the inlined body), verifies the interpreter handles it correctly.

**Deliverable:** Inlined bailout is safe. All existing tests pass.

### Phase F-4: x86-64 Backend (2 days)

1. Same mechanics as aarch64 in `codegen.rs`.
2. `emit_inline_callee` method mirrors the aarch64 version.
3. x86-64 JIT-to-JIT calls are currently `bail-on-entry` (not native). Phase F inlining replaces these calls with native inline code — effectively giving x86-64 its first working JIT-to-JIT inlining.

**Deliverable:** Both backends support inlining.

### Phase F-5: Testing + AFPC (2 days)

1. Verify `jit_hot_function_1M` on both backends.
2. Add `test_jit_inline_monomorphic` — a hot function that calls the same callee 1M times, verify 0 bailouts and `jit_entry_count ≈ 1` (trace is recorded once with inlined body).
3. Add `test_jit_inline_skip_noneligible` — verify that callees with `needs_frame == true` or large bodies are NOT inlined (fall through to `call_helper`).
4. AFPC cache round-trip test: save a trace with inlined code, reload, verify it produces correct results.
5. Verify no regressions on `poly_prop`, `proto_chain`, `loop_sum`, `array_push`.

**Deliverable:** All 309+ tests pass. Both backends verified.

---

## 6. Expected Impact

### Primary target: `jit_hot_function_1M`

Current: 129 ms, 999,952 JIT entries, 0 bailouts.
Breakdown:
- `blr` round-trip (call_helper + callee prologue/epilogue): ~90 ns per call × 1M = ~90 ms
- `add()` body: ~39 ns per call × 1M = ~39 ms
- Total: ~129 ms

After inlining:
- `blr` round-trip: 0 ns (eliminated)
- `add()` body: ~39 ns per call × 1M = ~39 ms
- Additional: trace body overhead (loop counter, branch, etc.) unchanged at ~10 ns per iter × 1M = ~10 ms
- Total: ~49 ms

**Expected result: 129 ms → ~49 ms (62% reduction, gap 40× → ~15×)**

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

## 8. Key Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Bailout strategy for inlined code | Bail the entire trace | Simpler to implement and correct. Inlined bailout should be rare (profile-guided inlining). |
| Inlining threshold | 50 bytecode instructions | Matches the trace recording threshold. Keeps code size manageable. |
| Maximum inlining depth | 2 levels | Prevents exponential code growth. Real-world hot call chains rarely exceed 2. |
| Frame-less callee only | `needs_frame == false` as prerequisite | Inlining with frame manipulation adds significant complexity. Deferred. |
| Profile collection | During trace recording only | No separate profiling pass needed. The trace recording already observes callee behavior at the call site. |
| AFPC interaction | Inlining baked into cached trace | Simplest approach. Re-recording handles profile drift naturally. |

---

## 9. Future Work (Post-v0.2)

- **Cross-module inlining:** Inlining builtins (`Array.push`, `Math.max`, etc.) into JIT code. Requires `needs_frame` relaxation or frame synthesis.
- **Polymorphic inlining:** Multiple callee versions at the same call site. Requires dispatch table + callee-specific traces.
- **Speculative inlining:** Inlining based on probabilistic profiling (not just observed monomorphic). Higher risk, higher reward.
- **Frame-ful inlining:** Inlining callees with lexical-scope opcodes. Requires synthesizing a `Frame` within the inlined body.
- **Inlining into function JIT (not just traces):** Currently Phase F only inlines during trace recording. Function JIT inlining is a separate effort.
