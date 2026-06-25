## Goal
Add input Smi type guards to all value-consuming JIT opcodes so non-Smi values bail to the interpreter (Phase B).

## Constraints & Preferences
- Every opcode that pops a value from the JIT stack must check Smi tag (bit 0 = 1) before operating; if not a Smi, restore the JIT stack, record `BailoutReason::NonSmiInput`, call the bailout helper, and return.
- Binary ops check both operands (after pop, before save). Unary ops check the single operand.
- `BailoutReason::NonSmiInput = 1` already exists in `lib.rs`.
- All existing tests must pass; clippy clean.
- The same guard pattern must be implemented on both backends.

## Progress
### Done
- **Phase B: input Smi guards fully implemented on both backends** and pushed (`90fc0b8`).
- **x86-64**: `emit_smi_check` helper (TEST rax,1 / JE bail / JMP ok), guards on 24 opcodes.
- **aarch64**: `emit_smi_check` helper (TBZ X0,#0,nosmi — no register clobber), guards on same 24 opcodes.
- **Deduplicated Le/Ge/StrictEq** on x86-64 (removed duplicate unguarded blocks).
- **Updated offset tests** for larger code size on both backends.
- **Updated JumpIfFalse tests** to use Smi values (non-Smi sentinels now bail to interpreter).
- **`vm_stub()` lint fix**: `bailout_stub as usize` → `bailout_stub as *const () as usize`.
- **PR2 overflow guards + IC miss-path** previously committed (`af6aa95`, `2204ca2`).

### In Progress
- None.

### Blocked
- **Integration tests for input guards**: `MakeArgumentsArray` bail‑on‑entry still prevents arithmetic/property opcodes from being reached through `Context::eval`. Only unit tests (direct JIT call) can test guards until Phase C/D.

## Key Decisions
- **x86-64 `emit_smi_check`**: Saves rax first on bail, then iterates saved register indices to push previous values (chronological order). On success path, rax is unmodified.
- **aarch64 `emit_smi_check`**: Uses `TBZ X0, #0, <bail>` which tests bit 0 without modifying any register. No register clobbering. On bail, pushes x0 then iterates saved register indices.
- **Register pressure**: x86‑64: b → rcx, a → r9. aarch64: b → x9 (Add/Sub/Mul/Shl) or x1 (others), a → x8.
- **UnaryPlus**: Was a no-op (pop, check Smi, push back) — now guarded like other unary ops.

## Critical Context
- **x86-64 jump patch formula**: `edit_target - (patch_addr + 4)` for both Jcc rel32 and JMP rel32. The displacement is relative to the *end* of the instruction. Jcc rel32: 2‑byte opcode + 4‑byte disp → end = disp_field + 4. JMP rel32: 1‑byte opcode + 4‑byte disp → end = disp_field + 4. Using `+6` (Jcc) or `+5` (JMP) is wrong and causes SIGSEGV.
- **aarch64 jump patch formula**: `((target - patch_addr) / 4)` for fixed‑width 4‑byte instructions. B.cond: imm19 × 4 (bits 23:5), B.uncond: imm26 × 4 (bits 25:0).

## Relevant Files
- `crates/rune_jit_baseline/src/codegen.rs`: x86-64 `emit_smi_check` + all input guards (24 opcodes).
- `crates/rune_jit_baseline/src/codegen_aarch64.rs`: aarch64 `emit_smi_check` + all input guards (24 opcodes).
- `crates/rune_jit_baseline/src/lib.rs`: `BailoutReason::NonSmiInput = 1`.
- `crates/rune_jit_baseline/src/assembler.rs`: x86-64 jump instructions (emit_jg_rel32, emit_jl_rel32).
