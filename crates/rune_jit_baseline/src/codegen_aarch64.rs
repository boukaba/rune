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
use crate::{BailoutPoint, BailoutReason, BailoutTable, CompiledFunction};
use rune_bytecode::opcode::Opcode;

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

/// JIT helper function pointer table. Must match `Vm::jit_helpers` layout.
#[repr(C)]
pub struct JitHelpers {
    pub lexical_helper: usize,
    pub bailout_helper: usize,
    _reserved: [usize; 6],
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
    if xd == 31 { emit(mem, 0x91000000 | (xm << 5) | 31); }
    else if xm == 31 { emit(mem, 0x91000000 | (31 << 5) | xd); }
    else { emit(mem, 0xAA0003E0 | (xm << 16) | xd); }
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

/// NOP
fn nop(mem: &mut ExecutableMemory) {
    emit(mem, 0xD503201F);
}

fn push_callee_saved(mem: &mut ExecutableMemory) {
    let mut stp = |rt: u32, rt2: u32| emit(mem, 0xA9BF0000 | (rt2 << 10) | (31 << 5) | rt);
    stp(29, 30); stp(19, 20); stp(21, 22); stp(23, 24); stp(25, 26);
}

fn pop_callee_saved(mem: &mut ExecutableMemory) {
    let mut ldp = |rt: u32, rt2: u32| emit(mem, 0xA8C10000 | (rt2 << 10) | (31 << 5) | rt);
    ldp(25, 26); ldp(23, 24); ldp(21, 22); ldp(19, 20); ldp(29, 30);
}

/// Compile a trace into the given ExecutableMemory buffer.
/// The caller is responsible for calling make_executable() and managing lifetime.
pub fn emit_trace_into(mem: &mut ExecutableMemory, ops: &[(Opcode, Vec<i64>, u64)]) {
    push_callee_saved(mem);
    mov_reg(mem, VM_REG, 0);
    mov_reg(mem, GC_REG, 1);
    mov_reg(mem, LOC_REG, 2);
    // JIT value-stack pointer = VM pointer + jit_stack offset (offset 0).
    add_imm(mem, JIT_STACK_REG, VM_REG, 0);
    for &(ref opcode, ref operands, _shape_id) in ops { compile_op(mem, *opcode, operands); }
    // Pop the top value into x0 and return.
    sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
    ldr_off(mem, 0, JIT_STACK_REG, 0);
    pop_callee_saved(mem);
    ret(mem);
}
/// Compile a recorded trace into native aarch64 code.
pub fn compile_trace(ops: &[(Opcode, Vec<i64>, u64)]) -> ExecutableMemory {
    let mut mem = ExecutableMemory::allocate(4096);
    emit_trace_into(&mut mem, ops);
    mem.make_executable();
    mem
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
pub struct Aarch64CodeGen {
    mem: ExecutableMemory,
    bc_to_native: Vec<usize>,
    pending_patches: Vec<(usize, usize, u32)>, // (patch_offset_in_bytes, target_bc_index, original_instr)
    bailout_table: Vec<BailoutPoint>,
    stack_depth: u32,
    /// Initial offset added to x22 (JIT stack pointer) during prologue.
    /// Used by tests that pre-populate the JIT stack.
    jit_stack_offset: u32,
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
        }
    }

    /// Set an initial offset for the JIT stack pointer.
    /// Allows pre-populated values on the JIT stack to be read correctly.
    pub fn with_jit_stack_offset(mut self, offset: u32) -> Self {
        self.jit_stack_offset = offset;
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

    pub fn compile(
        mut self,
        program: &rune_bytecode::opcode::BytecodeProgram,
    ) -> CompiledFunction {
        self.emit_prologue();

        for (bc_idx, instr) in program.instructions.iter().enumerate() {
            self.bc_to_native[bc_idx] = self.mem.current_offset();
            match instr.opcode {
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
                Opcode::LoadFloat64 => {
                    let idx = instr.operands[0] as usize;
                    let val = program.float_pool.get(idx).copied().unwrap_or(0.0);
                    let i = val as i64;
                    let smi_raw = ((i as u64) << 1) | 1;
                    mov_imm64(&mut self.mem, 0, smi_raw);
                    self.push();
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
                    mov_reg(&mut self.mem, 1, 0); // x1 = b
                    self.pop(); // x0 = a
                    sub_imm(&mut self.mem, 0, 0, 1); // untag a
                    add_reg(&mut self.mem, 0, 0, 1); // x0 = a + b
                    self.push();
                }
                Opcode::Sub => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    sub_reg(&mut self.mem, 0, 0, 1);
                    add_imm(&mut self.mem, 0, 0, 1); // retag
                    self.push();
                }
                Opcode::Mul => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    emit(&mut self.mem, 0x9341FC00); // ASR x0, x0, #1
                    emit(&mut self.mem, 0x9341FC21); // ASR x1, x1, #1
                    emit(&mut self.mem, 0x9B017C00); // MUL x0, x0, x1
                    emit(&mut self.mem, 0xD37FF800); // LSL x0, x0, #1
                    add_imm(&mut self.mem, 0, 0, 1);
                    self.push();
                }
                Opcode::Lt => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    cmp_reg(&mut self.mem, 0, 1);
                    // CSET x0, LT = CSINC x0, XZR, XZR, GE (= !LT)
                    emit(&mut self.mem, 0x9A9FA7E0);
                    emit(&mut self.mem, 0xD37FF800); // LSL x0, x0, #1
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::Gt => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    cmp_reg(&mut self.mem, 0, 1);
                    // CSET x0, GT = CSINC x0, XZR, XZR, LE (= !GT)
                    emit(&mut self.mem, 0x9A9FD7E0);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::Le => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    cmp_reg(&mut self.mem, 0, 1);
                    // CSET x0, LE = CSINC x0, XZR, XZR, GT (= !LE)
                    emit(&mut self.mem, 0x9A9FC7E0);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::Ge => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    cmp_reg(&mut self.mem, 0, 1);
                    // CSET x0, GE = CSINC x0, XZR, XZR, LT (= !GE)
                    emit(&mut self.mem, 0x9A9FB7E0);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::StrictEq => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    cmp_reg(&mut self.mem, 0, 1);
                    // CSET x0, EQ = CSINC x0, XZR, XZR, NE (= !EQ)
                    emit(&mut self.mem, 0x9A9F17E0);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::Shl => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    // Untag both: ASR #1 decodes Smi → int32
                    emit(&mut self.mem, 0x9341FC00); // ASR x0, x0, #1
                    emit(&mut self.mem, 0x9341FC21); // ASR x1, x1, #1
                    lsl_reg(&mut self.mem, 0, 0, 1);  // LSL x0, x0, x1
                    // Retag: LSL #1; ORR #1
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::Shr => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    emit(&mut self.mem, 0x9341FC00);
                    emit(&mut self.mem, 0x9341FC21);
                    asr_reg(&mut self.mem, 0, 0, 1);  // ASR x0, x0, x1
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::BitAnd => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    emit(&mut self.mem, 0x9341FC00);
                    emit(&mut self.mem, 0x9341FC21);
                    and_reg(&mut self.mem, 0, 0, 1);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::BitOr => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
                    emit(&mut self.mem, 0x9341FC00);
                    emit(&mut self.mem, 0x9341FC21);
                    orr_reg(&mut self.mem, 0, 0, 1);
                    emit(&mut self.mem, 0xD37FF800);
                    orr_imm1(&mut self.mem, 0, 0);
                    self.push();
                }
                Opcode::BitXor => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
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
                    mov_reg(&mut self.mem, 1, 0); // x1 = b
                    self.pop(); // x0 = a
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
                    mov_reg(&mut self.mem, 1, 0); // x1 = b
                    self.pop(); // x0 = a
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
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
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
                    movz(&mut self.mem, 1, 2);
                    cmp_reg(&mut self.mem, 0, 1);
                    emit(&mut self.mem, 0x54000049); // B.LS +2 (falsy: skip B)
                    movz(&mut self.mem, 1, 4);
                    cmp_reg(&mut self.mem, 0, 1);
                    emit(&mut self.mem, 0x54000020); // B.EQ +1 (falsy: skip B)
                    self.emit_b(target);
                }
                Opcode::LoadPropertyIC => {
                    let shape_id = instr.operands[0] as u64;
                    let offset = instr.operands[1] as u32;
                    let _proto_depth = instr.operands.get(2).copied().unwrap_or(0) as u32;
                    self.pop(); // x0 = object
                    // Save object pointer in x1 for validation + property load
                    mov_reg(&mut self.mem, 1, 0);
                    // Test bit 0: Smi → miss
                    movz(&mut self.mem, 2, 1);          // x2 = 1
                    emit(&mut self.mem, 0xEA02003F);    // TST x1, x2 (ANDS XZR, X1, X2)
                    let patch_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000001);    // B.NE +0 (patched → miss)
                    // CMP x1, #6 (sentinel ≤ 6 → miss)
                    emit(&mut self.mem, 0xF100183F);    // CMP x1, #6 (SUBS XZR, X1, #6)
                    let patch_sentinel = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000009);    // B.LS +0 (patched → miss)
                    // Load shape ptr from [x1 + 8]
                    ldr_off(&mut self.mem, 2, 1, 8);    // x2 = [x1 + 8] (shape ptr)
                    // Load shape.id from [x2]
                    ldr_off(&mut self.mem, 3, 2, 0);    // x3 = [x2] (shape.id)
                    // Compare with expected shape_id
                    mov_imm64(&mut self.mem, 4, shape_id);
                    cmp_reg(&mut self.mem, 3, 4);
                    let patch_shape = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000001);    // B.NE +0 (patched → miss)
                    // Load property from [x1 + 32 + offset*8]
                    ldr_off(&mut self.mem, 0, 1, 32 + offset * 8);
                    self.push();
                    // B done (skip miss handler)
                    let patch_done = self.mem.current_offset();
                    emit(&mut self.mem, 0x14000000);    // B +0 (patched → done)
                    // miss: push undefined (= 0)
                    let miss_offset = self.mem.current_offset();
                    movz(&mut self.mem, 0, 0);
                    self.push();
                    // done:
                    let done_offset = self.mem.current_offset();
                    // Patch forward jumps
                    // B.NE (smi check, cond=1)
                    let d = ((miss_offset as i64 - patch_smi as i64) / 4) as u32;
                    self.mem.patch_u32(patch_smi, 0x54000001 | ((d & 0x7FFFF) << 5));
                    // B.LS (sentinel check, cond=9)
                    let d = ((miss_offset as i64 - patch_sentinel as i64) / 4) as u32;
                    self.mem.patch_u32(patch_sentinel, 0x54000009 | ((d & 0x7FFFF) << 5));
                    // B.NE (shape mismatch, cond=1)
                    let d = ((miss_offset as i64 - patch_shape as i64) / 4) as u32;
                    self.mem.patch_u32(patch_shape, 0x54000001 | ((d & 0x7FFFF) << 5));
                    // B (unconditional to done)
                    let d = ((done_offset as i64 - patch_done as i64) / 4) as u32;
                    self.mem.patch_u32(patch_done, 0x14000000 | (d & 0x03FF_FFFF));
                }
                Opcode::StorePropertyIC => {
                    let shape_id = instr.operands[0] as u64;
                    let offset = instr.operands[1] as u32;
                    let _proto_depth = instr.operands.get(2).copied().unwrap_or(0) as u32;
                    self.pop(); // x0 = value
                    // Save value in x1
                    mov_reg(&mut self.mem, 1, 0);
                    // Pop object (skip key — offset cached in instruction)
                    self.pop(); // x0 = object
                    mov_reg(&mut self.mem, 2, 0); // x2 = object
                    // Test bit 0: Smi → miss
                    movz(&mut self.mem, 3, 1);          // x3 = 1
                    emit(&mut self.mem, 0xEA03005F);    // TST x2, x3 (ANDS XZR, X2, X3)
                    let patch_smi = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000001);    // B.NE +0 (patched → miss)
                    // CMP x2, #6 (sentinel ≤ 6 → miss)
                    emit(&mut self.mem, 0xF100185F);    // CMP x2, #6 (SUBS XZR, X2, #6)
                    let patch_sentinel = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000009);    // B.LS +0 (patched → miss)
                    // Load shape ptr from [x2 + 8]
                    ldr_off(&mut self.mem, 4, 2, 8);    // x4 = [x2 + 8] (shape ptr)
                    ldr_off(&mut self.mem, 5, 4, 0);    // x5 = [x4] (shape.id)
                    mov_imm64(&mut self.mem, 6, shape_id);
                    cmp_reg(&mut self.mem, 5, 6);
                    let patch_shape = self.mem.current_offset();
                    emit(&mut self.mem, 0x54000001);    // B.NE +0 (patched → miss)
                    // Store value to [x2 + 32 + offset*8]
                    str_off(&mut self.mem, 1, 2, 32 + offset * 8);
                    // Push value back (JS: store returns the value)
                    mov_reg(&mut self.mem, 0, 1);
                    self.push();
                    // B done
                    let patch_done = self.mem.current_offset();
                    emit(&mut self.mem, 0x14000000);    // B +0
                    // miss: push value back (same as hit — shape miss is rare)
                    let miss_offset = self.mem.current_offset();
                    mov_reg(&mut self.mem, 0, 1);
                    self.push();
                    // done:
                    let done_offset = self.mem.current_offset();
                    // Patch forward jumps
                    let d = ((miss_offset as i64 - patch_smi as i64) / 4) as u32;
                    self.mem.patch_u32(patch_smi, 0x54000001 | ((d & 0x7FFFF) << 5));
                    let d = ((miss_offset as i64 - patch_sentinel as i64) / 4) as u32;
                    self.mem.patch_u32(patch_sentinel, 0x54000009 | ((d & 0x7FFFF) << 5));
                    let d = ((miss_offset as i64 - patch_shape as i64) / 4) as u32;
                    self.mem.patch_u32(patch_shape, 0x54000001 | ((d & 0x7FFFF) << 5));
                    let d = ((done_offset as i64 - patch_done as i64) / 4) as u32;
                    self.mem.patch_u32(patch_done, 0x14000000 | (d & 0x03FF_FFFF));
                }
                Opcode::Return => {
                    self.emit_epilogue();
                }
                Opcode::Neg => {
                    // Smi(-n) = -(2n+1) + 2 = -2n + 1 = Smi(-n)
                    // neg x0; add x0, #2
                    self.pop();
                    sub_reg(&mut self.mem, 0, 31, 0); // SUB x0, XZR, x0 (= NEG)
                    add_imm(&mut self.mem, 0, 0, 2);
                    self.push();
                }
                Opcode::Not => {
                    self.pop();
                    mov_reg(&mut self.mem, 2, 0);   // x2 = original
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
                }
                Opcode::BitNot => {
                    // Smi(~n) = ~Smi(n) + 1
                    self.pop();
                    emit(&mut self.mem, 0xAA2003E0); // MVN x0, x0 (ORN x0, xzr, x0)
                    add_imm(&mut self.mem, 0, 0, 1);
                    self.push();
                }
                Opcode::StrictNe => {
                    self.pop();
                    mov_reg(&mut self.mem, 1, 0);
                    self.pop();
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
                    // PR1: bail on entry — always deopt to interpreter.
                    self.record_bailout_point(bc_idx, BailoutReason::BailOnEntry);
                    // x0 = vm_ptr, x1 = bc_pc, x2 = current_jit_sp
                    mov_reg(&mut self.mem, 2, JIT_STACK_REG);
                    mov_imm64(&mut self.mem, 1, bc_idx as u64);
                    mov_reg(&mut self.mem, 0, VM_REG);
                    ldr_off(&mut self.mem, 15, VM_REG, 520); // bailout_helper
                    emit(&mut self.mem, 0xD63F01E0);          // BLR x15
                    // Push a safe return value (undefined) before epilogue.
                    movz(&mut self.mem, 0, 0);
                    self.push();
                    self.emit_epilogue();
                }
                _ => {
                    // Unknown opcode: emit a trap so we notice quickly.
                    emit(&mut self.mem, 0xD4200000); // BRK #0
                }
            }
        }

        self.resolve_patches();

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

/// Compile a single trace opcode to aarch64 instructions.
#[allow(clippy::identity_op)] // instruction encoding uses explicit bit-field slots
fn compile_op(mem: &mut ExecutableMemory, opcode: Opcode, operands: &[i64]) {
    match opcode {
        Opcode::LoadSmi => {
            let val = operands[0];
            let smi_raw = ((val as u64) << 1) | 1;
            mov_imm64(mem, 0, smi_raw); // x0 = smi
            str_off(mem, 0, JIT_STACK_REG, 0); // str x0, [jit_stack]
            add_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8); // advance
        }
        Opcode::LoadUndefined => {
            movz(mem, 0, 0); // x0 = 0 (undefined)
            str_off(mem, 0, JIT_STACK_REG, 0);
            add_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        }
        Opcode::LoadNull => {
            // Load null value: 2
            movz(mem, 0, 2);
            str_off(mem, 0, JIT_STACK_REG, 0);
            add_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        }
        Opcode::LoadBoolean => {
            let raw = if operands[0] != 0 { 6u64 } else { 4u64 };
            mov_imm64(mem, 0, raw); // x0 = true(6) or false(4)
            str_off(mem, 0, JIT_STACK_REG, 0);
            add_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        }
        Opcode::LoadLocal => {
            let idx = operands[0] as u32;
            ldr_off(mem, 0, LOC_REG, idx * 8); // ldr x0, [x21, #idx*8]
            str_off(mem, 0, JIT_STACK_REG, 0);
            add_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        }
        Opcode::StoreLocal => {
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8); // pop
            ldr_off(mem, 0, JIT_STACK_REG, 0); // ldr x0, [jit_stack]
            let idx = operands[0] as u32;
            str_off(mem, 0, LOC_REG, idx * 8); // str x0, [x21, #idx*8]
        }
        Opcode::Pop => {
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        }
        Opcode::Add => {
            // pop b
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
            ldr_off(mem, 1, JIT_STACK_REG, 0); // x1 = b
            // pop a
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
            ldr_off(mem, 0, JIT_STACK_REG, 0); // x0 = a
            // Smi add: (a - 1) + b  (untag a, then add b)
            sub_imm(mem, 0, 0, 1); // x0 = a - 1 (clear smi tag)
            add_reg(mem, 0, 0, 1); // x0 = (a-1) + b
            // push result
            str_off(mem, 0, JIT_STACK_REG, 0);
            add_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        }
        Opcode::Sub => {
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
            ldr_off(mem, 1, JIT_STACK_REG, 0); // x1 = b
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
            ldr_off(mem, 0, JIT_STACK_REG, 0); // x0 = a
            sub_reg(mem, 0, 0, 1); // x0 = a - b
            add_imm(mem, 0, 0, 1); // x0 |= 1 (re-tag, same as ORR #1 since bit0 is 0)
            str_off(mem, 0, JIT_STACK_REG, 0);
            add_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        }
        Opcode::Mul => {
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
            ldr_off(mem, 1, JIT_STACK_REG, 0); // x1 = b
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
            ldr_off(mem, 0, JIT_STACK_REG, 0); // x0 = a
            emit(mem, 0x9341FC00); // ASR x0, x0, #1
            emit(mem, 0x9341FC21); // ASR x1, x1, #1
            emit(mem, 0x9B017C00); // MUL x0, x0, x1
            emit(mem, 0xD37FF800); // LSL x0, x0, #1
            add_imm(mem, 0, 0, 1); // x0 |= 1
            str_off(mem, 0, JIT_STACK_REG, 0);
            add_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        }
        Opcode::Lt => {
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
            ldr_off(mem, 1, JIT_STACK_REG, 0); // x1 = b
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
            ldr_off(mem, 0, JIT_STACK_REG, 0); // x0 = a
            cmp_reg(mem, 0, 1); // CMP a, b
            // CSET x0, LT = CSINC x0, XZR, XZR, GE (= !LT)
            emit(mem, 0x9A9FA7E0);
            emit(mem, 0xD37FF800); // LSL x0, x0, #1
            orr_imm1(mem, 0, 0); // x0 |= 1
            str_off(mem, 0, JIT_STACK_REG, 0);
            add_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        }
        Opcode::IncLocal => {
            let idx = operands[0] as u32;
            ldr_off(mem, 0, LOC_REG, idx * 8); // ldr x0, [x21, #idx*8]
            add_imm(mem, 0, 0, 2); // x0 += 2 (Smi +1)
            str_off(mem, 0, LOC_REG, idx * 8); // str x0, [x21, #idx*8]
        }
        Opcode::DecLocal => {
            let idx = operands[0] as u32;
            ldr_off(mem, 0, LOC_REG, idx * 8);
            sub_imm(mem, 0, 0, 2); // x0 -= 2 (Smi -1)
            str_off(mem, 0, LOC_REG, idx * 8);
        }
        Opcode::Jump => {
            // No-op: the trace body runs sequentially; back-edge is handled by
            // the caller (loops continuously from body_start)
            nop(mem);
        }
        Opcode::Return => {
            // Value is in x0, epilogue handles the rest
        }
        Opcode::UnaryPlus => {
            // No-op for Smi: value stays on JIT stack
        }
        Opcode::BitNot => {
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8); // pop
            ldr_off(mem, 0, JIT_STACK_REG, 0);
            emit(mem, 0xAA2003E0); // MVN x0, x0 (ORN x0, xzr, x0)
            add_imm(mem, 0, 0, 1);
            str_off(mem, 0, JIT_STACK_REG, 0);
            add_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        }
        Opcode::StorePropertyIC => {
            // StorePropertyIC: pop value, pop key (discard), pop obj, shape guard, store, push value
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
            ldr_off(mem, 0, JIT_STACK_REG, 0); // x0 = value
            mov_reg(mem, 1, 0);                // x1 = saved value
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
            ldr_off(mem, 0, JIT_STACK_REG, 0); // x0 = key (discard)
            sub_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
            ldr_off(mem, 0, JIT_STACK_REG, 0); // x0 = object
            mov_reg(mem, 2, 0);                // x2 = saved object
            // Shape guard: TST x2, #1; CMP x2, #6
            movz(mem, 3, 1);
            emit(mem, 0xEA03005F);              // TST x2, x3
            // nop for guard (trace assumes hit)
            nop(mem);
            nop(mem);
            nop(mem);
            // Store value to slot
            str_off(mem, 1, 2, 32);             // [x2 + 32] = x1 (simplified slot 0)
            // Push value back
            str_off(mem, 1, JIT_STACK_REG, 0);
            add_imm(mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        }
        _ => {
            nop(mem); // unhandled opcode — trace-verified, shouldn't hit
        }
    }
}

#[cfg(all(test, target_arch = "aarch64"))]
mod tests {
    use super::*;

    /// Allocate a `JitVmState` on the heap and return a raw VM pointer.
    /// The trace compiler expects `jit_stack` to live at offset 0 from this
    /// pointer. Tests intentionally leak this small allocation.
    fn jit_vm_ptr() -> *mut u8 {
        let state = Box::new(JitVmState {
            jit_stack: [0; JIT_STACK_SIZE],
            jit_helpers: JitHelpers {
                lexical_helper: 0,
                bailout_helper: 0,
                _reserved: [0; 6],
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

    #[allow(dead_code)]
    fn test_trace_smi() {
        let mut mem = ExecutableMemory::allocate(4096);
        push_callee_saved(&mut mem);
        mov_reg(&mut mem, VM_REG, 0);
        mov_reg(&mut mem, GC_REG, 1);
        mov_reg(&mut mem, LOC_REG, 2);
        sub_imm(&mut mem, 31, 31, 64);
        mov_imm64(&mut mem, 0, 85);
        str_off(&mut mem, 0, 31, 0);
        add_imm(&mut mem, 31, 31, 8);
        sub_imm(&mut mem, 31, 31, 8);
        ldr_off(&mut mem, 0, 31, 0);
        add_imm(&mut mem, 31, 31, 64);
        pop_callee_saved(&mut mem);
        ret(&mut mem);
        mem.make_executable();

        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
        assert_eq!(
            unsafe {
                func(
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            85
        );
    }

    #[test]
    fn test_compile_trace_smi() {
        let vm = jit_vm_ptr();
        let mut buf = vec![0u64; 256];
        let mut mem = ExecutableMemory::allocate(4096);
        let ops = vec![(Opcode::LoadSmi, vec![42], 0)];
        emit_trace_into(&mut mem, &ops);
        mem.make_executable();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
        // Invoke several times to verify the trace is repeatable and the JIT
        // stack is reset correctly between invocations.
        unsafe { func(vm, std::ptr::null_mut(), buf.as_mut_ptr()) };
        unsafe { func(vm, std::ptr::null_mut(), buf.as_mut_ptr()) };
        unsafe { func(vm, std::ptr::null_mut(), buf.as_mut_ptr()) };
        unsafe { func(vm, std::ptr::null_mut(), buf.as_mut_ptr()) };
        unsafe { func(vm, std::ptr::null_mut(), buf.as_mut_ptr()) };
        unsafe { func(vm, std::ptr::null_mut(), buf.as_mut_ptr()) };
        let result = unsafe { func(vm, std::ptr::null_mut(), buf.as_mut_ptr()) };
        assert_eq!(result, ((42u64 << 1) | 1));
    }

    #[test]
    fn test_trace_minimal() {
        let mut mem = ExecutableMemory::allocate(4096);
        push_callee_saved(&mut mem);
        mov_imm64(&mut mem, 0, 85);
        pop_callee_saved(&mut mem);
        ret(&mut mem);
        mem.make_executable();
        let func: unsafe fn() -> u64 = unsafe { std::mem::transmute(mem.code_ptr()) };
        assert_eq!(unsafe { func() }, 85);
    }

    #[test]
    fn test_trace_with_stack() {
        // Allocate JIT stack, push/load value, restore, return
        let mut mem = ExecutableMemory::allocate(4096);
        push_callee_saved(&mut mem);
        mov_reg(&mut mem, VM_REG, 0);
        mov_reg(&mut mem, GC_REG, 1);
        mov_reg(&mut mem, LOC_REG, 2);
        sub_imm(&mut mem, 31, 31, 512); // alloc JIT stack

        // Push LoadSmi(42) to JIT stack: x0 = 85, STR x0, [sp], ADD sp, #8
        mov_imm64(&mut mem, 0, 85);
        str_off(&mut mem, 0, 31, 0);
        add_imm(&mut mem, 31, 31, 8);
        // Pop it back into x0: SUB sp, #8, LDR x0, [sp]
        sub_imm(&mut mem, 31, 31, 8);
        ldr_off(&mut mem, 0, 31, 0);

        add_imm(&mut mem, 31, 31, 512);
        pop_callee_saved(&mut mem);
        ret(&mut mem);
        mem.make_executable();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
        assert_eq!(
            unsafe {
                func(
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            85
        );
    }

    #[test]
    fn test_trace_add() {
        let vm = jit_vm_ptr();
        let mut buf = vec![0u64; 256];
        let ops = vec![
            (Opcode::LoadSmi, vec![10], 0),
            (Opcode::LoadSmi, vec![32], 0),
        ];
        let mut mem = ExecutableMemory::allocate(4096);
        emit_trace_into(&mut mem, &ops);
        mem.make_executable();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), buf.as_mut_ptr()) };
        assert_eq!(r, ((32u64 << 1) | 1)); // last pushed value (32)
    }

    #[test]
    fn test_trace_sub() {
        let vm = jit_vm_ptr();
        let mut buf = vec![0u64; 256];
        let ops = vec![
            (Opcode::LoadSmi, vec![50], 0),
            (Opcode::LoadSmi, vec![8], 0),
            (Opcode::Sub, vec![], 0),
        ];
        let mut mem = ExecutableMemory::allocate(4096);
        emit_trace_into(&mut mem, &ops);
        mem.make_executable();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), buf.as_mut_ptr()) };
        assert_eq!(r, ((42u64 << 1) | 1));
    }

    #[test]
    fn test_aarch64_cset_lt_encoding() {
        let mut mem = ExecutableMemory::allocate(256);
        // CMP x0, x1 (x0=a=1, x1=b=21); CSET x0, LT; RET
        mov_imm64(&mut mem, 0, 1);  // x0 = 1 = Smi(0)
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
        mov_imm64(&mut mem, 1, 1);  // x1 = 1 = Smi(0)
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
        // if (true) 42 else 7
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![1]),
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
        // if (false) 42 else 7
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![0]),
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
    fn test_aarch64_codegen_large_sum_65537_iterations() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // Sum 0..65536 = 2,147,516,416 → Smi = 4,295,032,833 (> 2^32).
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![2]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![3]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadLocal, vec![3]),
                Instruction::new(Opcode::LoadSmi, vec![65537]),
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
        let expected = (2147516416u64 << 1) | 1;
        assert_eq!(r, expected, "65537 iters: got {}, expected {}", r, expected);
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
                vec![], vec![],
            );
            let compiled = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
            compiled.mem.make_executable();
            let vm = jit_vm_ptr();
            let func: JF = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
            let r = unsafe { func(vm, std::ptr::null_mut(), locals.as_mut_ptr()) };
            assert_eq!(r, expected, "{:?} {} {}: expected {}, got {}", op, a, b, expected, r);
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
                vec![], vec![],
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
        // Same loop as trace-level, but locals at indices [2] and [3] (matching
        // the recorded trace). Uses 4-element locals vec.
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![2]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![3]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadLocal, vec![3]),
                Instruction::new(Opcode::LoadSmi, vec![70000]),
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
        // 4-element locals: [0]=unused, [1]=unused, [2]=s, [3]=i
        let mut locals: Vec<u64> = vec![0, 0, 0, 0];
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), locals.as_mut_ptr()) };
        let expected = (2449965000u64 << 1) | 1;
        assert_eq!(r, expected, "got {}, expected {}", r, expected);
    }

    #[test]
    fn test_aarch64_codegen_large_sum_loop() {
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};
        // Sum 0..70000 = 2,449,965,000 → Smi = 4,899,930,001 (> 2^32).
        // This exercises arithmetic across the 32-bit boundary.
        let prog = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![0]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::StoreLocal, vec![1]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadLocal, vec![1]),
                Instruction::new(Opcode::LoadSmi, vec![70000]),
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
        // sum 0..69999 = 2,449,965,000 → Smi = 4,899,930,001
        let expected = (2449965000u64 << 1) | 1;
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
                _reserved: [0; 6],
            },
            jit_stack_base: 0,
        };
        vm.jit_stack[0] = obj_addr;
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
            .with_jit_stack_offset(8)
            .compile(&prog);
        compiled.mem.make_executable();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm_ptr, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, slot_value);
        unsafe { drop(Box::from_raw(shape_ptr)); }
    }
}
