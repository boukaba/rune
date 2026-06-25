# Rune JIT Bailout Mechanism — Design

> **Status:** Draft for review
> **Author:** Manager review
> **Scope:** v1 bailout — covers Phases A–D. Phase E (native Call) deferred.
> **Pre-reqs:** `e557218` (green baseline, 434 tests passing)

---

## 1. Goal & Non-Goals

### Goal
Allow a JIT-compiled function to **safely fall back to the interpreter** at any bytecode PC when it encounters a value or opcode it cannot handle natively. Once this exists, `is_jit_compatible` can be loosened and `all_smi` removed, unlocking the 14 currently-rejected opcodes.

### Non-Goals (v1)
- **Native `Call`.** `Call` always bails in v1. Re-entering JIT after a bail is a v2 problem (requires resume-from-PC entries).
- **Lazy / speculative deopt.** v1 is eager — guards at every opcode boundary that can fail.
- **Tiering back up.** Once a function bails, that *invocation* runs in the interpreter. The function is not re-JIT'd; the next call starts fresh in the JIT.
- **Trace compiler bailout.** The trace compiler already has its own "abort recording" path. Trace *execution* bailout is a separate, smaller follow-up after function-JIT bailout lands. (Trace execution bailout is actually simpler — see §10.)

---

## 2. Existing Infrastructure We Build On

| Component | Where | What it gives us |
|---|---|---|
| `JitHelpers` table | `vm.rs:94`, `codegen_aarch64.rs:32` | Fixed-offset (512) function-pointer table the JIT already calls into. Slot 0 = `lexical_helper`; slot 1 is reserved → **becomes `bailout_helper`**. |
| `rune_jit_lexical_helper` | `vm.rs:3982` | `extern "C"` helper called from JIT. **Pattern to copy** for `rune_jit_bailout_helper`. |
| Per-callsite entry guard | `vm.rs:2697` | `Self::all_smi(&jit_locals) && this_ok` already does function-level bailout. The new mechanism is *intra-function* bailout from a specific bc_pc. |
| `LoadPropertyIC` shape guards | `codegen_aarch64.rs:744` | Already emits inline shape-guard → branch-to-miss pattern. **The miss path is currently silently wrong** (pushes `undefined`). Bailout subsumes and fixes this. |
| `bc_to_native` | `codegen.rs:33`, `codegen_aarch64.rs` | Forward map bc→native. The bailout side table is the *companion* data: per-bc_pc stack depth + bailout id. |
| `JitVmState::jit_stack` | `codegen_aarch64.rs:40` | The JIT value stack is heap memory at offset 0 from `vm_ptr`. The bailout helper can read it directly via `vm_ptr`. |
| `Frame` struct | `vm.rs:49` | Interpreter frame shape. Bailout must materialize one of these. |

### New infrastructure required
- One new field on `Vm`: `jit_bailout: JitBailoutState`.
- One new field on `Vm`: `jit_stack_base: *mut u64` (written by JIT prologue).
- One new slot in `JitHelpers` (`bailout_helper`).
- One new side table per compiled function: `bailout_table: Vec<BailoutPoint>`.
- One new field on `Func`: `bailout_table: *const BailoutTable` (null = no bailouts possible).

---

## 3. Data Structures

### 3.1 `BailoutPoint` — compile-time side table

```rust
/// One entry per bytecode PC where a bailout can originate.
/// Emitted by CodeGen during compilation, stored alongside ExecutableMemory.
#[derive(Clone, Copy, Debug)]
pub struct BailoutPoint {
    /// Bytecode PC this bailout corresponds to. The interpreter resumes
    /// *at* this PC (re-executing the instruction that bailed).
    pub bc_pc: usize,
    /// Expected JIT value-stack depth at this PC (number of u64 slots).
    /// The bailout helper asserts this matches the live stack — catches
    /// off-by-one bugs loudly instead of silently corrupting the interpreter.
    pub stack_depth: u32,
    /// Reason tag for stats / debugging.
    pub reason: BailoutReason,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum BailoutReason {
    Overflow = 0,        // Phase A: Smi arithmetic out of i31 range
    NonSmiInput = 1,     // Phase A: input value has wrong tag
    BailOnEntry = 2,     // Phase B: opcode not natively compiled
    ShapeMiss = 3,       // LoadPropertyIC / StorePropertyIC guard failed
    Unimplemented = 4,   // Safety net — should never fire
}
```

### 3.2 `BailoutTable` — owned by `Func`

```rust
/// Heap-allocated side table, one per JIT-compiled function.
/// Stored as `*const` on `Func` because the JIT entry pointer alone is
/// not enough to recover the table — many functions can share an entry
/// shape but have different bc_pc layouts.
#[repr(C)]
pub struct BailoutTable {
    pub points: [BailoutPoint],  // sorted by bailout_id == index
}
```

**Storage convention:** `Func::bailout_table: *const BailoutTable` (null if JIT'd before bailout mechanism existed, or if function has zero bailout points). Set alongside `Func::set_jit_entry()`.

### 3.3 `JitBailoutState` — runtime, on `Vm`

```rust
/// Written by the bailout helper, read by the vm.rs call site.
#[repr(C)]
pub struct JitBailoutState {
    /// Bytecode PC where the bailout occurred (set by helper).
    pub bc_pc: usize,
    /// Set by helper to signal a bailout. Checked by call site instead of
    /// `bc_pc != 0` — MakeArgumentsArray is emitted as the very first
    /// instruction of every non-arrow function (bc_pc = 0), so the former
    /// sentinel `bc_pc = 0 ⇒ no bailout` would miss a bail at PC 0.
    pub pending: bool,
    /// Snapshot of the JIT value stack at bailout, deepest-first.
    /// Copied from `jit_stack_base..jit_stack_sp` by the helper.
    pub stack_snapshot: Vec<u64>,
    /// Reason tag (for stats).
    pub reason: BailoutReason,
}

impl Default for JitBailoutState {
    fn default() -> Self {
        Self { bc_pc: 0, pending: false, stack_snapshot: Vec::new(), reason: BailoutReason::Unimplemented }
    }
}
```

### 3.4 `JitHelpers` extension

```rust
#[repr(C)]
pub struct JitHelpers {
    pub lexical_helper: usize,   // offset 512 — unchanged
    pub bailout_helper: usize,   // offset 520 — NEW (was _reserved[0])
    _reserved: [usize; 6],
}
```

Both the `vm.rs` and `codegen_aarch64.rs` definitions must stay in sync.

---

## 4. Compilation: Guard Site Emission

### 4.1 Bailout ID = bc_pc

Don't introduce a separate bailout_id. The bc_pc is unique per guard site, the compiler has it in hand during emission, and the interpreter needs it to resume. One identifier, one source of truth.

### 4.2 Stack depth tracking

`CodeGen` already iterates bytecode in order. Add a `stack_depth: u32` field, updated per opcode:
- `LoadSmi`, `LoadUndefined`, `LoadNull`, `LoadBoolean`, `LoadFloat64`, `LoadLocal`, `LoadThis`, `LoadLexical`, `LoadPropertyIC`, `LoadGlobal`, `Dup`, `LoadString*`, `NewObject`, `NewArray`, `MakeFunction`, `TypeOf`, `Neg`, `Not`, `BitNot`, `UnaryPlus`, `Void` → +1
- `Pop`, `StoreLocal`, `StoreLexical`, `StoreGlobal`, `StorePropertyIC`, `Throw`, `Return`, `BlockLeave` → -1
- `Add`, `Sub`, `Mul`, `Div`, `Mod`, `Exp`, `Shl`, `Shr`, `ShrU`, `BitAnd`, `BitOr`, `BitXor`, `Eq`, `Ne`, `StrictEq`, `StrictNe`, `Lt`, `Gt`, `Le`, `Ge`, `In`, `Instanceof`, `StringConcat`, `ArrayPush` → -1 (two popped, one pushed)
- `Jump`, `JumpIfTrue`, `JumpIfFalse` → 0 or -1 (JumpIfTrue/False pop the condition)
- `Swap` → 0
- `Call` → `-(argc+1) + 1` = `-argc` (pops callee+this+args, pushes result)
- `BlockEnter`, `BlockLeave`, `DeclareLet`, `DeclareConst`, `IncLocal`, `DecLocal`, `IncGlobal`, `DecGlobal`, `TryBegin`, `TryEnd`, `FinallyDone`, `MakeEnv`, `RestoreEnv`, `InitGenerator`, `ForInInit`, `ForInNext`, `CopyLexical` → 0 (or check per-op)

Record `stack_depth` at the *start* of each opcode's emission. This goes into `BailoutPoint.stack_depth` for any guard emitted at that pc.

### 4.3 Phase A guard emission (AArch64 example — `Add` overflow)

Current `Add` emits: pop, add, push. New emission adds an overflow check:

```asm
; --- existing ---
sub  x22, x22, #8           ; pop b
ldr  x0, [x22]
mov  x1, x0                 ; x1 = b
sub  x22, x22, #8           ; pop a
ldr  x0, [x22]
and  x0, x0, #-2            ; clear tag
adds x0, x0, x1             ; ADDS sets flags
; --- new overflow guard ---
b.vc 1f                     ; skip bail if no overflow
mov  x2, x22                ; arg2 = current jit_sp
movz x1, #<bc_pc>           ; arg1 = bc_pc (low 16 bits; use movk if >65535)
mov  x0, x19                ; arg0 = vm_ptr
ldr  x15, [x19, #520]       ; x15 = bailout_helper
blr  x15
; bailout helper sets vm.jit_bailout.pending = true
b 2f                       ; always bail — jump to epilogue
1:
; --- continue ---
```

**Do not inspect the return value.** The bailout helper writes `vm.jit_bailout.pending = true` (with `bc_pc` and `stack_snapshot`). The JIT call site in `vm.rs` checks `vm.jit_bailout.pending` after the JIT function returns — never inspects the JIT return value. This avoids the sentinel collision problem entirely. The `pending` flag is cleared before every JIT call and after handling the bailout.

For overflow guards (PR2, §9), the pattern is the same — call the helper and bail. No return-value check in asm:

```asm
ldr  x15, [x19, #520]
blr  x15
; Always bail — vm.rs checks vm.jit_bailout.pending
b 2f                       ; jump to epilogue
1:
; ... continue opcode ...
2:
; function epilogue (shared with Return)
```

### 4.4 Phase B bail-on-entry emission

For opcodes in the Phase B list (§7.2), emit a guard at the *start* of the opcode's native code, before any other instruction:

```asm
mov  x2, x22                ; arg2 = jit_sp
movz x1, #<bc_pc>           ; arg1 = bc_pc
mov  x0, x19                ; arg0 = vm_ptr
ldr  x15, [x19, #520]
blr  x15
b 2f                        ; always bail — jump to epilogue
```

The opcode does no native work; the interpreter handles it from bc_pc.

### 4.5 Where the side table lives during compilation

Add to `CodeGen`:
```rust
pub struct CodeGen {
    mem: ExecutableMemory,
    bc_to_native: Vec<usize>,
    pending_patches: Vec<(usize, usize)>,
    bailout_table: Vec<BailoutPoint>,    // NEW
    stack_depth: u32,                     // NEW
}
```

Change `compile()` return type:
```rust
pub struct CompiledFunction {
    pub mem: ExecutableMemory,
    pub bailout_table: Vec<BailoutPoint>,
}

pub fn compile(mut self, program: &BytecodeProgram) -> CompiledFunction { ... }
```

Update all call sites in `vm.rs` (function JIT tier-up at line 2660, trace compilation at line 3151).

---

## 5. Runtime: The Bailout Helper

```rust
/// Bailout helper, called from JIT guard sites via JitHelpers.bailout_helper.
///
/// Arguments (System V / AAPCS64):
///   vm_ptr   = arg0
///   bc_pc    = arg1  (the bytecode PC that bailed)
///   jit_sp   = arg2  (current value of JIT_STACK_REG)
///
/// Returns: 0 (unused — vm.rs checks vm.jit_bailout.bc_pc).
///
/// Side effects:
///   - Writes vm.jit_bailout.bc_pc = bc_pc
///   - Writes vm.jit_bailout.stack_snapshot = [jit_stack_base..jit_sp]
///   - Writes vm.jit_bailout.reason = (looked up from table)
///
/// # Safety
/// Caller must pass valid vm_ptr and jit_sp pointing into vm.jit_stack.
#[unsafe(no_mangle)]
pub extern "C" fn rune_jit_bailout_helper(
    vm_ptr: *mut u8,
    bc_pc: u64,
    jit_sp: u64,
) -> u64 {
    let vm = unsafe { &mut *(vm_ptr as *mut Vm) };
    let base = vm.jit_stack_base as *const u64;
    let sp = jit_sp as *const u64;
    let depth = ((sp as usize) - (base as usize)) / 8;

    // Optional: lookup reason from the current function's bailout table.
    // For v1, just tag as BailOnEntry unless we add per-site metadata.
    let reason = BailoutReason::BailOnEntry;  // refined in Phase A

    // Snapshot the JIT stack into a Vec<u64>.
    let mut snapshot = Vec::with_capacity(depth);
    unsafe {
        for i in 0..depth {
            snapshot.push(*base.add(i));
        }
    }

    vm.jit_bailout.bc_pc = bc_pc as usize;
    vm.jit_bailout.stack_snapshot = snapshot;
    vm.jit_bailout.reason = reason;
    0
}
```

### 5.1 `jit_stack_base` plumbing

The JIT prologue (after setting up `JIT_STACK_REG`/`rbx`) must store the base pointer into `vm.jit_stack_base`:

**AArch64:**
```asm
; x22 = VM_REG + jit_stack_offset  (existing)
str  x22, [x19, #<offset_of_jit_stack_base>]   ; NEW
```

**x86-64:**
```asm
; rbx = rsp  (existing)
mov  [r15 + <offset_of_jit_stack_base>], rbx   ; NEW
```

`offset_of_jit_stack_base` is computed from the `Vm` struct layout. To keep it stable, declare it as a `#[repr(C)]` field at a fixed position (right after `jit_helpers`).

---

## 6. Call Site: `vm.rs` Changes

### 6.1 Setup before JIT call

At `vm.rs:2681` (where `jit_entry` is checked non-null), set up the helper pointer:

```rust
self.jit_helpers.bailout_helper = rune_jit_bailout_helper as usize;
self.jit_bailout.bc_pc = 0;  // clear any stale bailout
```

### 6.2 After the JIT call

Currently (line 2704):
```rust
let result_raw = unsafe { func(vm_ptr, gc_ptr, jit_locals.as_mut_ptr() as *mut u64) };
self.last_locals = jit_locals;
self.push(Value::from_raw(result_raw));
self.frames[fi].pc = pc + 1;
continue;
```

New:
```rust
let result_raw = unsafe { func(vm_ptr, gc_ptr, jit_locals.as_mut_ptr() as *mut u64) };

if self.jit_bailout.bc_pc != 0 {
    // Bailout: materialize a Frame and resume in interpreter.
    let bailout_pc = self.jit_bailout.bc_pc;
    let snapshot = std::mem::take(&mut self.jit_bailout.stack_snapshot);
    self.jit_bailout.bc_pc = 0;  // clear

    // Build the frame exactly as the fall-through interpreter path does
    // (see existing code at vm.rs:2729), but with pc = bailout_pc.
    let func_env = unsafe { Func::env_ptr(ptr as *mut Func) };
    let passed_argc = args.len();
    let mut locals: Vec<Value> = if func_prog.named_function { vec![callee] } else { vec![] };
    locals.extend(args);
    let stack_base = self.stack.len();
    // Push the bailout stack snapshot onto the interpreter stack.
    for raw in &snapshot {
        self.stack.push(Value::from_raw(*raw));
    }
    self.frames.push(Frame {
        locals,
        lexical_slots: Vec::new(),
        lexical_tdz: Vec::new(),
        lexical_const: Vec::new(),
        scope_boundaries: Vec::new(),
        passed_argc,
        pc: bailout_pc,
        stack_base,
        prog: func_prog as *const BytecodeProgram,
        generator_id: None,
        this,
        is_constructor_call: false,
        constructed_object: Value::undefined(),
        env: func_env,
    });
    continue;  // interpreter loop resumes the new frame at bailout_pc
}

// Normal JIT return path (unchanged)
self.last_locals = jit_locals;
self.push(Value::from_raw(result_raw));
self.frames[fi].pc = pc + 1;
continue;
```

**Critical correctness notes:**
- `stack_base` is set to the stack length *before* pushing the snapshot. When the new frame returns, its `Return` opcode will pop down to `stack_base + 1` (the return value), which is correct.
- `pc = bailout_pc`, not `bailout_pc + 1`. The instruction that bailed must re-execute in the interpreter — the JIT did not complete it.
- `lexical_slots` is empty because the JIT maintains lexical state via the lexical helper (which writes into the *current top* frame — but during JIT there is no top frame). **This is a known gap:** if the bailout happens inside a `let` block, the lexical state is lost. Mitigation: see §10.2.

---

## 7. Per-Opcode Classification

### 7.1 Phase A — Inline runtime guards (current `is_jit_compatible` whitelist unchanged)

These opcodes pass `is_jit_compatible` today but can fail at runtime. Add overflow/type guards:

| Opcode | Guard | Reason |
|---|---|---|
| `Add`, `Sub`, `Mul` | `ADDS`/`SUBS`/`SMULL` overflow flag → bail | Smi is i31; result may exceed. |
| `Neg` | Operand == `-(2^30) << 1 \| 1` → bail | Negating min i31 overflows. |
| `Shl` | Result outside i31 range → bail | `1 << 30` is out of Smi. |
| `Shr`, `ShrU` | (No guard needed — result is always ≤ i32) | OK as-is. |
| `LoadFloat64` | (No runtime guard — compile-time check is sufficient) | OK as-is. |
| `LoadPropertyIC`, `StorePropertyIC` | Replace the existing silent-miss with a real bailout | **Fixes the latent bug.** |

### 7.2 Phase B — Bail-on-entry stubs (loosen `is_jit_compatible` to allow these)

The JIT emits the bail-on-entry stub from §4.4 and does no native work. The interpreter handles them.

**Literals:** `LoadString`, `LoadStringConst`
**Unary:** `TypeOf`
**Arithmetic (non-integer):** `Div`, `Mod`, `Exp`
**Objects:** `NewObject`, `NewArray`, `ArrayPush`, `ArrayExtend`, `ArraySlice`, `SpreadIntoObject`, `LoadProperty`, `StoreProperty`, `DeleteProperty`, `DefineProperty`
**Strings:** `ToString`, `StringConcat`
**Globals:** `LoadGlobal`, `StoreGlobal`, `IncGlobal`, `DecGlobal`
**Control flow:** `Throw`, `ThrowIfNullish`, `TryBegin`, `TryEnd`, `FinallyDone`
**Functions:** `MakeFunction`, `Call`, `CallFromArray`, `New`, `MakeRestArray`, `MakeArgumentsArray`, `CopyLexical`
**Generators:** `Yield`, `YieldStar`, `Resume`, `InitGenerator`
**for-in:** `ForInInit`, `ForInNext`
**Environment:** `MakeEnv`, `RestoreEnv`, `LoadCaptured`, `StoreCaptured`
**Relational:** `In`, `Instanceof`

### 7.3 Phase C — Native JIT support (incremental, in any order)

Each opcode moves from Phase B (bail) to native. Suggested order by ROI:

1. **`TypeOf`** — 2 branches on tag bits + 1 helper call for heap-object tag string. ~30 min.
2. **`LoadStringConst`** — already cached in `vm.string_cache[prog_ptr][idx]`. Single helper call returning the cached `Value`. ~1 hr.
3. **`LoadGlobal`, `StoreGlobal`, `IncGlobal`, `DecGlobal`** — helper callout to existing `vm.globals` HashMap. ~2 hrs.
4. **`Div`, `Mod`, `Exp`** — promote to heap Float64, use SSE/NEON. Result Smi'd if in range. ~1 day.
5. **`NewObject`, `NewArray`, `ArrayPush`** — helper callouts. ~1 day.
6. **`ToString`, `StringConcat`** — helper callouts to existing builtins. ~1 day.

### 7.4 Phase D — Remove `all_smi`

After Phase C ships, `is_jit_compatible` allows everything, every opcode either has native code or bails cleanly. The `all_smi(&jit_locals)` check at `vm.rs:2697` becomes redundant — any non-Smi input will trigger a `NonSmiInput` bailout at the first opcode that cares. **Remove the check.** Half a day.

### 7.5 Phase E — Native `Call` (v2, deferred)

Out of scope. Requires:
- Resume-from-PC JIT entries (so the interpreter can re-enter JIT after a Call returns).
- Argument marshaling between JIT stack and interpreter stack.
- Re-entry guard: don't re-enter JIT if the same function bailed recently.

---

## 8. Test Matrix

### 8.1 Phase A — overflow guards

One test per arithmetic opcode, each in a function large enough to JIT (`>= MIN_JIT_FUNCTION_SIZE`):

```js
function add_overflow(a, b) { return a + b; }
// Call with (2^30 - 1, 1) — exceeds Smi range.
// Assert: result == 2^30 (correct, via interpreter).
// Assert: vm.jit_bailout.bc_pc was set then cleared (JIT path taken, bailed).
```

Opcodes: `Add`, `Sub`, `Mul`, `Neg`, `Shl`. Five tests.

### 8.2 Phase B — bail-on-entry stubs

For each Phase B opcode, **one round-trip equality test**: build a function that uses the opcode, run it once with the JIT disabled (interpreter-only), once with the JIT enabled, assert results equal. Cover at least:

- `LoadStringConst`: `function f() { return "hello"; }`
- `TypeOf`: `function f(x) { return typeof x; }` (call with smi, string, undefined, object)
- `Call`: `function f(x) { return g(x); }` where `g` is non-JIT'd
- `Div`: `function f(a, b) { return a / b; }` with non-integer result
- `NewObject`: `function f() { return {x: 1}; }`
- `NewArray`: `function f() { return [1, 2, 3]; }`
- `ArrayPush`: `function f(arr, v) { arr.push(v); return arr.length; }`
- `LoadProperty` (non-IC): `function f(o) { return o.x; }` on first call (before IC patching)
- `ToString`/`StringConcat`: `function f(name) { return "hello " + name; }`
- `LoadGlobal`/`StoreGlobal`: `function f() { g = 42; return g; }`
- `Throw`: `function f() { throw new Error("x"); }`
- `MakeFunction`: `function f() { function inner() { return 1; } return inner(); }`

~15 tests. Each must assert: (a) result matches interpreter-only run, (b) bailout occurred.

### 8.3 Phase C — native support

For each opcode migrated from Phase B to native, the §8.2 test is kept but the "bailout occurred" assertion flips to "bailout did NOT occur." Same test, expected behavior changes. This makes migration safe-by-construction.

### 8.4 Phase D — `all_smi` removal

Run the entire 434-test suite. Expect 100% green. Add 5 new tests that pass non-Smi arguments to JIT'd functions (strings, objects, floats) and assert correctness.

### 8.5 Regression: existing LoadPropertyIC bug

Add a test that exercises `LoadPropertyIC` shape miss under JIT and asserts the result is **the correct property value from the interpreter**, not `undefined`. This codifies the bug fix from §7.1.

---

## 9. Implementation Order (5 PRs)

| PR | Scope | Est. | Tests added |
|---|---|---|---|
| **PR1** | Infrastructure: `BailoutPoint`, `BailoutState`, `JitHelpers.bailout_helper`, `rune_jit_bailout_helper`, `jit_stack_base` plumbing, `vm.rs` call-site changes. No new guards yet — just the mechanism. Add a single test opcode that always bails (`TypeOf`) to prove the path works end-to-end. | 2 days | ~3 |
| **PR2** | Phase A: overflow guards on `Add`/`Sub`/`Mul`/`Neg`/`Shl`. Fix `LoadPropertyIC`/`StorePropertyIC` miss path. | 1 day | ~6 |
| **PR3** | Phase B: loosen `is_jit_compatible`, add bail-on-entry stubs for all §7.2 opcodes. | 1.5 days | ~15 |
| **PR4** | Phase C (incremental): `TypeOf`, `LoadStringConst`, `LoadGlobal`/`StoreGlobal`/`IncGlobal`/`DecGlobal`. | 2 days | ~8 |
| **PR5** | Phase D: remove `all_smi`. | 0.5 days | ~5 |

**Total: ~7 days.** After PR5, the JIT covers the full opcode set (with bailout for the still-unimplemented subset), and `all_smi` is gone.

Phase C continues beyond PR4 with `Div`/`Mod`/`Exp`, `NewObject`, etc. — each is an independent PR after PR5.

---

## 10. Risks & Open Questions

### 10.1 Lexical state loss on bailout (§6.2)

If a function uses `let`/`const` and bails mid-block, `Frame::lexical_slots` is empty — the interpreter sees no `let` bindings and re-declares them on `BlockEnter` (which is re-executed since `pc = bailout_pc`). This is *probably* correct (TDZ is reset, the block re-enters), but I want a test for it before committing. **Action: PR1 must include a `let`-in-loop bailout test.**

Worst case: the JIT's lexical helper calls already wrote into the *caller's* frame (since the JIT'd function had no frame of its own). That state is gone. If this breaks, the fix is to materialize a stub Frame before the JIT call (so lexical helpers write into a real frame), then on bailout we already have it. **Defer this complexity unless the test fails.**

### 10.2 Trace compiler bailout

The trace compiler (`compile_trace` at `vm.rs:3080+`) is a separate codepath. It has its own `is_jit_compatible` check at line 3148 and bails (aborts recording) on incompatible opcodes — but it has no *runtime* bailout. Once Phase A lands, the trace compiler should use the same bailout helper. **Suggested follow-up to PR2**, not blocking.

### 10.3 `Func::bailout_table` lifetime

The `BailoutTable` is heap-allocated and owned by `Func`. The `Func` is in the GC heap. **The GC does not know about `bailout_table`.** If the function is collected, the table leaks (or worse, is freed while a JIT call is in flight). Mitigation: store `BailoutTable` in `Vm` keyed by entry pointer (`HashMap<usize, Box<BailoutTable>>`), not on `Func`. The `Vm` outlives all JIT calls. **Action: revise §3.2 to use Vm-owned table.**

### 10.4 Stack depth assertion cost

The `stack_depth` field on `BailoutPoint` is checked in the bailout helper (`debug_assert_eq!(depth, point.stack_depth)`). In release builds this is stripped — but a release-mode mismatch silently corrupts the interpreter stack. **Recommendation:** keep the check in release builds for v1 (cheap: one compare per bailout). Remove once we have 1000+ bailouts in CI without a mismatch.

### 10.5 AFPC cache invalidation

`BailoutTable` is per-function. If we serialize it into the AFPC cache (rkyv), cached code from before this change is incompatible. **Action: bump the AFPC cache format version.** Old caches must be invalidated (delete and recompile).

### 10.6 x86-64 codegen

This doc uses AArch64 examples. The x86-64 codegen in `codegen.rs` needs the same treatment — same opcodes, same side table, same helper signature. The REX-prefix tag-test patterns differ but the logic is identical. PR1 must ship both backends in lockstep; do not let them drift.

---

## Appendix A: Why not setjmp/longjmp?

Considered and rejected. Reasons:
1. **UB in Rust.** `setjmp`/`longjmp` over FFI is only well-defined for C; Rust's borrow checker assumes the call stack unwinds normally.
2. **Invisible control flow.** A `longjmp` from inside JIT code bypasses all Rust destructors in `vm.rs`'s call stack. The `Frame` push, the `args` vec, the `gc` borrow — all leak.
3. **No benefit.** The bailout helper can do everything we need by writing to a `Vm` field and returning normally. The JIT function's epilogue runs (popping callee-saved registers), control returns to `vm.rs`, and we inspect the state. One extra function return is ~10ns. Not worth the UB.

## Appendix B: Why eager, not lazy deopt?

Lazy deopt (compile speculatively, only invalidate when an assumption actually breaks) is a tier-2 optimization. It requires:
- Dependency tracking: which compiled code depends on which assumptions.
- Invalidation: sweep all dependent code on assumption break.
- Re-entry: re-JIT with new assumptions.

For a baseline JIT, this is overkill. Eager deopt (guard at every opcode boundary) costs ~2 instructions per guard site and is trivially correct. Rune is currently 5–230× slower than V8 on hot loops; the guard overhead is in the noise. Revisit when the JIT is within 5× of V8.

