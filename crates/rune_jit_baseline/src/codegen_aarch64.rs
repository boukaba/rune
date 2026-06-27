/// AArch64 native code emission for trace compilation.
///
/// ARM64 instructions are fixed 32-bit. All registers are 64-bit (x0-x30).
/// Callee-saved: x19-x28, x29(fp), x30(lr).
///
/// Calling convention (AAPCS64):
///   x0 = vm_ptr, x1 = gc_ptr, x2 = locals_ptr
///   return value in x0
///
/// The trace uses VM-heap memory for its value stack instead of the native
/// stack pointer. On macOS Apple Silicon, JIT pages are restricted from
/// writing through sp, so we keep the real stack pointer intact and use a
/// dedicated pointer (x22) into `JitVmState::jit_stack` at offset 0 from the
/// VM pointer.
use crate::assembler::ExecutableMemory;
use crate::ic::{InlineEntry, InlinePlan, InlineProfile, TraceIcTable};
use crate::{BailoutPoint, BailoutReason, BailoutTable, CompiledFunction};
use rune_bytecode::opcode::Opcode;
use rune_jit_stencils::{LOAD_SMI_16_BYTES, LOAD_SMI_32_BYTES, RUNE_PUSH_HELPER};

/// Operation codes for the lexical helper callout (must match vm.rs).
const LEX_BLOCK_ENTER: u64 = 0;
const LEX_BLOCK_LEAVE: u64 = 1;
const LEX_DECLARE_LET: u64 = 2;
const LEX_DECLARE_CONST: u64 = 3;
const LEX_LOAD: u64 = 4;
const LEX_STORE: u64 = 5;
const LEX_LOAD_THIS: u64 = 6;

/// Number of u64 slots reserved for the trace value stack.
pub const JIT_STACK_SIZE: usize = 64;

/// Smi i31 range constants for overflow detection.
pub const MAX_I31: u64 = 0x3FFFFFFF; // 2^30 − 1
pub const MIN_I31: u64 = 0xFFFF_FFFF_C000_0000; // −2^30

/// JIT helper function pointer table. Must match `Vm::jit_helpers` layout.
#[repr(C)]
pub struct JitHelpers {
    pub lexical_helper: usize,
    pub bailout_helper: usize,
    pub typeof_helper: usize,
    pub string_helper: usize,
    pub global_helper: usize,
    /// Helper that promotes Add operands to f64 on Smi overflow or non-Smi input.
    pub float64_add_helper: usize,
    /// Call helper for JIT-to-JIT function calls (Phase E).
    pub call_helper: usize,
    _reserved: [usize; 1],
}

/// VM state visible to the trace compiler. Must be placed at offset 0 from
/// the VM pointer passed to emitted trace code.
#[repr(C)]
pub struct JitVmState {
    pub jit_stack: [u64; JIT_STACK_SIZE],
    pub jit_helpers: JitHelpers,
    pub jit_stack_base: u64,
}

/// Register assignments for the trace compiler.
const VM_REG: u32 = 19; // callee-saved, holds Vm pointer
const GC_REG: u32 = 20; // callee-saved, holds GC pointer
const LOC_REG: u32 = 21; // callee-saved, holds locals pointer
const JIT_STACK_REG: u32 = 22; // callee-saved, holds JIT value-stack pointer

/// Emit a full 32-bit instruction.
fn emit(mem: &mut ExecutableMemory, instr: u32) {
    mem.emit_byte(instr as u8);
    mem.emit_byte((instr >> 8) as u8);
    mem.emit_byte((instr >> 16) as u8);
    mem.emit_byte((instr >> 24) as u8);
}

/// MOV xd, xm  (ORR xd, xzr, xm)
fn mov_reg(mem: &mut ExecutableMemory, xd: u32, xm: u32) {
    if xd == 31 {
        emit(mem, 0x91000000 | (xm << 5) | 31);
    } else if xm == 31 {
        emit(mem, 0x91000000 | (31 << 5) | xd);
    } else {
        emit(mem, 0xAA0003E0 | (xm << 16) | xd);
    }
}

/// MOVZ xd, #u16, lsl #0
fn movz(mem: &mut ExecutableMemory, xd: u32, imm16: u16) {
    emit(mem, 0xD2800000 | ((imm16 as u32) << 5) | xd);
}

/// MOVK xd, #imm16, lsl #(shift*16)
fn movk(mem: &mut ExecutableMemory, xd: u32, imm16: u16, shift: u32) {
    emit(mem, 0xF2800000 | (shift << 21) | ((imm16 as u32) << 5) | xd);
}

/// MOV xd, #u64 (split across MOVZ + MOVK)
fn mov_imm64(mem: &mut ExecutableMemory, xd: u32, val: u64) {
    let w0 = val as u16;
    let w1 = (val >> 16) as u16;
    let w2 = (val >> 32) as u16;
    let w3 = (val >> 48) as u16;
    movz(mem, xd, w0);
    if w1 != 0 {
        movk(mem, xd, w1, 1);
    }
    if w2 != 0 {
        movk(mem, xd, w2, 2);
    }
    if w3 != 0 {
        movk(mem, xd, w3, 3);
    }
}

/// ADD xd, xn, xm
fn add_reg(mem: &mut ExecutableMemory, xd: u32, xn: u32, xm: u32) {
    emit(mem, 0x8B000000 | (xm << 16) | (xn << 5) | xd);
}

/// SUB xd, xn, xm
fn sub_reg(mem: &mut ExecutableMemory, xd: u32, xn: u32, xm: u32) {
    emit(mem, 0xCB000000 | (xm << 16) | (xn << 5) | xd);
}

/// ADD xd, xn, #imm12
fn add_imm(mem: &mut ExecutableMemory, xd: u32, xn: u32, imm12: u32) {
    emit(mem, 0x91000000 | ((imm12 & 0xFFF) << 10) | (xn << 5) | xd);
}

/// SUB xd, xn, #imm12
fn sub_imm(mem: &mut ExecutableMemory, xd: u32, xn: u32, imm12: u32) {
    emit(mem, 0xD1000000 | ((imm12 & 0xFFF) << 10) | (xn << 5) | xd);
}

/// SUBS xzr, xn, xm  (CMP)
fn cmp_reg(mem: &mut ExecutableMemory, xn: u32, xm: u32) {
    emit(mem, 0xEB00001F | (xm << 16) | (xn << 5));
}

/// AND xd, xn, xm
fn and_reg(mem: &mut ExecutableMemory, xd: u32, xn: u32, xm: u32) {
    emit(mem, 0x8A000000 | (xm << 16) | (xn << 5) | xd);
}

/// ORR xd, xn, xm
fn orr_reg(mem: &mut ExecutableMemory, xd: u32, xn: u32, xm: u32) {
    emit(mem, 0xAA000000 | (xm << 16) | (xn << 5) | xd);
}

/// EOR xd, xn, xm
fn eor_reg(mem: &mut ExecutableMemory, xd: u32, xn: u32, xm: u32) {
    emit(mem, 0xCA000000 | (xm << 16) | (xn << 5) | xd);
}

/// LSL xd, xn, xm  (register shift)
fn lsl_reg(mem: &mut ExecutableMemory, xd: u32, xn: u32, xm: u32) {
    emit(mem, 0x9AC02000 | (xm << 16) | (xn << 5) | xd);
}

/// ASR xd, xn, xm  (register arithmetic shift right)
fn asr_reg(mem: &mut ExecutableMemory, xd: u32, xn: u32, xm: u32) {
    emit(mem, 0x9AC02800 | (xm << 16) | (xn << 5) | xd);
}

/// LSR xd, xn, xm  (register logical shift right)
fn lsr_reg(mem: &mut ExecutableMemory, xd: u32, xn: u32, xm: u32) {
    emit(mem, 0x9AC02400 | (xm << 16) | (xn << 5) | xd);
}

/// ORR xd, xn, #1 — set bit 0 (Smi tag)
fn orr_imm1(mem: &mut ExecutableMemory, xd: u32, xn: u32) {
    // ORR xd, xn, #1: bitmask encoding N:immr:imms = 0:000000:000000
    emit(mem, 0xB2400000 | (xn << 5) | xd);
}

/// LDR xd, [xn, #uoffset]  — unsigned offset, scaled by 8
fn ldr_off(mem: &mut ExecutableMemory, xd: u32, xn: u32, uoffset: u32) {
    emit(mem, 0xF9400000 | ((uoffset >> 3) << 10) | (xn << 5) | xd);
}

/// STR xd, [xn, #uoffset]  — unsigned offset, scaled by 8
fn str_off(mem: &mut ExecutableMemory, xd: u32, xn: u32, uoffset: u32) {
    emit(mem, 0xF9000000 | ((uoffset >> 3) << 10) | (xn << 5) | xd);
}

/// B #offset  (unconditional branch, imm26 offset in instructions)
#[allow(dead_code)]
fn b_imm(mem: &mut ExecutableMemory, offset_in_instrs: i32) {
    emit(mem, 0x14000000 | ((offset_in_instrs as u32) & 0x3FF_FFFF));
}

/// B.EQ #offset  (conditional branch on equal)
#[allow(dead_code)]
fn b_eq(mem: &mut ExecutableMemory, offset_in_instrs: i32) {
    emit(
        mem,
        0x54000000 | (((offset_in_instrs as u32) & 0x7FFFF) << 5),
    );
}

/// B.NE #offset  (conditional branch on not equal)
#[allow(dead_code)]
fn b_ne(mem: &mut ExecutableMemory, offset_in_instrs: i32) {
    let imm19 = (offset_in_instrs as u32) & 0x7FFFF;
    emit(mem, 0x54000000 | (imm19 << 5) | 1);
}

/// RET
fn ret(mem: &mut ExecutableMemory) {
    emit(mem, 0xD65F03C0);
}

// LDP helper removed — use two ldr_off calls instead of the
// pair load to avoid encoding bugs. The compiler will fuse them
// in hardware (Apple M-series load fusion).

/// LDR Xt, [Xn, Xm, LSL #0]  (register offset)
fn ldr_reg(mem: &mut ExecutableMemory, xt: u32, xn: u32, xm: u32) {
    emit(mem, 0xF8600800 | (xm << 16) | (3 << 13) | (xn << 5) | xt);
}

/// STR Xt, [Xn, Xm, LSL #0]  (register offset)
fn str_reg(mem: &mut ExecutableMemory, xt: u32, xn: u32, xm: u32) {
    emit(mem, 0xF8200800 | (xm << 16) | (3 << 13) | (xn << 5) | xt);
}

fn push_callee_saved(mem: &mut ExecutableMemory) {
    let mut stp = |rt: u32, rt2: u32| emit(mem, 0xA9BF0000 | (rt2 << 10) | (31 << 5) | rt);
    stp(29, 30);
    stp(19, 20);
    stp(21, 22);
    stp(23, 24);
    stp(25, 26);
}

fn pop_callee_saved(mem: &mut ExecutableMemory) {
    let mut ldp = |rt: u32, rt2: u32| emit(mem, 0xA8C10000 | (rt2 << 10) | (31 << 5) | rt);
    ldp(25, 26);
    ldp(23, 24);
    ldp(21, 22);
    ldp(19, 20);
    ldp(29, 30);
}

// ===========================================================================
// Function AOT compiler (parallel to x86_64 CodeGen)
// ===========================================================================

/// AArch64 function baseline JIT compiler.
///
/// Calling convention (AAPCS64):
///   x0 = vm_ptr, x1 = gc_ptr, x2 = locals_ptr
///   return value in x0
///
/// The JIT value stack lives in VM heap memory (`JitVmState::jit_stack`) so
/// that JIT pages on macOS Apple Silicon do not need to write through `sp`.
/// A pending ADR-to-IC-table patch. The ADR at `adr_offset` needs to be
/// fixed up to point to the 128-byte (8-entry) table data written at a
/// later offset in the emitted code.
struct IcTablePatch {
    adr_offset: usize,
    table: TraceIcTable,
}

pub struct Aarch64CodeGen {
    mem: ExecutableMemory,
    bc_to_native: Vec<usize>,
    pending_patches: Vec<(usize, usize, u32)>, // (patch_offset_in_bytes, target_bc_index, original_instr)
    bailout_table: Vec<BailoutPoint>,
    stack_depth: u32,
    /// Initial offset added to x22 (JIT stack pointer) during prologue.
    /// Used by tests that pre-populate the JIT stack.
    jit_stack_offset: u32,
    /// Polymorphic IC tables for trace-compiled property accesses.
    /// Indexed by trace instruction position; empty tables (count=0) mean
    /// single-guard code is used.
    ic_tables: Vec<TraceIcTable>,
    /// Pending ADR-to-table patches collected during compilation.
    /// Resolved at the end of `compile()` by writing table data and
    /// patching the ADR instructions.
    ic_table_patches: Vec<IcTablePatch>,
    /// Inlining profiles collected during trace recording.
    /// Populated by F-1; superseded by `inline_plan` in F-2.
    #[allow(dead_code)]
    inline_profiles: Vec<InlineProfile>,
    /// Inlining plan: describes which call sites to inline (F-2 Layer 2a+).
    inline_plan: InlinePlan,
    /// If true, use stencil-based code emission for supported opcodes.
    stencil_jit: bool,
}

impl Aarch64CodeGen {
    pub fn new(instruction_count: usize) -> Self {
        let mem = ExecutableMemory::allocate(64 * 1024);
        Self {
            mem,
            bc_to_native: vec![0; instruction_count],
            pending_patches: Vec::new(),
            bailout_table: Vec::new(),
            stack_depth: 0,
            jit_stack_offset: 0,
            ic_tables: Vec::new(),
            ic_table_patches: Vec::new(),
            inline_profiles: Vec::new(),
            inline_plan: InlinePlan::default(),
            stencil_jit: false,
        }
    }

    /// Set an initial offset for the JIT stack pointer.
    /// Allows pre-populated values on the JIT stack to be read correctly.
    pub fn with_jit_stack_offset(mut self, offset: u32) -> Self {
        self.jit_stack_offset = offset;
        self
    }

    pub fn with_trace_ic_tables(mut self, tables: Vec<TraceIcTable>) -> Self {
        self.ic_tables = tables;
        self
    }

    pub fn with_inline_profiles(mut self, profiles: Vec<InlineProfile>) -> Self {
        self.inline_profiles = profiles;
        self
    }

    pub fn with_inline_plan(mut self, plan: InlinePlan) -> Self {
        self.inline_plan = plan;
        self
    }

    pub fn with_stencil_jit(mut self, enabled: bool) -> Self {
        self.stencil_jit = enabled;
        self
    }

    fn push(&mut self) {
        // x0 -> [jit_stack]; jit_stack += 8
        str_off(&mut self.mem, 0, JIT_STACK_REG, 0);
        add_imm(&mut self.mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        self.stack_depth += 1;
    }

    fn pop(&mut self) {
        // jit_stack -= 8; x0 <- [jit_stack]
        sub_imm(&mut self.mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        ldr_off(&mut self.mem, 0, JIT_STACK_REG, 0);
        self.stack_depth = self.stack_depth.saturating_sub(1);
    }

    /// Record a bailout point at the current bytecode PC.
    fn record_bailout_point(&mut self, bc_pc: usize, reason: BailoutReason) {
        self.bailout_table.push(BailoutPoint {
            bc_pc,
            stack_depth: self.stack_depth,
            reason,
        });
    }

    /// Check if the Smi-encoded value in x0 fits in i31 range.
    /// On overflow: restore stack from saved registers, record bailout, call helper, epilogue.
    /// On no overflow: fall through with x0 unchanged.
    fn emit_smi_overflow_bailout_or_continue(
        &mut self,
        bc_idx: usize,
        saved_a: Option<u32>,
        saved_b: Option<u32>,
    ) {
        emit(&mut self.mem, 0x9341FC02); // ASR x2, x0, #1 (untag)
        mov_imm64(&mut self.mem, 3, MAX_I31);
        cmp_reg(&mut self.mem, 2, 3); // CMP x2, x3
        let patch_jg = self.mem.current_offset();
        emit(&mut self.mem, 0x5400000C); // B.GT +0

        mov_imm64(&mut self.mem, 3, MIN_I31);
        cmp_reg(&mut self.mem, 2, 3); // CMP x2, x3
        let patch_jl = self.mem.current_offset();
        emit(&mut self.mem, 0x5400000B); // B.LT +0

        // No overflow — skip bailout
        let patch_done = self.mem.current_offset();
        emit(&mut self.mem, 0x14000000); // B +0

        // Overflow: restore stack, record bailout, call helper, epilogue
        let ov_label = self.mem.current_offset();
        if let Some(reg) = saved_a {
            mov_reg(&mut self.mem, 0, reg);
            self.push();
        }
        if let Some(reg) = saved_b {
            mov_reg(&mut self.mem, 0, reg);
            self.push();
        }
        self.record_bailout_point(bc_idx, BailoutReason::Overflow);
        mov_reg(&mut self.mem, 2, JIT_STACK_REG);
        mov_imm64(&mut self.mem, 1, bc_idx as u64);
        mov_reg(&mut self.mem, 0, VM_REG);
        ldr_off(&mut self.mem, 15, VM_REG, 520);
        emit(&mut self.mem, 0xD63F01E0); // BLR x15
        movz(&mut self.mem, 0, 0);
        self.push();
        self.emit_epilogue();

        // Patch forward jumps
        let done_label = self.mem.current_offset();
        let d_jg = ((ov_label as i64 - patch_jg as i64) / 4) as u32;
        self.mem
            .patch_u32(patch_jg, 0x5400000C | ((d_jg & 0x7FFFF) << 5));
        let d_jl = ((ov_label as i64 - patch_jl as i64) / 4) as u32;
        self.mem
            .patch_u32(patch_jl, 0x5400000B | ((d_jl & 0x7FFFF) << 5));
        let d_done = ((done_label as i64 - patch_done as i64) / 4) as u32;
        self.mem
            .patch_u32(patch_done, 0x14000000 | (d_done & 0x03FF_FFFF));
    }

    /// Check if x0 holds a Smi (bit 0 = 1). If yes, fall through.
    /// If not, restore the JIT stack from `saved` registers, record
    /// NonSmiInput bailout, call bailout_helper, and return.
    /// `saved`: register indices of previously-popped values in chronological
    /// order (earliest first). Restored on bail after current x0.
    fn emit_smi_check(&mut self, bc_idx: usize, saved: &[u8]) {
        let patch_bail = self.mem.current_offset();
        emit(&mut self.mem, 0x36000000); // TBZ X0, #0, <0> (patched; branches if not Smi)
        let patch_ok = self.mem.current_offset();
        emit(&mut self.mem, 0x14000000); // B ok (patched)
        let bail_label = self.mem.current_offset();
        // Restore JIT stack: push x0 (current), then saved values
        self.push(); // push x0 (current failed check)
        for &reg in saved.iter() {
            mov_reg(&mut self.mem, 0, reg as u32);
            self.push();
        }
        self.record_bailout_point(bc_idx, BailoutReason::NonSmiInput);
        // Call bailout_helper(x0=vm_ptr, x1=bc_idx, x2=jit_sp)
        mov_reg(&mut self.mem, 2, JIT_STACK_REG);
        mov_imm64(&mut self.mem, 1, bc_idx as u64);
        mov_reg(&mut self.mem, 0, VM_REG);
        ldr_off(&mut self.mem, 15, VM_REG, 520);
        emit(&mut self.mem, 0xD63F01E0); // BLR x15
        movz(&mut self.mem, 0, 0);
        self.push();
        self.emit_epilogue();
        let ok_label = self.mem.current_offset();
        // Patch TBZ X0, #0: d = (bail_label - patch_bail) / 4
        let d = ((bail_label as i64 - patch_bail as i64) / 4) as u32;
        self.mem
            .patch_u32(patch_bail, 0x36000000 | ((d & 0x3FFF) << 5));
        // Patch B (unconditional)
        let d = ((ok_label as i64 - patch_ok as i64) / 4) as u32;
        self.mem.patch_u32(patch_ok, 0x14000000 | (d & 0x03FF_FFFF));
    }

    fn emit_prologue(&mut self) {
        push_callee_saved(&mut self.mem);
        mov_reg(&mut self.mem, VM_REG, 0);
        mov_reg(&mut self.mem, GC_REG, 1);
        mov_reg(&mut self.mem, LOC_REG, 2);
        add_imm(&mut self.mem, JIT_STACK_REG, VM_REG, self.jit_stack_offset);
        // Store initial JIT stack pointer as jit_stack_base (offset 576 from vm_ptr).
        // jit_stack[64] (512) + jit_helpers[8] (64) = 576
        str_off(&mut self.mem, JIT_STACK_REG, VM_REG, 576);
    }

    fn emit_epilogue(&mut self) {
        sub_imm(&mut self.mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        ldr_off(&mut self.mem, 0, JIT_STACK_REG, 0);
        pop_callee_saved(&mut self.mem);
        ret(&mut self.mem);
    }

    fn emit_b_cond(&mut self, cond: u32, target_bc: usize) -> usize {
        // B.cond: 0x54000000 | (imm19 << 5) | cond
        let patch_offset = self.mem.current_offset();
        let instr = 0x54000000 | cond;
        emit(&mut self.mem, instr);
        self.pending_patches.push((patch_offset, target_bc, instr));
        patch_offset
    }

    fn emit_b(&mut self, target_bc: usize) -> usize {
        // B: 0x14000000 | imm26
        let patch_offset = self.mem.current_offset();
        let instr = 0x14000000;
        emit(&mut self.mem, instr);
        self.pending_patches.push((patch_offset, target_bc, instr));
        patch_offset
    }

    fn resolve_patches(&mut self) {
        for &(patch_offset, bc_target, original_instr) in &self.pending_patches {
            let native_target = self.bc_to_native[bc_target];
            let from_addr = patch_offset as i64;
            let to_addr = native_target as i64;
            let instr = if (original_instr & 0xFF000000) == 0x14000000 {
                // Unconditional B: imm26 in bits [25:0], encoded as signed
                // offset in instructions.
                let imm = to_addr - from_addr;
                let imm_instr = (imm / 4) as i32;
                (original_instr & !0x03FF_FFFF) | ((imm_instr as u32) & 0x03FF_FFFF)
            } else {
                // B.cond: imm19 in bits [23:5].
                let imm = to_addr - from_addr;
                let imm_instr = (imm / 4) as i32;
                (original_instr & !0x00FF_FFE0) | (((imm_instr as u32) & 0x0007_FFFF) << 5)
            };
            self.mem.patch_u32(patch_offset, instr);
        }
        self.pending_patches.clear();
    }

    /// Emit a call site by inlining the callee's body.
    /// Called from the `Call` handler when `inline_plan` has an entry for this `call_bc_idx`.
    /// Stack effects (push/pop/stack_depth) are tracked identically to the normal call_helper
    /// path so that bailout information remains consistent.
    fn emit_inline_call(&mut self, call_bc_idx: usize, entry: &InlineEntry) {
        // Record the pre-call stack depth so we can restore the JIT stack
        // to pre-call state on bailout (F-3).
        let pre_call_depth = self.stack_depth;

        // Save caller's LOC_REG (x21) into x23.  Callee-saved x23 won't be
        // clobbered by any emitted instructions inside the inlined body.
        mov_reg(&mut self.mem, 23, 21);

        // Redirect LOC_REG to point at the callee's argument area on the JIT stack.
        // Non-named: local[0] = arg_0 at x22[-argc]
        // Named:     local[0] = callee at x22[-(argc+1)]
        let named = if entry.callee_named_function { 1 } else { 0 };
        let base_words = entry.argc + named; // words below x22 to reach local[0]
        // x21 = x22 - base_words * 8
        let base_bytes = base_words * 8;
        if base_bytes <= 0xFFF {
            sub_imm(&mut self.mem, 21, 22, base_bytes);
        } else {
            mov_imm64(&mut self.mem, 0, base_bytes as u64);
            sub_reg(&mut self.mem, 21, 22, 0);
        }

        // Resolve the callee's BytecodeProgram.
        let parent =
            unsafe { &*(entry.callee_prog_ptr as *const rune_bytecode::opcode::BytecodeProgram) };
        let callee_prog = &parent.functions[entry.callee_func_idx as usize];

        // Emit the callee's body instructions.  LoadLocal/StoreLocal access
        // via the redirected LOC_REG; arithmetic bailout points use the
        // caller's bc_idx so the bailout table maps to the right trace PC.
        // P25: The opcodes handled here must match the eligibility whitelist
        // at vm.rs:3652. Fix: move to pub fn is_inlineable_opcode() here.
        for instr in &callee_prog.instructions {
            match instr.opcode {
                Opcode::Return => {
                    // Pop the return value from the JIT stack.
                    self.pop();
                    // Restore caller's LOC_REG.
                    mov_reg(&mut self.mem, 21, 23);
                    // Remove this + callee + args from JIT stack.
                    // Do NOT update stack_depth here — matches normal call_helper path
                    // where sub_imm is used without a corresponding pop().
                    let pop_bytes = (entry.argc + 2) * 8;
                    sub_imm(&mut self.mem, 22, 22, pop_bytes);
                    // Push the return value back (stack_depth increments by 1).
                    self.push();
                    return;
                }
                Opcode::LoadLocal => {
                    let idx = instr.operands[0] as u32;
                    ldr_off(&mut self.mem, 0, 21, idx * 8);
                    self.push();
                }
                Opcode::StoreLocal => {
                    let idx = instr.operands[0] as u32;
                    self.pop();
                    str_off(&mut self.mem, 0, 21, idx * 8);
                    self.push();
                }
                Opcode::LoadSmi => {
                    let smi_raw = ((instr.operands[0] as u64) << 1) | 1;
                    mov_imm64(&mut self.mem, 0, smi_raw);
                    self.push();
                }
                Opcode::LoadUndefined => {
                    movz(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::LoadNull => {
                    movz(&mut self.mem, 0, 2);
                    self.push();
                }
                Opcode::LoadBoolean => {
                    let raw = if instr.operands[0] != 0 { 6u64 } else { 4u64 };
                    mov_imm64(&mut self.mem, 0, raw);
                    self.push();
                }
                Opcode::Pop => {
                    self.pop();
                }
                Opcode::Dup => {
                    sub_imm(&mut self.mem, JIT_STACK_REG, JIT_STACK_REG, 8);
                    ldr_off(&mut self.mem, 0, JIT_STACK_REG, 0);
                    add_imm(&mut self.mem, JIT_STACK_REG, JIT_STACK_REG, 8);
                    self.push();
                }
                Opcode::Swap => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    mov_reg(&mut self.mem, 2, 0);
                    mov_reg(&mut self.mem, 0, 1);
                    self.push();
                    mov_reg(&mut self.mem, 0, 2);
                    self.push();
                }
                Opcode::Add => {
                    self.pop(); // x0 = b
                    mov_reg(&mut self.mem, 9, 0); // x9 = b
                    self.pop(); // x0 = a
                    mov_reg(&mut self.mem, 8, 0); // x8 = a

                    // Smi fast path (bailout uses call_bc_idx)
                    mov_reg(&mut self.mem, 1, 9);
                    emit(&mut self.mem, 0x92400021); // AND x1, x1, #1
                    let b_not_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0xB4000001); // CBZ X1, +0
                    mov_reg(&mut self.mem, 1, 8);
                    emit(&mut self.mem, 0x92400021); // AND x1, x1, #1
                    let a_not_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0xB4000001); // CBZ X1, +0

                    mov_reg(&mut self.mem, 0, 8);
                    sub_imm(&mut self.mem, 0, 0, 1);
                    add_reg(&mut self.mem, 0, 0, 9);

                    emit(&mut self.mem, 0x9341FC02); // ASR x2, x0, #1
                    mov_imm64(&mut self.mem, 3, MAX_I31);
                    cmp_reg(&mut self.mem, 2, 3);
                    let ov_jg = self.mem.current_offset();
                    emit(&mut self.mem, 0x5400000C); // B.GT +0

                    mov_imm64(&mut self.mem, 3, MIN_I31);
                    cmp_reg(&mut self.mem, 2, 3);
                    let ov_jl = self.mem.current_offset();
                    emit(&mut self.mem, 0x5400000B); // B.LT +0

                    let smi_done = self.mem.current_offset();
                    emit(&mut self.mem, 0x14000000); // B +0

                    let float64_label = self.mem.current_offset();
                    mov_reg(&mut self.mem, 0, VM_REG);
                    mov_reg(&mut self.mem, 1, GC_REG);
                    mov_reg(&mut self.mem, 2, 8);
                    mov_reg(&mut self.mem, 3, 9);
                    ldr_off(&mut self.mem, 15, VM_REG, 552);
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15

                    let done_label = self.mem.current_offset();

                    let d_bns = ((float64_label as i64 - b_not_smi as i64) / 4) as u32;
                    self.mem
                        .patch_u32(b_not_smi, 0xB4000001 | ((d_bns & 0x7FFFF) << 5));
                    let d_ans = ((float64_label as i64 - a_not_smi as i64) / 4) as u32;
                    self.mem
                        .patch_u32(a_not_smi, 0xB4000001 | ((d_ans & 0x7FFFF) << 5));
                    let d_jg = ((float64_label as i64 - ov_jg as i64) / 4) as u32;
                    self.mem
                        .patch_u32(ov_jg, 0x5400000C | ((d_jg & 0x7FFFF) << 5));
                    let d_jl = ((float64_label as i64 - ov_jl as i64) / 4) as u32;
                    self.mem
                        .patch_u32(ov_jl, 0x5400000B | ((d_jl & 0x7FFFF) << 5));
                    let d_done = ((done_label as i64 - smi_done as i64) / 4) as u32;
                    self.mem
                        .patch_u32(smi_done, 0x14000000 | (d_done & 0x03FF_FFFF));

                    self.push();
                }
                Opcode::Sub => {
                    self.pop();
                    mov_reg(&mut self.mem, 9, 0);
                    self.pop();
                    mov_reg(&mut self.mem, 8, 0);
                    // Smi fast path: both operands Smi
                    mov_reg(&mut self.mem, 1, 9);
                    emit(&mut self.mem, 0x92400021);
                    let b_not_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0xB4000001);
                    mov_reg(&mut self.mem, 1, 8);
                    emit(&mut self.mem, 0x92400021);
                    let a_not_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0xB4000001);
                    sub_reg(&mut self.mem, 0, 8, 9);
                    add_imm(&mut self.mem, 0, 0, 1);
                    // Overflow check
                    emit(&mut self.mem, 0x9341FC02);
                    mov_imm64(&mut self.mem, 3, MAX_I31);
                    cmp_reg(&mut self.mem, 2, 3);
                    let ov_jg = self.mem.current_offset();
                    emit(&mut self.mem, 0x5400000C);
                    mov_imm64(&mut self.mem, 3, MIN_I31);
                    cmp_reg(&mut self.mem, 2, 3);
                    let ov_jl = self.mem.current_offset();
                    emit(&mut self.mem, 0x5400000B);
                    let smi_done = self.mem.current_offset();
                    emit(&mut self.mem, 0x14000000);
                    // Bailout: non-Smi input or overflow
                    let bail_label = self.mem.current_offset();
                    {
                        let d_bns = ((bail_label as i64 - b_not_smi as i64) / 4) as u32;
                        self.mem.patch_u32(b_not_smi, 0xB4000001 | ((d_bns & 0x7FFFF) << 5));
                        let d_ans = ((bail_label as i64 - a_not_smi as i64) / 4) as u32;
                        self.mem.patch_u32(a_not_smi, 0xB4000001 | ((d_ans & 0x7FFFF) << 5));
                        let d_jg = ((bail_label as i64 - ov_jg as i64) / 4) as u32;
                        self.mem.patch_u32(ov_jg, 0x5400000C | ((d_jg & 0x7FFFF) << 5));
                        let d_jl = ((bail_label as i64 - ov_jl as i64) / 4) as u32;
                        self.mem.patch_u32(ov_jl, 0x5400000B | ((d_jl & 0x7FFFF) << 5));
                    }
                    self.emit_inline_bailout(call_bc_idx, BailoutReason::Overflow, pre_call_depth);
                    let done_label = self.mem.current_offset();
                    let d_done = ((done_label as i64 - smi_done as i64) / 4) as u32;
                    self.mem.patch_u32(smi_done, 0x14000000 | (d_done & 0x03FF_FFFF));
                    self.push();
                }
                _ => {
                    // Unsupported opcode in inline context — bail at the call site.
                    // This shouldn't happen for eligible callees (checked at plan-build time).
                    self.record_bailout_point(call_bc_idx, BailoutReason::Unimplemented);
                    mov_reg(&mut self.mem, 2, JIT_STACK_REG);
                    mov_imm64(&mut self.mem, 1, call_bc_idx as u64);
                    mov_reg(&mut self.mem, 0, VM_REG);
                    ldr_off(&mut self.mem, 15, VM_REG, 520);
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    movz(&mut self.mem, 0, 0);
                    self.push();
                    self.emit_epilogue();
                    return;
                }
            }
        }
    }

    /// Emit a bailout sequence for an inlined callee that restores the JIT stack
    /// to the pre-call level before calling `bailout_helper`.  This ensures the
    /// captured stack snapshot has only the pre-call state (args + this + callee),
    /// so the interpreter can re-execute the Call instruction correctly (F-3).
    fn emit_inline_bailout(&mut self, call_bc_idx: usize, reason: BailoutReason, pre_depth: u32) {
        let delta = self.stack_depth.saturating_sub(pre_depth);
        self.record_bailout_point(call_bc_idx, reason);
        if delta > 0 {
            let bytes = delta * 8;
            if bytes <= 0xFFF {
                sub_imm(&mut self.mem, 22, 22, bytes);
            } else {
                mov_imm64(&mut self.mem, 0, bytes as u64);
                sub_reg(&mut self.mem, 22, 22, 0);
            }
        }
        mov_reg(&mut self.mem, 2, JIT_STACK_REG);
        mov_imm64(&mut self.mem, 1, call_bc_idx as u64);
        mov_reg(&mut self.mem, 0, VM_REG);
        ldr_off(&mut self.mem, 15, VM_REG, 520);
        emit(&mut self.mem, 0xD63F01E0);
        movz(&mut self.mem, 0, 0);
        self.push();
        self.emit_epilogue();
    }

    pub fn compile(mut self, program: &rune_bytecode::opcode::BytecodeProgram) -> CompiledFunction {
        self.emit_prologue();

        if self.stencil_jit {
            // No setup needed — each stencil inlines the push body directly.
        }

        for (bc_idx, instr) in program.instructions.iter().enumerate() {
            self.bc_to_native[bc_idx] = self.mem.current_offset();
            match instr.opcode {
                Opcode::LoadSmi => {
                    let smi_raw = ((instr.operands[0] as u64) << 1) | 1;
                    if self.stencil_jit {
                        // Inlined stencil: emit MOVZ from the stencil bytes, fix the value hole,
                        // then emit STR+ADD inline (no branch, no RET — the push runs immediately).
                        let use_32 = (smi_raw >> 16) != 0;
                        let stencil_start = self.mem.current_offset();
                        // Emit MOVZ from stencil bytes, then patch to 64-bit (sf=1).
                        if use_32 {
                            for &b in &LOAD_SMI_32_BYTES[..8] { self.mem.emit_byte(b); }
                        } else {
                            for &b in &LOAD_SMI_16_BYTES[..4] { self.mem.emit_byte(b); }
                        }
                        // Patch value hole(s): 64-bit MOVZ/MOVK (sf=1, bit31=1)
                        let lo16 = smi_raw as u32 & 0xFFFF;
                        // 0xD2800000 = MOVZ X0, #0, LSL #0 (64-bit, sf=1)
                        self.mem.patch_u32(stencil_start, 0xD2800000u32 | (lo16 << 5));
                        if use_32 {
                            let hi16 = (smi_raw as u32 >> 16) & 0xFFFF;
                            // 0xF2A00000 = MOVK X0, #?, LSL #16 (64-bit, sf=1)
                            self.mem.patch_u32(stencil_start + 4, 0xF2A00000u32 | (hi16 << 5));
                        }
                        // Emit STR+ADD inline (the push operation)
                        for &b in RUNE_PUSH_HELPER.bytes { self.mem.emit_byte(b); }
                        self.stack_depth += 1;
                    } else {
                        mov_imm64(&mut self.mem, 0, smi_raw);
                        self.push();
                    }
                }
                Opcode::LoadUndefined => {
                    movz(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::LoadNull => {
                    movz(&mut self.mem, 0, 2);
                    self.push();
                }
                Opcode::LoadBoolean => {
                    let raw = if instr.operands[0] != 0 { 6u64 } else { 4u64 };
                    mov_imm64(&mut self.mem, 0, raw);
                    self.push();
                }
                Opcode::LoadFloat64 => {
                    let idx = instr.operands[0] as usize;
                    let val = program.float_pool.get(idx).copied().unwrap_or(0.0);
                    let i = val as i64;
                    let smi_raw = ((i as u64) << 1) | 1;
                    mov_imm64(&mut self.mem, 0, smi_raw);
                    self.push();
                }
                Opcode::LoadStringConst => {
                    let string_idx = instr.operands[0] as u64;
                    let prog_ptr = program as *const rune_bytecode::opcode::BytecodeProgram
                        as *const u8 as u64;
                    // x0 = x19 (vm_ptr), x1 = x20 (gc_ptr)
                    mov_reg(&mut self.mem, 0, VM_REG);
                    mov_reg(&mut self.mem, 1, GC_REG);
                    // x2 = prog_ptr (immediate), x3 = string_idx (immediate)
                    mov_imm64(&mut self.mem, 2, prog_ptr);
                    mov_imm64(&mut self.mem, 3, string_idx);
                    // Load string_helper from [x19 + 536] into x15
                    ldr_off(&mut self.mem, 15, VM_REG, 536);
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    self.push(); // push result (x0)
                }
                Opcode::LoadLocal => {
                    let idx = instr.operands[0] as u32;
                    ldr_off(&mut self.mem, 0, LOC_REG, idx * 8);
                    self.push();
                }
                Opcode::StoreLocal => {
                    let idx = instr.operands[0] as u32;
                    self.pop();
                    str_off(&mut self.mem, 0, LOC_REG, idx * 8);
                    self.push();
                }
                Opcode::Pop => {
                    self.pop();
                }
                Opcode::Dup => {
                    // peek top without popping
                    sub_imm(&mut self.mem, JIT_STACK_REG, JIT_STACK_REG, 8);
                    ldr_off(&mut self.mem, 0, JIT_STACK_REG, 0);
                    add_imm(&mut self.mem, JIT_STACK_REG, JIT_STACK_REG, 8);
                    self.push();
                }
                Opcode::Add => {
                    self.pop(); // x0 = b
                    mov_reg(&mut self.mem, 9, 0); // x9 = b (save)
                    self.pop(); // x0 = a
                    mov_reg(&mut self.mem, 8, 0); // x8 = a (save)

                    // === Smi fast path: both operands Smi ===
                    // Check b is Smi (bit 0) without destroying x9
                    mov_reg(&mut self.mem, 1, 9);
                    emit(&mut self.mem, 0x92400021); // AND x1, x1, #1
                    let b_not_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0xB4000001); // CBZ X1, +0 (b not Smi → float64)

                    // Check a is Smi (bit 0) without destroying x8
                    mov_reg(&mut self.mem, 1, 8);
                    emit(&mut self.mem, 0x92400021); // AND x1, x1, #1
                    let a_not_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0xB4000001); // CBZ X1, +0 (a not Smi → float64)

                    // Both Smi: do Smi add
                    mov_reg(&mut self.mem, 0, 8); // x0 = a
                    sub_imm(&mut self.mem, 0, 0, 1); // untag a: x0 = a - 1 = a_untag*2
                    add_reg(&mut self.mem, 0, 0, 9); // add b (tagged) → Smi result

                    // Check overflow
                    emit(&mut self.mem, 0x9341FC02); // ASR x2, x0, #1 (untag result)
                    mov_imm64(&mut self.mem, 3, MAX_I31);
                    cmp_reg(&mut self.mem, 2, 3);
                    let ov_jg = self.mem.current_offset();
                    emit(&mut self.mem, 0x5400000C); // B.GT +0 (overflow → float64)

                    mov_imm64(&mut self.mem, 3, MIN_I31);
                    cmp_reg(&mut self.mem, 2, 3);
                    let ov_jl = self.mem.current_offset();
                    emit(&mut self.mem, 0x5400000B); // B.LT +0 (overflow → float64)

                    // No overflow — Smi result in x0, skip helper
                    let smi_done = self.mem.current_offset();
                    emit(&mut self.mem, 0x14000000); // B +0

                    // === Float64 path (non-Smi input or overflow) ===
                    let float64_label = self.mem.current_offset();

                    // Call float64_add_helper(vm_ptr=x19, gc_ptr=x20, a_raw=x8, b_raw=x9)
                    mov_reg(&mut self.mem, 0, VM_REG);
                    mov_reg(&mut self.mem, 1, GC_REG);
                    mov_reg(&mut self.mem, 2, 8); // a_raw
                    mov_reg(&mut self.mem, 3, 9); // b_raw
                    ldr_off(&mut self.mem, 15, VM_REG, 552); // float64_add_helper
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15

                    // === Done: push result (Smi or float64) ===
                    let done_label = self.mem.current_offset();

                    // Patch: b_not_smi → float64_label
                    let d_bns = ((float64_label as i64 - b_not_smi as i64) / 4) as u32;
                    self.mem
                        .patch_u32(b_not_smi, 0xB4000001 | ((d_bns & 0x7FFFF) << 5));
                    // Patch: a_not_smi → float64_label
                    let d_ans = ((float64_label as i64 - a_not_smi as i64) / 4) as u32;
                    self.mem
                        .patch_u32(a_not_smi, 0xB4000001 | ((d_ans & 0x7FFFF) << 5));
                    // Patch: ov_jg → float64_label
                    let d_jg = ((float64_label as i64 - ov_jg as i64) / 4) as u32;
                    self.mem
                        .patch_u32(ov_jg, 0x5400000C | ((d_jg & 0x7FFFF) << 5));
                    // Patch: ov_jl → float64_label
                    let d_jl = ((float64_label as i64 - ov_jl as i64) / 4) as u32;
                    self.mem
                        .patch_u32(ov_jl, 0x5400000B | ((d_jl & 0x7FFFF) << 5));
                    // Patch: smi_done → done_label
                    let d_done = ((done_label as i64 - smi_done as i64) / 4) as u32;
                    self.mem
                        .patch_u32(smi_done, 0x14000000 | (d_done & 0x03FF_FFFF));

                    self.push();
                }
                Opcode::Sub => {
                    self.pop(); // x0 = b
                    mov_reg(&mut self.mem, 9, 0); // x9 = b
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop(); // x0 = a
                    mov_reg(&mut self.mem, 8, 0); // x8 = a
                    self.emit_smi_check(bc_idx, &[9]); // check a; saved=[x9(b)]
                    sub_reg(&mut self.mem, 0, 0, 1);
                    add_imm(&mut self.mem, 0, 0, 1); // retag
                    self.emit_smi_overflow_bailout_or_continue(bc_idx, Some(8), Some(9));
                    self.push();
                }
                Opcode::Mul => {
                    self.pop(); // x0 = b
                    mov_reg(&mut self.mem, 9, 0); // x9 = b
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop(); // x0 = a
                    mov_reg(&mut self.mem, 8, 0); // x8 = a
                    self.emit_smi_check(bc_idx, &[9]); // check a; saved=[x9(b)]
                    emit(&mut self.mem, 0x9341FC00); // ASR x0, x0, #1
                    emit(&mut self.mem, 0x9341FC21); // ASR x1, x1, #1
                    emit(&mut self.mem, 0x9B017C00); // MUL x0, x0, x1
                    emit(&mut self.mem, 0xD37FF800); // LSL x0, x0, #1
                    add_imm(&mut self.mem, 0, 0, 1);
                    self.emit_smi_overflow_bailout_or_continue(bc_idx, Some(8), Some(9));
                    self.push();
                }
                Opcode::Mod => {
                    self.pop();
                    mov_reg(&mut self.mem, 9, 0);
                    self.emit_smi_check(bc_idx, &[]);
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    mov_reg(&mut self.mem, 8, 0);
                    self.emit_smi_check(bc_idx, &[9]);
                    emit(&mut self.mem, 0x9341FC21); // ASR x1, x1, #1
                    let div_by_zero = self.mem.current_offset();
                    emit(&mut self.mem, 0xB4000001); // CBZ x1, +0
                    emit(&mut self.mem, 0x9341FC00); // ASR x0, x0, #1
                    emit(&mut self.mem, 0x9AC10C02); // SDIV x2, x0, x1
                    emit(&mut self.mem, 0x9B018040); // MSUB x0, x2, x1, x0
                    emit(&mut self.mem, 0xD37FF800); // LSL x0, x0, #1
                    add_imm(&mut self.mem, 0, 0, 1);
                    let mod_ok = self.mem.current_offset();
                    emit(&mut self.mem, 0x14000000); // B push
                    let div_by_zero_label = self.mem.current_offset();
                    mov_reg(&mut self.mem, 0, 8);
                    self.push();
                    mov_reg(&mut self.mem, 0, 9);
                    self.push();
                    self.record_bailout_point(bc_idx, BailoutReason::NonSmiInput);
                    mov_reg(&mut self.mem, 2, JIT_STACK_REG);
                    mov_imm64(&mut self.mem, 1, bc_idx as u64);
                    mov_reg(&mut self.mem, 0, VM_REG);
                    ldr_off(&mut self.mem, 15, VM_REG, 520);
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    movz(&mut self.mem, 0, 0);
                    self.push();
                    self.emit_epilogue();
                    let push_label = self.mem.current_offset();
                    let d = ((div_by_zero_label as i64 - div_by_zero as i64) / 4) as u32;
                    self.mem
                        .patch_u32(div_by_zero, 0xB4000001 | ((d & 0x7FFFF) << 5));
                    let d = ((push_label as i64 - mod_ok as i64) / 4) as u32;
                    self.mem.patch_u32(mod_ok, 0x14000000 | (d & 0x03FF_FFFF));
                    self.push();
                }
                Opcode::Lt => {
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    cmp_reg(&mut self.mem, 0, 1);
                    // CSET x0, LT = CSINC x0, XZR, XZR, GE (= !LT)
                    emit(&mut self.mem, 0x9A9FA7E0);
                    emit(&mut self.mem, 0xD37FF800); // LSL x0, x0, #1
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::Gt => {
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    cmp_reg(&mut self.mem, 0, 1);
                    // CSET x0, GT = CSINC x0, XZR, XZR, LE (= !GT)
                    emit(&mut self.mem, 0x9A9FD7E0);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::Le => {
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    cmp_reg(&mut self.mem, 0, 1);
                    // CSET x0, LE = CSINC x0, XZR, XZR, GT (= !LE)
                    emit(&mut self.mem, 0x9A9FC7E0);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::Ge => {
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    cmp_reg(&mut self.mem, 0, 1);
                    // CSET x0, GE = CSINC x0, XZR, XZR, LT (= !GE)
                    emit(&mut self.mem, 0x9A9FB7E0);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::StrictEq => {
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    cmp_reg(&mut self.mem, 0, 1);
                    // CSET x0, EQ = CSINC x0, XZR, XZR, NE (= !EQ)
                    emit(&mut self.mem, 0x9A9F17E0);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::Shl => {
                    self.pop(); // x0 = b
                    mov_reg(&mut self.mem, 9, 0); // x9 = b
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop(); // x0 = a
                    mov_reg(&mut self.mem, 8, 0); // x8 = a
                    self.emit_smi_check(bc_idx, &[9]); // check a; saved=[x9(b)]
                    // Untag both: ASR #1 decodes Smi → int32
                    emit(&mut self.mem, 0x9341FC00); // ASR x0, x0, #1
                    emit(&mut self.mem, 0x9341FC21); // ASR x1, x1, #1
                    lsl_reg(&mut self.mem, 0, 0, 1); // LSL x0, x0, x1
                    // Retag: LSL #1; ORR #1
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.emit_smi_overflow_bailout_or_continue(bc_idx, Some(8), Some(9));
                    self.push();
                }
                Opcode::Shr => {
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    emit(&mut self.mem, 0x9341FC00);
                    emit(&mut self.mem, 0x9341FC21);
                    asr_reg(&mut self.mem, 0, 0, 1); // ASR x0, x0, x1
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::BitAnd => {
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    emit(&mut self.mem, 0x9341FC00);
                    emit(&mut self.mem, 0x9341FC21);
                    and_reg(&mut self.mem, 0, 0, 1);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::BitOr => {
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    emit(&mut self.mem, 0x9341FC00);
                    emit(&mut self.mem, 0x9341FC21);
                    orr_reg(&mut self.mem, 0, 0, 1);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::BitXor => {
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    emit(&mut self.mem, 0x9341FC00);
                    emit(&mut self.mem, 0x9341FC21);
                    eor_reg(&mut self.mem, 0, 0, 1);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::ShrU => {
                    // Smi unsigned right shift (>>>)
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0); // x1 = b
                    self.pop(); // x0 = a
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    emit(&mut self.mem, 0x9341FC00); // ASR x0, x0, #1 (untag a)
                    emit(&mut self.mem, 0x9341FC21); // ASR x1, x1, #1 (untag b)
                    lsr_reg(&mut self.mem, 0, 0, 1); // LSR x0, x0, x1 (unsigned shift)
                    emit(&mut self.mem, 0xD37FF800); // LSL x0, x0, #1 (retag)
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::Swap => {
                    self.pop(); // x0 = top (b)
                    mov_reg(&mut self.mem, 1, 0); // x1 = b
                    self.pop(); // x0 = second (a)
                    mov_reg(&mut self.mem, 2, 0); // x2 = a
                    mov_reg(&mut self.mem, 0, 1); // x0 = b
                    self.push();
                    mov_reg(&mut self.mem, 0, 2); // x0 = a
                    self.push();
                }
                Opcode::Eq => {
                    // Abstract equality for Smi/sentinel values. Branchless CSET-based.
                    self.pop(); // x0 = b
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0); // x1 = b
                    self.pop(); // x0 = a
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    mov_reg(&mut self.mem, 2, 31); // x2 = 0 (result accumulator)
                    // a == b
                    cmp_reg(&mut self.mem, 0, 1);
                    emit(&mut self.mem, 0x9A9F07E3); // CSET x3, EQ
                    orr_reg(&mut self.mem, 2, 2, 3);
                    // null == undefined: (a==0 && b==2)
                    movz(&mut self.mem, 3, 0);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E4); // CSET x4, EQ
                    movz(&mut self.mem, 3, 2);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E5); // CSET x5, EQ
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    // null == undefined: (a==2 && b==0)
                    movz(&mut self.mem, 3, 2);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E4);
                    movz(&mut self.mem, 3, 0);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E5);
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    // a == false(4): b == Smi(0)=1
                    movz(&mut self.mem, 3, 4);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E4);
                    movz(&mut self.mem, 3, 1);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E5);
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    // a == true(6): b == Smi(1)=3
                    movz(&mut self.mem, 3, 6);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E4);
                    movz(&mut self.mem, 3, 3);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E5);
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    // b == false(4): a == Smi(0)=1
                    movz(&mut self.mem, 3, 4);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E4);
                    movz(&mut self.mem, 3, 1);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E5);
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    // b == true(6): a == Smi(1)=3
                    movz(&mut self.mem, 3, 6);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E4);
                    movz(&mut self.mem, 3, 3);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E5);
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    // Smi-encode result (x2 = 0 or 1)
                    emit(&mut self.mem, 0xD37FF842); // LSL x2, x2, #1
                    orr_imm1(&mut self.mem, 2, 2);
                    mov_reg(&mut self.mem, 0, 2);
                    self.push();
                }
                Opcode::Ne => {
                    // Same as Eq, then invert result
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    mov_reg(&mut self.mem, 2, 31);
                    cmp_reg(&mut self.mem, 0, 1);
                    emit(&mut self.mem, 0x9A9F07E3);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    movz(&mut self.mem, 3, 0);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E4);
                    movz(&mut self.mem, 3, 2);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E5);
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    movz(&mut self.mem, 3, 2);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E4);
                    movz(&mut self.mem, 3, 0);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E5);
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    movz(&mut self.mem, 3, 4);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E4);
                    movz(&mut self.mem, 3, 1);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E5);
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    movz(&mut self.mem, 3, 6);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E4);
                    movz(&mut self.mem, 3, 3);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E5);
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    movz(&mut self.mem, 3, 4);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E4);
                    movz(&mut self.mem, 3, 1);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E5);
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    movz(&mut self.mem, 3, 6);
                    cmp_reg(&mut self.mem, 1, 3);
                    emit(&mut self.mem, 0x9A9F07E4);
                    movz(&mut self.mem, 3, 3);
                    cmp_reg(&mut self.mem, 0, 3);
                    emit(&mut self.mem, 0x9A9F07E5);
                    and_reg(&mut self.mem, 3, 4, 5);
                    orr_reg(&mut self.mem, 2, 2, 3);
                    // Invert: x2 = 1 - x2
                    movz(&mut self.mem, 3, 1);
                    eor_reg(&mut self.mem, 2, 2, 3);
                    // Smi-encode
                    emit(&mut self.mem, 0xD37FF842);
                    orr_imm1(&mut self.mem, 2, 2);
                    mov_reg(&mut self.mem, 0, 2);
                    self.push();
                }
                Opcode::IncLocal => {
                    let idx = instr.operands[0] as u32;
                    let is_prefix = instr.operands.get(1).copied().unwrap_or(0) != 0;
                    ldr_off(&mut self.mem, 0, LOC_REG, idx * 8);
                    if is_prefix {
                        add_imm(&mut self.mem, 0, 0, 2);
                        str_off(&mut self.mem, 0, LOC_REG, idx * 8);
                        self.push();
                    } else {
                        mov_reg(&mut self.mem, 2, 0); // x2 = old
                        add_imm(&mut self.mem, 0, 0, 2);
                        str_off(&mut self.mem, 0, LOC_REG, idx * 8);
                        mov_reg(&mut self.mem, 0, 2);
                        self.push();
                    }
                }
                Opcode::DecLocal => {
                    let idx = instr.operands[0] as u32;
                    let is_prefix = instr.operands.get(1).copied().unwrap_or(0) != 0;
                    ldr_off(&mut self.mem, 0, LOC_REG, idx * 8);
                    if is_prefix {
                        sub_imm(&mut self.mem, 0, 0, 2);
                        str_off(&mut self.mem, 0, LOC_REG, idx * 8);
                        self.push();
                    } else {
                        mov_reg(&mut self.mem, 2, 0);
                        sub_imm(&mut self.mem, 0, 0, 2);
                        str_off(&mut self.mem, 0, LOC_REG, idx * 8);
                        mov_reg(&mut self.mem, 0, 2);
                        self.push();
                    }
                }
                Opcode::Jump => {
                    let target = instr.operands[0] as usize;
                    self.emit_b(target);
                }
                Opcode::JumpIfFalse => {
                    let target = instr.operands[0] as usize;
                    self.pop(); // x0 = condition
                    self.emit_smi_check(bc_idx, &[]); // check condition is Smi
                    movz(&mut self.mem, 1, 2); // x1 = 2 (null sentinel)
                    cmp_reg(&mut self.mem, 0, 1);
                    self.emit_b_cond(0x9, target); // B.LS target (falsy: ≤ 2)
                    movz(&mut self.mem, 1, 4); // x1 = 4 (false sentinel)
                    cmp_reg(&mut self.mem, 0, 1);
                    self.emit_b_cond(0x0, target); // B.EQ target (falsy: == 4)
                }
                Opcode::JumpIfTrue => {
                    let target = instr.operands[0] as usize;
                    self.pop(); // x0 = condition
                    self.emit_smi_check(bc_idx, &[]); // check condition is Smi
                    movz(&mut self.mem, 1, 2);
                    cmp_reg(&mut self.mem, 0, 1);
                    emit(&mut self.mem, 0x54000049); // B.LS +2 (falsy: skip B)
                    movz(&mut self.mem, 1, 4);
                    cmp_reg(&mut self.mem, 0, 1);
                    emit(&mut self.mem, 0x54000020); // B.EQ +1 (falsy: skip B)
                    self.emit_b(target);
                }
                Opcode::LoadProperty => {
                    // Computed property access: `obj[key]`.
                    // Fast path: dense array + Smi key → direct element load.
                    // Miss: restore JIT stack, bail to interpreter.
                    self.pop(); // x0 = key
                    mov_reg(&mut self.mem, 7, 0); // x7 = key
                    self.pop(); // x0 = obj
                    mov_reg(&mut self.mem, 1, 0); // x1 = obj
                    // === Fast path: dense array + Smi key ===
                    movz(&mut self.mem, 2, 1); // x2 = 1
                    emit(&mut self.mem, 0xEA02003F); // TST x1, x2
                    let patch_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000001); // B.NE miss (obj is Smi)
                    emit(&mut self.mem, 0xF100183F); // CMP x1, #6
                    let patch_sentinel = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000009); // B.LS miss (obj ≤ 6)
                    ldr_off(&mut self.mem, 3, 1, 0); // x3 = [x1] = header
                    mov_imm64(&mut self.mem, 4, 7); // x4 = 7
                    emit(&mut self.mem, 0x8A040063); // AND x3, x3, x4 = tag
                    emit(&mut self.mem, 0xF100107F); // CMP x3, #4
                    let patch_tag = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000001); // B.NE miss (not array)
                    emit(&mut self.mem, 0xEA0200FF); // TST x7, x2
                    let patch_key_not_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000000); // B.EQ miss (key not Smi)
                    emit(&mut self.mem, 0xD341FCE6); // LSR x6, x7, #1 = index
                    ldr_off(&mut self.mem, 4, 1, 16); // x4 = [x1+16] = len|cap
                    emit(&mut self.mem, 0x6B06009F); // CMP w4, w6
                    let patch_oob = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000009); // B.LS miss (index ≥ len)
                    add_imm(&mut self.mem, 5, 1, 32); // x5 = x1 + 32 (elements)
                    emit(&mut self.mem, 0xF86678A0); // LDR x0, [x5, x6, LSL #3]
                    let b_done = self.mem.current_offset();
                    emit(&mut self.mem, 0x14000000); // B done (skip miss path)
                    // === Miss: restore stack, bail to interpreter ===
                    let miss_offset = self.mem.current_offset();
                    mov_reg(&mut self.mem, 0, 1);
                    self.push();
                    mov_reg(&mut self.mem, 0, 7);
                    self.push();
                    self.record_bailout_point(bc_idx, BailoutReason::ShapeMiss);
                    mov_reg(&mut self.mem, 2, JIT_STACK_REG);
                    mov_imm64(&mut self.mem, 1, bc_idx as u64);
                    mov_reg(&mut self.mem, 0, VM_REG);
                    ldr_off(&mut self.mem, 15, VM_REG, 520);
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    movz(&mut self.mem, 0, 0);
                    self.push();
                    self.emit_epilogue();
                    // done: push result (from fast path or bailout) and continue
                    let done_offset = self.mem.current_offset();
                    let d = ((done_offset as i64 - b_done as i64) / 4) as u32;
                    self.mem.patch_u32(b_done, 0x14000000 | (d & 0x03FF_FFFF));
                    // Patch miss branches
                    let d = ((miss_offset as i64 - patch_smi as i64) / 4) as u32;
                    self.mem
                        .patch_u32(patch_smi, 0x54000001 | ((d & 0x7FFFF) << 5));
                    let d = ((miss_offset as i64 - patch_sentinel as i64) / 4) as u32;
                    self.mem
                        .patch_u32(patch_sentinel, 0x54000009 | ((d & 0x7FFFF) << 5));
                    let d = ((miss_offset as i64 - patch_tag as i64) / 4) as u32;
                    self.mem
                        .patch_u32(patch_tag, 0x54000001 | ((d & 0x7FFFF) << 5));
                    let d = ((miss_offset as i64 - patch_key_not_smi as i64) / 4) as u32;
                    self.mem
                        .patch_u32(patch_key_not_smi, 0x54000000 | ((d & 0x7FFFF) << 5));
                    let d = ((miss_offset as i64 - patch_oob as i64) / 4) as u32;
                    self.mem
                        .patch_u32(patch_oob, 0x54000009 | ((d & 0x7FFFF) << 5));
                    self.push();
                }
                Opcode::LoadPropertyIC => {
                    let shape_id = instr.operands[0] as u64;
                    let offset = instr.operands[1] as u32;
                    let proto_depth = instr.operands.get(2).copied().unwrap_or(0) as u32;
                    let ic_table = self.ic_tables.get(bc_idx).filter(|t| t.count > 1).cloned();
                    self.pop(); // x0 = key
                    mov_reg(&mut self.mem, 7, 0); // x7 = key (saved for miss path)
                    self.pop(); // x0 = object
                    mov_reg(&mut self.mem, 1, 0); // x1 = object
                    movz(&mut self.mem, 2, 1);
                    emit(&mut self.mem, 0xEA02003F); // TST x1, x2
                    let patch_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000001); // B.NE +0
                    emit(&mut self.mem, 0xF100183F); // CMP x1, #6
                    let patch_sentinel = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000009); // B.LS +0
                    ldr_off(&mut self.mem, 2, 1, 8); // x2 = [x1 + 8]
                    ldr_off(&mut self.mem, 3, 2, 0); // x3 = [x2]
                    let mut miss_patches: Vec<(usize, u32)> = Vec::new();
                    let mut done_patches: Vec<usize> = Vec::new();
                    if let Some(table) = ic_table {
                        // === Polymorphic: N-entry scalar scan ===
                        let n = table.count;
                        let adr_off = self.mem.current_offset();
                        emit(&mut self.mem, 0x10000000 | 4); // ADR x4, 0 (placeholder)
                        let mut eq_offsets: Vec<usize> = Vec::with_capacity(n);
                        for i in 0..n {
                            let base = (i as u32) * 16;
                            ldr_off(&mut self.mem, 5, 4, base); // x5 = shape_id[i]
                            ldr_off(&mut self.mem, 6, 4, base + 8); // x6 = slot_offset[i]
                            cmp_reg(&mut self.mem, 5, 3);
                            eq_offsets.push(self.mem.current_offset());
                            emit(&mut self.mem, 0x54000000); // B.EQ +0 (→ found)
                        }
                        let b_miss = self.mem.current_offset();
                        emit(&mut self.mem, 0x14000000); // B +0 (→ miss)
                        let found_off = self.mem.current_offset();
                        // Patch B.EQ entries → found_off (known now)
                        for &eq in &eq_offsets {
                            let d = ((found_off as i64 - eq as i64) / 4) as u32;
                            self.mem.patch_u32(eq, 0x54000000 | ((d & 0x7FFFF) << 5));
                        }
                        miss_patches.push((b_miss, 0x14000000)); // B → miss (deferred)
                        if proto_depth > 0 {
                            for _ in 0..proto_depth {
                                ldr_off(&mut self.mem, 1, 1, 24);
                            }
                        }
                        ldr_reg(&mut self.mem, 0, 1, 6); // x0 = [x1 + x6]
                        self.push();
                        done_patches.push(self.mem.current_offset());
                        emit(&mut self.mem, 0x14000000); // B → done
                        self.ic_table_patches.push(IcTablePatch {
                            adr_offset: adr_off,
                            table,
                        });
                    } else {
                        // === Monomorphic: single shape_id guard ===
                        mov_imm64(&mut self.mem, 4, shape_id);
                        cmp_reg(&mut self.mem, 3, 4);
                        miss_patches.push((self.mem.current_offset(), 0x54000001)); // B.NE → miss
                        emit(&mut self.mem, 0x54000001);
                        if proto_depth > 0 {
                            for _ in 0..proto_depth {
                                ldr_off(&mut self.mem, 1, 1, 24);
                            }
                        }
                        ldr_off(&mut self.mem, 0, 1, 32 + offset * 8);
                        self.push();
                        done_patches.push(self.mem.current_offset());
                        emit(&mut self.mem, 0x14000000); // B → done
                    }
                    // === Miss handler ===
                    let miss_offset = self.mem.current_offset();
                    for &(patch, orig) in &miss_patches {
                        let d = ((miss_offset as i64 - patch as i64) / 4) as u32;
                        if (orig & 0xFF000000) == 0x14000000 {
                            self.mem.patch_u32(patch, 0x14000000 | (d & 0x03FF_FFFF));
                        } else {
                            self.mem
                                .patch_u32(patch, (orig & !0x00FF_FFE0) | ((d & 0x7FFFF) << 5));
                        }
                    }
                    self.record_bailout_point(bc_idx, BailoutReason::ShapeMiss);
                    mov_reg(&mut self.mem, 0, 1);
                    self.push();
                    mov_reg(&mut self.mem, 0, 7);
                    self.push();
                    mov_reg(&mut self.mem, 2, JIT_STACK_REG);
                    mov_imm64(&mut self.mem, 1, bc_idx as u64);
                    mov_reg(&mut self.mem, 0, VM_REG);
                    ldr_off(&mut self.mem, 15, VM_REG, 520);
                    emit(&mut self.mem, 0xD63F01E0);
                    movz(&mut self.mem, 0, 0);
                    self.push();
                    self.emit_epilogue();
                    let done_offset = self.mem.current_offset();
                    for &patch in &done_patches {
                        let d = ((done_offset as i64 - patch as i64) / 4) as u32;
                        self.mem.patch_u32(patch, 0x14000000 | (d & 0x03FF_FFFF));
                    }
                    let d = ((miss_offset as i64 - patch_smi as i64) / 4) as u32;
                    self.mem
                        .patch_u32(patch_smi, 0x54000001 | ((d & 0x7FFFF) << 5));
                    let d = ((miss_offset as i64 - patch_sentinel as i64) / 4) as u32;
                    self.mem
                        .patch_u32(patch_sentinel, 0x54000009 | ((d & 0x7FFFF) << 5));
                }
                Opcode::StorePropertyIC => {
                    let shape_id = instr.operands[0] as u64;
                    let offset = instr.operands[1] as u32;
                    let _proto_depth = instr.operands.get(2).copied().unwrap_or(0) as u32;
                    let ic_table = self.ic_tables.get(bc_idx).filter(|t| t.count > 1).cloned();
                    self.pop(); // x0 = value
                    mov_reg(&mut self.mem, 1, 0); // x1 = value
                    self.pop(); // x0 = key
                    mov_reg(&mut self.mem, 7, 0); // x7 = key (saved for miss path)
                    self.pop(); // x0 = object
                    mov_reg(&mut self.mem, 2, 0); // x2 = object
                    movz(&mut self.mem, 3, 1);
                    emit(&mut self.mem, 0xEA03005F); // TST x2, x3
                    let patch_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000001); // B.NE +0
                    emit(&mut self.mem, 0xF100185F); // CMP x2, #6
                    let patch_sentinel = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000009); // B.LS +0
                    ldr_off(&mut self.mem, 4, 2, 8); // x4 = [x2 + 8] (shape ptr)
                    ldr_off(&mut self.mem, 3, 4, 0); // x3 = [x4] (shape.id)
                    let mut miss_patches: Vec<(usize, u32)> = Vec::new();
                    let mut done_patches: Vec<usize> = Vec::new();
                    if let Some(table) = ic_table {
                        // === Polymorphic: N-entry scalar scan ===
                        let n = table.count;
                        let adr_off = self.mem.current_offset();
                        emit(&mut self.mem, 0x10000000 | 4); // ADR x4, 0
                        let mut eq_offsets: Vec<usize> = Vec::with_capacity(n);
                        for i in 0..n {
                            let base = (i as u32) * 16;
                            ldr_off(&mut self.mem, 5, 4, base); // x5 = shape_id[i]
                            ldr_off(&mut self.mem, 6, 4, base + 8); // x6 = slot_offset[i]
                            cmp_reg(&mut self.mem, 5, 3);
                            eq_offsets.push(self.mem.current_offset());
                            emit(&mut self.mem, 0x54000000); // B.EQ → found
                        }
                        let b_miss = self.mem.current_offset();
                        emit(&mut self.mem, 0x14000000); // B → miss
                        let found_off = self.mem.current_offset();
                        for &eq in &eq_offsets {
                            let d = ((found_off as i64 - eq as i64) / 4) as u32;
                            self.mem.patch_u32(eq, 0x54000000 | ((d & 0x7FFFF) << 5));
                        }
                        miss_patches.push((b_miss, 0x14000000));
                        str_reg(&mut self.mem, 1, 2, 6); // [x2 + x6] = value
                        mov_reg(&mut self.mem, 0, 1);
                        self.push();
                        done_patches.push(self.mem.current_offset());
                        emit(&mut self.mem, 0x14000000); // B → done
                        self.ic_table_patches.push(IcTablePatch {
                            adr_offset: adr_off,
                            table,
                        });
                    } else {
                        // === Monomorphic: single shape_id guard ===
                        mov_imm64(&mut self.mem, 6, shape_id);
                        cmp_reg(&mut self.mem, 3, 6);
                        miss_patches.push((self.mem.current_offset(), 0x54000001));
                        emit(&mut self.mem, 0x54000001); // B.NE → miss
                        str_off(&mut self.mem, 1, 2, 32 + offset * 8);
                        mov_reg(&mut self.mem, 0, 1);
                        self.push();
                        done_patches.push(self.mem.current_offset());
                        emit(&mut self.mem, 0x14000000); // B → done
                    }
                    // === Miss handler ===
                    let miss_offset = self.mem.current_offset();
                    for &(patch, orig) in &miss_patches {
                        let d = ((miss_offset as i64 - patch as i64) / 4) as u32;
                        if (orig & 0xFF000000) == 0x14000000 {
                            self.mem.patch_u32(patch, 0x14000000 | (d & 0x03FF_FFFF));
                        } else {
                            self.mem
                                .patch_u32(patch, (orig & !0x00FF_FFE0) | ((d & 0x7FFFF) << 5));
                        }
                    }
                    self.record_bailout_point(bc_idx, BailoutReason::ShapeMiss);
                    mov_reg(&mut self.mem, 0, 2);
                    self.push();
                    mov_reg(&mut self.mem, 0, 7);
                    self.push();
                    mov_reg(&mut self.mem, 0, 1);
                    self.push();
                    mov_reg(&mut self.mem, 2, JIT_STACK_REG);
                    mov_imm64(&mut self.mem, 1, bc_idx as u64);
                    mov_reg(&mut self.mem, 0, VM_REG);
                    ldr_off(&mut self.mem, 15, VM_REG, 520);
                    emit(&mut self.mem, 0xD63F01E0);
                    movz(&mut self.mem, 0, 0);
                    self.push();
                    self.emit_epilogue();
                    let done_offset = self.mem.current_offset();
                    for &patch in &done_patches {
                        let d = ((done_offset as i64 - patch as i64) / 4) as u32;
                        self.mem.patch_u32(patch, 0x14000000 | (d & 0x03FF_FFFF));
                    }
                    let d = ((miss_offset as i64 - patch_smi as i64) / 4) as u32;
                    self.mem
                        .patch_u32(patch_smi, 0x54000001 | ((d & 0x7FFFF) << 5));
                    let d = ((miss_offset as i64 - patch_sentinel as i64) / 4) as u32;
                    self.mem
                        .patch_u32(patch_sentinel, 0x54000009 | ((d & 0x7FFFF) << 5));
                }
                Opcode::Return => {
                    self.emit_epilogue();
                }
                Opcode::Neg => {
                    // Smi(-n) = -(2n+1) + 2 = -2n + 1 = Smi(-n)
                    self.pop(); // x0 = value
                    self.emit_smi_check(bc_idx, &[]); // check value is Smi
                    mov_reg(&mut self.mem, 8, 0); // x8 = value (save)
                    sub_reg(&mut self.mem, 0, 31, 0); // SUB x0, XZR, x0 (= NEG)
                    add_imm(&mut self.mem, 0, 0, 2);
                    self.emit_smi_overflow_bailout_or_continue(bc_idx, Some(8), None);
                    self.push();
                }
                Opcode::Not => {
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check value is Smi
                    mov_reg(&mut self.mem, 2, 0); // x2 = original
                    movz(&mut self.mem, 1, 2);
                    cmp_reg(&mut self.mem, 2, 1);
                    // CSET x0, LS = CSINC x0, XZR, XZR, HI (= !LS)
                    emit(&mut self.mem, 0x9A9F87E0);
                    movz(&mut self.mem, 1, 4);
                    cmp_reg(&mut self.mem, 2, 1);
                    // CSET EQ = CSINC NE
                    emit(&mut self.mem, 0x9A9F17E0); // x1 = 1 if original == 4
                    orr_reg(&mut self.mem, 0, 0, 1); // x0 = x0 | x1
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::Void => {
                    self.pop();
                    movz(&mut self.mem, 0, 0); // x0 = 0 (undefined)
                    self.push();
                }
                Opcode::UnaryPlus => {
                    // No-op for Smi: ToNumber(smi) = smi, value stays on JIT stack
                    // But other types need interpreter, so guard the top of stack
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]);
                    // value remains in x0, still on stack top — push it back
                    self.push();
                }
                Opcode::BitNot => {
                    // Smi(~n) = ~Smi(n) + 1
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]);
                    emit(&mut self.mem, 0xAA2003E0); // MVN x0, x0 (ORN x0, xzr, x0)
                    add_imm(&mut self.mem, 0, 0, 1);
                    self.push();
                }
                Opcode::StrictNe => {
                    self.pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[x1(b)]
                    cmp_reg(&mut self.mem, 0, 1);
                    // CSET x0, NE = CSINC x0, XZR, XZR, EQ (= !NE)
                    emit(&mut self.mem, 0x9A9F07E0); // CSET NE = CSINC EQ
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::LoadThis => {
                    // Call lexical helper with LEX_LOAD_THIS
                    movz(&mut self.mem, 2, 0); // x2 = 0 (unused arg1)
                    movz(&mut self.mem, 1, LEX_LOAD_THIS as u16); // x1 = op
                    ldr_off(&mut self.mem, 15, VM_REG, 512);
                    mov_reg(&mut self.mem, 0, VM_REG);
                    movz(&mut self.mem, 3, 0);
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    self.push();
                }
                // Lexical-scope operations — call into VM via helper function
                Opcode::BlockEnter => {
                    let count = *instr.operands.first().unwrap_or(&0) as u64;
                    self.emit_lexical_call(LEX_BLOCK_ENTER, count, 0);
                }
                Opcode::BlockLeave => {
                    self.emit_lexical_call(LEX_BLOCK_LEAVE, 0, 0);
                }
                Opcode::DeclareLet => {
                    let slot = *instr.operands.first().unwrap_or(&0) as u64;
                    self.emit_lexical_call(LEX_DECLARE_LET, slot, 0);
                }
                Opcode::DeclareConst => {
                    let slot = *instr.operands.first().unwrap_or(&0) as u64;
                    self.emit_lexical_call(LEX_DECLARE_CONST, slot, 0);
                }
                Opcode::LoadLexical => {
                    let slot = *instr.operands.first().unwrap_or(&0) as u64;
                    // Set up args: x0=vm_ptr, x1=op(LEX_LOAD), x2=slot, x3=0
                    mov_imm64(&mut self.mem, 2, slot);
                    mov_imm64(&mut self.mem, 1, LEX_LOAD);
                    ldr_off(&mut self.mem, 15, VM_REG, 512);
                    mov_reg(&mut self.mem, 0, VM_REG);
                    movz(&mut self.mem, 3, 0); // x3 = 0
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    self.push(); // push the loaded value (in x0)
                }
                Opcode::StoreLexical => {
                    let slot = *instr.operands.first().unwrap_or(&0) as u64;
                    self.pop(); // x0 = value to store
                    // Pass value as arg2 (already in x0), vm_ptr in x0 is clobbered
                    // We need to save and set up args carefully
                    mov_reg(&mut self.mem, 3, 0); // x3 = value (arg2)
                    mov_imm64(&mut self.mem, 2, slot); // x2 = slot (arg1)
                    mov_imm64(&mut self.mem, 1, LEX_STORE); // x1 = op
                    ldr_off(&mut self.mem, 15, VM_REG, 512); // x15 = helper addr
                    mov_reg(&mut self.mem, 0, VM_REG); // x0 = vm_ptr
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    // helper returns val back in x0
                    self.push();
                }
                Opcode::TypeOf => {
                    // Pop value from JIT stack
                    self.pop(); // x0 = value
                    mov_reg(&mut self.mem, 1, 0); // x1 = value_raw (second arg)
                    mov_reg(&mut self.mem, 0, VM_REG); // x0 = vm_ptr (first arg)
                    ldr_off(&mut self.mem, 15, VM_REG, 528); // typeof_helper at offset 528
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    self.push(); // push result (x0)
                }
                Opcode::MakeArgumentsArray => {
                    // Phase B: bail on entry — always deopt to interpreter.
                    self.record_bailout_point(bc_idx, BailoutReason::BailOnEntry);
                    mov_reg(&mut self.mem, 2, JIT_STACK_REG);
                    mov_imm64(&mut self.mem, 1, bc_idx as u64);
                    mov_reg(&mut self.mem, 0, VM_REG);
                    ldr_off(&mut self.mem, 15, VM_REG, 520);
                    emit(&mut self.mem, 0xD63F01E0);
                    movz(&mut self.mem, 0, 0);
                    self.push();
                    self.emit_epilogue();
                }
                Opcode::Call => {
                    let argc = instr.operands[0] as u32;
                    // F-2 Layer 2b: inline eligible call sites
                    let inline_entry = self
                        .inline_plan
                        .entries
                        .iter()
                        .find(|e| e.call_instr_idx == bc_idx)
                        .cloned();
                    if let Some(entry) = inline_entry {
                        self.emit_inline_call(bc_idx, &entry);
                        continue;
                    }
                    // Clear bailout flag at jit_stack[63] (offset 63*8=504)
                    movz(&mut self.mem, 0, 0);
                    str_off(&mut self.mem, 0, VM_REG, 504);
                    // Call helper: x0=vm_ptr, x1=gc_ptr, x2=argc, x3=bc_idx, x4=jit_sp
                    mov_reg(&mut self.mem, 0, VM_REG);
                    mov_reg(&mut self.mem, 1, GC_REG);
                    mov_imm64(&mut self.mem, 2, argc as u64);
                    mov_imm64(&mut self.mem, 3, bc_idx as u64);
                    mov_reg(&mut self.mem, 4, JIT_STACK_REG);
                    ldr_off(&mut self.mem, 15, VM_REG, 560); // call_helper
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    // Check bailout flag at [VM_REG + 504]
                    ldr_off(&mut self.mem, 1, VM_REG, 504); // x1 = flag
                    movz(&mut self.mem, 2, 1); // x2 = 1
                    cmp_reg(&mut self.mem, 1, 2); // flag == 1?
                    let bail_path = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000001); // B.NE +0 (skip bailout if flag != 1)
                    // Bailout path: call bailout_helper and return from JIT
                    self.record_bailout_point(bc_idx, BailoutReason::BailOnEntry);
                    mov_reg(&mut self.mem, 2, JIT_STACK_REG);
                    mov_imm64(&mut self.mem, 1, bc_idx as u64);
                    mov_reg(&mut self.mem, 0, VM_REG);
                    ldr_off(&mut self.mem, 15, VM_REG, 520); // bailout_helper
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    movz(&mut self.mem, 0, 0);
                    self.push();
                    self.emit_epilogue();
                    // Normal path: pop argc+2 (args+callee+this) and push result
                    let done_path = self.mem.current_offset();
                    let pop_bytes = (argc + 2) * 8;
                    sub_imm(&mut self.mem, JIT_STACK_REG, JIT_STACK_REG, pop_bytes);
                    self.push();
                    // Patch B.NE to skip bailout
                    let d = ((done_path as i64 - bail_path as i64) / 4) as u32;
                    self.mem
                        .patch_u32(bail_path, 0x54000001 | ((d & 0x7FFFF) << 5));
                }
                Opcode::LoadGlobal => {
                    let name_idx = instr.operands[0] as u64;
                    let prog_ptr = program as *const rune_bytecode::opcode::BytecodeProgram
                        as *const u8 as u64;
                    // x0 = x19 (vm_ptr), x1 = x20 (gc_ptr)
                    mov_reg(&mut self.mem, 0, VM_REG);
                    mov_reg(&mut self.mem, 1, GC_REG);
                    // x2 = prog_ptr (immediate), x3 = 0 (op: LoadGlobal)
                    mov_imm64(&mut self.mem, 2, prog_ptr);
                    mov_imm64(&mut self.mem, 3, 0);
                    // x4 = name_idx, x5 = 0 (unused for load)
                    mov_imm64(&mut self.mem, 4, name_idx);
                    mov_imm64(&mut self.mem, 5, 0);
                    // Load global_helper from [x19 + 544] into x15
                    ldr_off(&mut self.mem, 15, VM_REG, 544);
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    self.push(); // push result (x0)
                }
                Opcode::StoreGlobal => {
                    let name_idx = instr.operands[0] as u64;
                    let prog_ptr = program as *const rune_bytecode::opcode::BytecodeProgram
                        as *const u8 as u64;
                    // Pop value to store from JIT stack into x5
                    self.pop();
                    mov_reg(&mut self.mem, 5, 0); // x5 = value_raw
                    // x0 = x19 (vm_ptr), x1 = x20 (gc_ptr)
                    mov_reg(&mut self.mem, 0, VM_REG);
                    mov_reg(&mut self.mem, 1, GC_REG);
                    // x2 = prog_ptr, x3 = 1 (op: StoreGlobal)
                    mov_imm64(&mut self.mem, 2, prog_ptr);
                    mov_imm64(&mut self.mem, 3, 1);
                    // x4 = name_idx, x5 = value_raw (already set)
                    mov_imm64(&mut self.mem, 4, name_idx);
                    // Load global_helper from [x19 + 544] into x15
                    ldr_off(&mut self.mem, 15, VM_REG, 544);
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    self.push(); // push result (stored value)
                }
                Opcode::IncGlobal | Opcode::DecGlobal => {
                    let name_idx = instr.operands[0] as u64;
                    let is_prefix = instr.operands[1];
                    let op = if matches!(instr.opcode, Opcode::IncGlobal) {
                        2u64
                    } else {
                        3u64
                    };
                    let prog_ptr = program as *const rune_bytecode::opcode::BytecodeProgram
                        as *const u8 as u64;
                    // x0 = x19 (vm_ptr), x1 = x20 (gc_ptr)
                    mov_reg(&mut self.mem, 0, VM_REG);
                    mov_reg(&mut self.mem, 1, GC_REG);
                    // x2 = prog_ptr, x3 = op
                    mov_imm64(&mut self.mem, 2, prog_ptr);
                    mov_imm64(&mut self.mem, 3, op);
                    // x4 = name_idx, x5 = is_prefix
                    mov_imm64(&mut self.mem, 4, name_idx);
                    mov_imm64(&mut self.mem, 5, is_prefix as u64);
                    // Load global_helper from [x19 + 544] into x15
                    ldr_off(&mut self.mem, 15, VM_REG, 544);
                    emit(&mut self.mem, 0xD63F01E0); // BLR x15
                    self.push(); // push result (new or old value)
                }
                _ => {
                    // Unknown opcode: emit a trap so we notice quickly.
                    emit(&mut self.mem, 0xD4200000); // BRK #0
                }
            }
        }

        self.resolve_patches();

        // Post-process IC table patches: write table data and fix up ADRs.
        let patches = std::mem::take(&mut self.ic_table_patches);
        for p in &patches {
            let table_offset = self.mem.current_offset();
            for i in 0..16 {
                if i < p.table.count {
                    self.mem.emit_u64(p.table.entries[i].shape_id);
                    self.mem.emit_u64(p.table.entries[i].slot_offset);
                } else {
                    self.mem.emit_u64(0); // shape_id = 0 (never matches)
                    self.mem.emit_u64(0); // slot_offset = 0 (unused)
                }
            }
            // Patch the ADR instruction at p.adr_offset
            let byte_offset = (table_offset as i64) - (p.adr_offset as i64);
            let immlo = (byte_offset & 0x3) as u32;
            let immhi = ((byte_offset >> 2) & 0x7FFFF) as u32;
            let instr = 0x10000000 | (immlo << 29) | (immhi << 5) | 4; // xd = x4
            self.mem.patch_u32(p.adr_offset, instr);
        }

        let bailout_table = BailoutTable {
            points: std::mem::take(&mut self.bailout_table),
        };

        CompiledFunction {
            mem: self.mem,
            bailout_table,
        }
    }

    /// Call the lexical helper function (loaded from JitVmState::jit_helpers).
    fn emit_lexical_call(&mut self, op: u64, arg1: u64, arg2: u64) {
        // Load helper address from [x19 + 512] (offset of JitHelpers in Vm)
        ldr_off(&mut self.mem, 15, VM_REG, 512);
        // Set up arguments: x0=vm_ptr, x1=op, x2=arg1, x3=arg2
        mov_reg(&mut self.mem, 0, VM_REG);
        mov_imm64(&mut self.mem, 1, op);
        mov_imm64(&mut self.mem, 2, arg1);
        mov_imm64(&mut self.mem, 3, arg2);
        // BLR x15
        emit(&mut self.mem, 0xD63F01E0);
    }
}

#[cfg(all(test, target_arch = "aarch64"))]
mod tests {
    use super::*;

    /// Stub bailout helper for tests that don't need real bailout processing.
    /// Prevents SIGSEGV when the overflow guard fires.
    extern "C" fn test_bailout_stub(_vm: *mut u8, _bc_pc: usize, _jit_sp: *mut u64) -> u64 {
        0
    }

    /// Stub float64 add helper for tests — prevents SIGSEGV when the float64 Add
    /// path is taken (non-Smi input or Smi overflow).
    extern "C" fn test_float64_add_stub(_vm: *mut u8, _gc: *mut u8, _a: u64, _b: u64) -> u64 {
        0
    }

    /// Allocate a `JitVmState` on the heap and return a raw VM pointer.
    /// The trace compiler expects `jit_stack` to live at offset 0 from this
    /// pointer. Tests intentionally leak this small allocation.
    fn jit_vm_ptr() -> *mut u8 {
        let state = Box::new(JitVmState {
            jit_stack: [0; JIT_STACK_SIZE],
            jit_helpers: JitHelpers {
                lexical_helper: 0,
                bailout_helper: test_bailout_stub as usize,
                typeof_helper: 0,
                string_helper: 0,
                global_helper: 0,
                float64_add_helper: test_float64_add_stub as usize,
                call_helper: 0,
                _reserved: [0; 1],
            },
            jit_stack_base: 0,
        });
        Box::into_raw(state) as *mut u8
    }

    #[test]
    fn test_aarch64_mov_ret() {
        // Emit: mov x0, #85 ; ret (85 = Smi(42) = 42*2+1)
        let mut mem = ExecutableMemory::allocate(256);
        mov_imm64(&mut mem, 0, 85);
        ret(&mut mem);
        mem.make_executable();

        let func: unsafe fn() -> u64 = unsafe { std::mem::transmute(mem.code_ptr()) };
        assert_eq!(unsafe { func() }, 85);
    }

    #[test]
    fn test_aarch64_add() {
        // Emit: add x0, x0, x1 ; ret (AAPCS64: x0 = arg1, x1 = arg2)
        let mut mem = ExecutableMemory::allocate(256);
        add_reg(&mut mem, 0, 0, 1);
        ret(&mut mem);
        mem.make_executable();

        let func: unsafe fn(u64, u64) -> u64 = unsafe { std::mem::transmute(mem.code_ptr()) };
        assert_eq!(unsafe { func(10, 32) }, 42);
    }

    #[test]
    fn test_aarch64_cset_lt_encoding() {
        let mut mem = ExecutableMemory::allocate(256);
        // CMP x0, x1 (x0=a=1, x1=b=21); CSET x0, LT; RET
        mov_imm64(&mut mem, 0, 1); // x0 = 1 = Smi(0)
        mov_imm64(&mut mem, 1, 21); // x1 = 21 = Smi(10)
        cmp_reg(&mut mem, 0, 1);
        // CSET x0, LT = CSINC x0, XZR, XZR, GE
        emit(&mut mem, 0x9A9FA7E0);
        ret(&mut mem);
        mem.make_executable();
        let func: unsafe fn() -> u64 = unsafe { std::mem::transmute(mem.code_ptr()) };
        let r = unsafe { func() };
        assert_eq!(r, 1u64, "CSET LT (1 < 21) should return 1");
    }

    #[test]
    fn test_aarch64_cset_lt_false_encoding() {
        let mut mem = ExecutableMemory::allocate(256);
        mov_imm64(&mut mem, 0, 21); // x0 = 21 = Smi(10)
        mov_imm64(&mut mem, 1, 1); // x1 = 1 = Smi(0)
        cmp_reg(&mut mem, 0, 1);
        emit(&mut mem, 0x9A9FA7E0); // CSET x0, LT
        ret(&mut mem);
        mem.make_executable();
        let func: unsafe fn() -> u64 = unsafe { std::mem::transmute(mem.code_ptr()) };
        let r = unsafe { func() };
        assert_eq!(r, 0u64, "CSET LT (21 < 1) should return 0");
    }

    #[test]
    fn test_aarch64_codegen_load_smi_return() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(r, ((42u64 << 1) | 1));
    }

    #[test]
    fn test_aarch64_codegen_add_locals() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadLocal, vec![0]),
                Instruction::new(Opcode::LoadLocal, vec![1]),
                Instruction::new(Opcode::Add, vec![]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let mut locals: Vec<u64> = vec![((10u64 << 1) | 1), ((32u64 << 1) | 1)];
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), locals.as_mut_ptr()) };
        assert_eq!(r, ((42u64 << 1) | 1));
    }

    #[test]
    fn test_aarch64_codegen_lt_smi() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::LoadSmi, vec![10]),
                Instruction::new(Opcode::Lt, vec![]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        // Lt should return true, encoded as Smi(1)=3 in the JIT (matching x86_64).
        assert_eq!(r, 3u64);
    }

    #[test]
    fn test_aarch64_codegen_jump_if_false() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // if (Smi(1)=truthy) 42 else 7
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::JumpIfFalse, vec![4]),
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::Jump, vec![5]),
                Instruction::new(Opcode::LoadSmi, vec![7]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(r, ((42u64 << 1) | 1));
    }

    #[test]
    fn test_aarch64_codegen_jump_if_false_falsy() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // if (Smi(0)=falsy) 42 else 7
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::JumpIfFalse, vec![4]),
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::Jump, vec![5]),
                Instruction::new(Opcode::LoadSmi, vec![7]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(r, ((7u64 << 1) | 1));
    }

    #[test]
    fn test_aarch64_codegen_load_large_smi() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // LoadSmi 100000 → should produce Smi(100000) = 200001
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![100000]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(r, ((100000u64 << 1) | 1));
    }

    #[test]
    fn test_aarch64_codegen_load_very_large_smi() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // LoadSmi 70000 → Smi(70000) = 140001 (needs 18 bits)
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![70000]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(r, ((70000u64 << 1) | 1));
    }

    #[test]
    fn test_aarch64_codegen_large_sum_1000_iterations() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // Sum 0..1000 = 499,500 — well within i31 range (max = 2^30 − 1 = 1,073,741,823).
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![2]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![3]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadLocal, vec![3]),
                Instruction::new(Opcode::LoadSmi, vec![1000]),
                Instruction::new(Opcode::Lt, vec![]),
                Instruction::new(Opcode::JumpIfFalse, vec![18]),
                Instruction::new(Opcode::LoadLocal, vec![2]),
                Instruction::new(Opcode::LoadLocal, vec![3]),
                Instruction::new(Opcode::Add, vec![]),
                Instruction::new(Opcode::StoreLocal, vec![2]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::IncLocal, vec![3, 1]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::Jump, vec![6]),
                Instruction::new(Opcode::LoadLocal, vec![2]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let mut locals: Vec<u64> = vec![0, 0, 0, 0];
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), locals.as_mut_ptr()) };
        let expected = (499500u64 << 1) | 1;
        assert_eq!(r, expected, "1000 iters: got {}, expected {}", r, expected);
    }

    #[test]
    fn test_aarch64_codegen_bitwise_ops() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // Test Shl, Shr, BitAnd, BitOr, BitXor with Smi operands
        // 10 << 1 = 20 → Smi(20) = 41
        // 20 >> 1 = 10 → Smi(10) = 21
        // 6 & 3 = 2 → Smi(2) = 5
        // 6 | 3 = 7 → Smi(7) = 15
        // 6 ^ 3 = 5 → Smi(5) = 11
        let tests: Vec<(Opcode, i64, i64, u64)> = vec![
            (Opcode::Shl, 10, 1, 41),
            (Opcode::Shr, 20, 1, 21),
            (Opcode::BitAnd, 6, 3, 5),
            (Opcode::BitOr, 6, 3, 15),
            (Opcode::BitXor, 6, 3, 11),
        ];
        type JF = unsafe fn(*mut u8, *mut u8, *mut u64) -> u64;
        for (op, a, b, expected) in tests {
            let a_smi = ((a as u64) << 1) | 1;
            let b_smi = ((b as u64) << 1) | 1;
            let mut locals = vec![a_smi, b_smi];
            let prog = BytecodeProgram::new(
                vec![
                    Instruction::new(Opcode::LoadLocal, vec![0]),
                    Instruction::new(Opcode::LoadLocal, vec![1]),
                    Instruction::new(op, vec![]),
                    Instruction::new(Opcode::Return, vec![]),
                ],
                vec![],
                vec![],
            );
            let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
            compiled.mem.make_executable();
            let vm = jit_vm_ptr();
            let func: JF = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
            let r = unsafe { func(vm, std::ptr::null_mut(), locals.as_mut_ptr()) };
            assert_eq!(
                r, expected,
                "{:?} {} {}: expected {}, got {}",
                op, a, b, expected, r
            );
        }
    }

    #[test]
    fn test_aarch64_cset_all_comparisons() {
        // Test each comparison opcode: Lt, Gt, Le, Ge, StrictEq
        type JF = unsafe fn(*mut u8, *mut u8, *mut u64) -> u64;
        let test = |op: Opcode, a: i64, b: i64, expected: bool| {
            let a_smi = ((a as u64) << 1) | 1;
            let b_smi = ((b as u64) << 1) | 1;
            let mut locals = vec![a_smi, b_smi];
            let prog = BytecodeProgram::new(
                vec![
                    Instruction::new(Opcode::LoadLocal, vec![0]),
                    Instruction::new(Opcode::LoadLocal, vec![1]),
                    Instruction::new(op, vec![]),
                    Instruction::new(Opcode::Return, vec![]),
                ],
                vec![],
                vec![],
            );
            let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
            compiled.mem.make_executable();
            let vm = jit_vm_ptr();
            let func: JF = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
            let r = unsafe { func(vm, std::ptr::null_mut(), locals.as_mut_ptr()) };
            // true=3 (Smi(1)), false=1 (Smi(0))
            assert_eq!(r == 3, expected, "{:?} {} {}: got {}", op, a, b, r);
        };
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        test(Opcode::Lt, 0, 10, true);
        test(Opcode::Lt, 10, 0, false);
        test(Opcode::Gt, 10, 0, true);
        test(Opcode::Gt, 0, 10, false);
        test(Opcode::Le, 0, 10, true);
        test(Opcode::Le, 10, 10, true);
        test(Opcode::Le, 10, 0, false);
        test(Opcode::Ge, 10, 0, true);
        test(Opcode::Ge, 10, 10, true);
        test(Opcode::Ge, 0, 10, false);
        test(Opcode::StrictEq, 42, 42, true);
        test(Opcode::StrictEq, 42, 99, false);
    }

    #[test]
    fn test_aarch64_codegen_large_sum_loop_trace_indices() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // Sum 0..999 = 499,500 — well within i31 range.
        // Uses 4-element locals vec.
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![2]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![3]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadLocal, vec![3]),
                Instruction::new(Opcode::LoadSmi, vec![1000]),
                Instruction::new(Opcode::Lt, vec![]),
                Instruction::new(Opcode::JumpIfFalse, vec![18]),
                Instruction::new(Opcode::LoadLocal, vec![2]),
                Instruction::new(Opcode::LoadLocal, vec![3]),
                Instruction::new(Opcode::Add, vec![]),
                Instruction::new(Opcode::StoreLocal, vec![2]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::IncLocal, vec![3, 1]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::Jump, vec![6]),
                Instruction::new(Opcode::LoadLocal, vec![2]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let mut locals: Vec<u64> = vec![0, 0, 0, 0];
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), locals.as_mut_ptr()) };
        let expected = (499500u64 << 1) | 1;
        assert_eq!(r, expected, "got {}, expected {}", r, expected);
    }

    #[test]
    fn test_aarch64_codegen_large_sum_loop() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // Sum 0..999 = 499,500 — well within i31 range.
        // Uses 2-element locals vec.
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![0]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![1]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadLocal, vec![1]),
                Instruction::new(Opcode::LoadSmi, vec![1000]),
                Instruction::new(Opcode::Lt, vec![]),
                Instruction::new(Opcode::JumpIfFalse, vec![18]),
                Instruction::new(Opcode::LoadLocal, vec![0]),
                Instruction::new(Opcode::LoadLocal, vec![1]),
                Instruction::new(Opcode::Add, vec![]),
                Instruction::new(Opcode::StoreLocal, vec![0]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::IncLocal, vec![1, 1]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::Jump, vec![6]),
                Instruction::new(Opcode::LoadLocal, vec![0]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let mut locals: Vec<u64> = vec![0, 0];
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), locals.as_mut_ptr()) };
        let expected = (499500u64 << 1) | 1;
        assert_eq!(r, expected, "got {}, expected {}", r, expected);
    }

    #[test]
    fn test_aarch64_codegen_loop() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // i = 0; s = 0; while (i < 10) { s += i; i++; } return s;
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![1]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![0]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadLocal, vec![0]), // loop head
                Instruction::new(Opcode::LoadSmi, vec![10]),
                Instruction::new(Opcode::Lt, vec![]),
                Instruction::new(Opcode::JumpIfFalse, vec![18]),
                Instruction::new(Opcode::LoadLocal, vec![1]),
                Instruction::new(Opcode::LoadLocal, vec![0]),
                Instruction::new(Opcode::Add, vec![]),
                Instruction::new(Opcode::StoreLocal, vec![1]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::IncLocal, vec![0, 1]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::Jump, vec![6]),
                Instruction::new(Opcode::LoadLocal, vec![1]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let mut locals: Vec<u64> = vec![0, 0];
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), locals.as_mut_ptr()) };
        // sum 0..9 = 45
        assert_eq!(r, ((45u64 << 1) | 1));
    }

    #[test]
    fn test_aarch64_codegen_load_property_ic() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        let shape_id: u64 = 0xDEAD_BEEF_CAFE_1234;
        let slot_value: u64 = (42u64 << 1) | 1; // Smi(42)
        // Mock shape: [id: u64]
        let mock_shape = Box::new(shape_id);
        let shape_ptr = Box::into_raw(mock_shape) as *mut u8;
        // Mock object: [GcHeader(8) | shape*(8) | proto*(8) | unused(8) | slots(8)...]
        let mut obj = [0u8; 80];
        obj[8..16].copy_from_slice(&(shape_ptr as u64).to_le_bytes());
        obj[32..40].copy_from_slice(&slot_value.to_le_bytes());
        let obj_addr = obj.as_ptr() as u64;
        // Create JIT VM state with obj_addr pre-pushed
        let mut vm = JitVmState {
            jit_stack: [0; JIT_STACK_SIZE],
            jit_helpers: JitHelpers {
                lexical_helper: 0,
                bailout_helper: 0,
                typeof_helper: 0,
                string_helper: 0,
                global_helper: 0,
                float64_add_helper: 0,
                call_helper: 0,
                _reserved: [0; 1],
            },
            jit_stack_base: 0,
        };
        // Stack: [top=key, object] — LoadPropertyIC pops key then object
        vm.jit_stack[0] = obj_addr; // second pop: object
        vm.jit_stack[1] = 0x42; // first pop: key (discarded in fast path)
        let vm_ptr = &mut vm as *mut _ as *mut u8;
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadPropertyIC, vec![shape_id as i64, 0]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len())
            .with_jit_stack_offset(16)
            .compile(&prog);
        compiled.mem.make_executable();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm_ptr, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, slot_value);
        unsafe {
            drop(Box::from_raw(shape_ptr));
        }
    }

    // -----------------------------------------------------------------------
    // Overflow guard tests (aarch64)
    // -----------------------------------------------------------------------

    #[test]
    fn test_aarch64_add_overflow() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // (2^30 − 1) + 1 = 2^30 → exceeds i31 → bailout
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1073741823]),
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::Add, vec![]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        // Overflow → bailout → returns undefined = 0
        assert_eq!(
            result, 0,
            "Add overflow: expected 0 (bailout), got {}",
            result
        );
    }

    #[test]
    fn test_aarch64_sub_overflow() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // −2^30 − 1 = −(2^30+1) < −2^30 → underflow → bailout
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![-1073741824]),
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::Sub, vec![]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(
            result, 0,
            "Sub underflow: expected 0 (bailout), got {}",
            result
        );
    }

    #[test]
    fn test_aarch64_mul_overflow() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // 2^16 × 2^16 = 2^32 > 2^30−1 → overflow → bailout
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![65536]),
                Instruction::new(Opcode::LoadSmi, vec![65536]),
                Instruction::new(Opcode::Mul, vec![]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(
            result, 0,
            "Mul overflow: expected 0 (bailout), got {}",
            result
        );
    }

    #[test]
    fn test_aarch64_neg_overflow() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // −(−2^30) = 2^30 > 2^30−1 → overflow → bailout
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![-1073741824]),
                Instruction::new(Opcode::Neg, vec![]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(
            result, 0,
            "Neg overflow: expected 0 (bailout), got {}",
            result
        );
    }

    #[test]
    fn test_aarch64_shl_overflow() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // 1 << 31 = 2^31 > 2^30−1 → overflow → bailout
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::LoadSmi, vec![31]),
                Instruction::new(Opcode::Shl, vec![]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(
            result, 0,
            "Shl overflow: expected 0 (bailout), got {}",
            result
        );
    }

    #[test]
    fn test_aarch64_non_smi_input_bailouts() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction, Opcode};
        let run = |instrs: Vec<Instruction>| -> u64 {
            let prog = BytecodeProgram::new(instrs, vec![], vec![]);
            let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
            compiled.mem.make_executable();
            let vm = jit_vm_ptr();
            let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
                unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
            unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) }
        };

        // Binary ops — non-Smi b (first operand), Smi a
        for &(op, name) in &[
            (Opcode::Add, "Add"),
            (Opcode::Sub, "Sub"),
            (Opcode::Mul, "Mul"),
            (Opcode::Shl, "Shl"),
            (Opcode::Shr, "Shr"),
            (Opcode::BitAnd, "BitAnd"),
            (Opcode::BitOr, "BitOr"),
            (Opcode::BitXor, "BitXor"),
            (Opcode::ShrU, "ShrU"),
            (Opcode::Lt, "Lt"),
            (Opcode::Gt, "Gt"),
            (Opcode::Le, "Le"),
            (Opcode::Ge, "Ge"),
            (Opcode::StrictEq, "StrictEq"),
            (Opcode::StrictNe, "StrictNe"),
            (Opcode::Eq, "Eq"),
            (Opcode::Ne, "Ne"),
        ] {
            let r = run(vec![
                Instruction::new(Opcode::LoadUndefined, vec![]),
                Instruction::new(Opcode::LoadSmi, vec![10]),
                Instruction::new(op, vec![]),
                Instruction::new(Opcode::Return, vec![]),
            ]);
            assert_eq!(r, 0, "{}: non-Smi b should bail, got {}", name, r);
        }

        // Binary ops — non-Smi a (second operand), Smi b
        for &(op, name) in &[
            (Opcode::Add, "Add"),
            (Opcode::Sub, "Sub"),
            (Opcode::Mul, "Mul"),
            (Opcode::Shl, "Shl"),
            (Opcode::Shr, "Shr"),
            (Opcode::BitAnd, "BitAnd"),
            (Opcode::BitOr, "BitOr"),
            (Opcode::BitXor, "BitXor"),
            (Opcode::ShrU, "ShrU"),
            (Opcode::Lt, "Lt"),
            (Opcode::Gt, "Gt"),
            (Opcode::Le, "Le"),
            (Opcode::Ge, "Ge"),
            (Opcode::StrictEq, "StrictEq"),
            (Opcode::StrictNe, "StrictNe"),
            (Opcode::Eq, "Eq"),
            (Opcode::Ne, "Ne"),
        ] {
            let r = run(vec![
                Instruction::new(Opcode::LoadSmi, vec![10]),
                Instruction::new(Opcode::LoadUndefined, vec![]),
                Instruction::new(op, vec![]),
                Instruction::new(Opcode::Return, vec![]),
            ]);
            assert_eq!(r, 0, "{}: non-Smi a should bail, got {}", name, r);
        }

        // Unary ops — non-Smi operand
        for &(op, name) in &[
            (Opcode::Neg, "Neg"),
            (Opcode::Not, "Not"),
            (Opcode::BitNot, "BitNot"),
            (Opcode::UnaryPlus, "UnaryPlus"),
        ] {
            let r = run(vec![
                Instruction::new(Opcode::LoadUndefined, vec![]),
                Instruction::new(op, vec![]),
                Instruction::new(Opcode::Return, vec![]),
            ]);
            assert_eq!(r, 0, "{}: non-Smi should bail, got {}", name, r);
        }

        // Jumps — non-Smi condition; target must be ≤ instruction count
        for &(op, name) in &[
            (Opcode::JumpIfFalse, "JumpIfFalse"),
            (Opcode::JumpIfTrue, "JumpIfTrue"),
        ] {
            let r = run(vec![
                Instruction::new(Opcode::LoadUndefined, vec![]),
                Instruction::new(op, vec![3]),
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::Return, vec![]),
            ]);
            assert_eq!(r, 0, "{}: non-Smi should bail, got {}", name, r);
        }
    }

    #[test]
    fn test_jit_vm_state_layout() {
        // JitVmState::jit_stack is [u64; 64] → 512 bytes.
        // JitHelpers starts immediately after jit_stack.
        use core::mem::offset_of;
        assert_eq!(
            offset_of!(JitVmState, jit_helpers),
            512,
            "jit_helpers must be at offset 512 (after 64-slot jit_stack)"
        );
        assert_eq!(
            offset_of!(JitVmState, jit_stack_base),
            512 + 64,
            "jit_stack_base must be at offset 576 (after jit_stack + JitHelpers)"
        );
    }

    #[test]
    fn test_jit_helpers_offsets() {
        use core::mem::offset_of;
        // Every hardcoded offset in codegen_aarch64.rs must match JitHelpers layout.
        // Fields are [usize; 8] → 8 bytes each, #[repr(C)].
        assert_eq!(
            offset_of!(JitHelpers, lexical_helper),
            0,
            "lexical_helper at offset 512 from VM base"
        );
        assert_eq!(
            offset_of!(JitHelpers, bailout_helper),
            8,
            "bailout_helper at offset 520 from VM base"
        );
        assert_eq!(
            offset_of!(JitHelpers, typeof_helper),
            16,
            "typeof_helper at offset 528 from VM base"
        );
        assert_eq!(
            offset_of!(JitHelpers, string_helper),
            24,
            "string_helper at offset 536 from VM base"
        );
        assert_eq!(
            offset_of!(JitHelpers, global_helper),
            32,
            "global_helper at offset 544 from VM base"
        );
        assert_eq!(
            offset_of!(JitHelpers, float64_add_helper),
            40,
            "float64_add_helper at offset 552 from VM base"
        );
        assert_eq!(
            offset_of!(JitHelpers, call_helper),
            48,
            "call_helper at offset 560 from VM base"
        );
    }
}
