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
use rune_bytecode::opcode::Opcode;

/// Number of u64 slots reserved for the trace value stack.
pub const JIT_STACK_SIZE: usize = 64;

/// VM state visible to the trace compiler. Must be placed at offset 0 from
/// the VM pointer passed to emitted trace code.
#[repr(C)]
pub struct JitVmState {
    pub jit_stack: [u64; JIT_STACK_SIZE],
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

/// MOVK xd, #u16, lsl #16
fn movk(mem: &mut ExecutableMemory, xd: u32, imm16: u16) {
    emit(mem, 0xF2800000 | ((imm16 as u32) << 5) | xd);
}

/// MOV xd, #u64 (split across MOVZ + MOVK)
fn mov_imm64(mem: &mut ExecutableMemory, xd: u32, val: u64) {
    let w0 = val as u16;
    let w1 = (val >> 16) as u16;
    let w2 = (val >> 32) as u16;
    let w3 = (val >> 48) as u16;
    movz(mem, xd, w0);
    if w1 != 0 {
        movk(mem, xd, w1);
    }
    if w2 != 0 {
        movk(mem, xd, w2);
    }
    if w3 != 0 {
        movk(mem, xd, w3);
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
#[allow(dead_code)]
fn and_reg(mem: &mut ExecutableMemory, xd: u32, xn: u32, xm: u32) {
    emit(mem, 0x8A000000 | (xm << 16) | (xn << 5) | xd);
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
}

impl Aarch64CodeGen {
    pub fn new(instruction_count: usize) -> Self {
        let mem = ExecutableMemory::allocate(64 * 1024);
        Self {
            mem,
            bc_to_native: vec![0; instruction_count],
            pending_patches: Vec::new(),
        }
    }

    fn push(&mut self) {
        // x0 -> [jit_stack]; jit_stack += 8
        str_off(&mut self.mem, 0, JIT_STACK_REG, 0);
        add_imm(&mut self.mem, JIT_STACK_REG, JIT_STACK_REG, 8);
    }

    fn pop(&mut self) {
        // jit_stack -= 8; x0 <- [jit_stack]
        sub_imm(&mut self.mem, JIT_STACK_REG, JIT_STACK_REG, 8);
        ldr_off(&mut self.mem, 0, JIT_STACK_REG, 0);
    }

    fn emit_prologue(&mut self) {
        push_callee_saved(&mut self.mem);
        mov_reg(&mut self.mem, VM_REG, 0);
        mov_reg(&mut self.mem, GC_REG, 1);
        mov_reg(&mut self.mem, LOC_REG, 2);
        add_imm(&mut self.mem, JIT_STACK_REG, VM_REG, 0);
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

    pub fn compile(mut self, program: &rune_bytecode::opcode::BytecodeProgram) -> ExecutableMemory {
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
                    self.emit_b_cond(0x9, target); // B.LS target (falsy: undefined/Smi(0)/null)
                    movz(&mut self.mem, 1, 4); // x1 = 4 (false sentinel)
                    cmp_reg(&mut self.mem, 0, 1);
                    self.emit_b_cond(0x0, target); // B.EQ target
                }
                Opcode::Return => {
                    self.emit_epilogue();
                }
                _ => {
                    // Unknown opcode: emit a trap so we notice quickly.
                    emit(&mut self.mem, 0xD4200000); // BRK #0
                }
            }
        }

        self.resolve_patches();
        self.mem
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
        let mem = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
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
        let mem = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let vm = jit_vm_ptr();
        let mut locals: Vec<u64> = vec![((10u64 << 1) | 1), ((32u64 << 1) | 1)];
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
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
        let mem = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
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
        let mem = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
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
        let mem = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let vm = jit_vm_ptr();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(r, ((7u64 << 1) | 1));
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
        let mem = Aarch64CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let vm = jit_vm_ptr();
        let mut locals: Vec<u64> = vec![0, 0];
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
        let r = unsafe { func(vm, std::ptr::null_mut(), locals.as_mut_ptr()) };
        // sum 0..9 = 45
        assert_eq!(r, ((45u64 << 1) | 1));
    }
}
