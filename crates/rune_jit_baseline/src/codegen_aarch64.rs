/// AArch64 native code emission for trace compilation.
///
/// ARM64 instructions are fixed 32-bit. All registers are 64-bit (x0-x30).
/// Callee-saved: x19-x28, x29(fp), x30(lr).
///
/// Calling convention (AAPCS64):
///   x0 = vm_ptr, x1 = gc_ptr, x2 = locals_ptr
///   return value in x0
use crate::assembler::ExecutableMemory;
use rune_bytecode::opcode::Opcode;

/// Register assignments for the trace compiler.
const VM_REG: u32 = 19; // callee-saved, holds Vm pointer
const GC_REG: u32 = 20; // callee-saved, holds GC pointer
const LOC_REG: u32 = 21; // callee-saved, holds locals pointer

/// Emit a full 32-bit instruction.
fn emit(mem: &mut ExecutableMemory, instr: u32) {
    mem.emit_byte(instr as u8);
    mem.emit_byte((instr >> 8) as u8);
    mem.emit_byte((instr >> 16) as u8);
    mem.emit_byte((instr >> 24) as u8);
}

/// MOV xd, xm  (ORR xd, xzr, xm)
fn mov_reg(mem: &mut ExecutableMemory, xd: u32, xm: u32) {
    emit(mem, 0xAA0003E0 | (xm << 16) | xd);
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
fn and_reg(mem: &mut ExecutableMemory, xd: u32, xn: u32, xm: u32) {
    emit(mem, 0x8A000000 | (xm << 16) | (xn << 5) | xd);
}

/// ORR xd, xn, #imm — only certain patterns work.
/// For simple mask like #1: use ORR immediate form
fn orr_imm1(mem: &mut ExecutableMemory, xd: u32, xn: u32) {
    // ORR xd, xn, #1 encoded as bitmask immediate
    // immr=0, imms=0, N=0 → encodes #1
    emit(mem, 0xB2400000 | ((1) << 10) | (xn << 5) | xd);
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
fn b_imm(mem: &mut ExecutableMemory, offset_in_instrs: i32) {
    emit(mem, 0x14000000 | ((offset_in_instrs as u32) & 0x3FF_FFFF));
}

/// B.EQ #offset  (conditional branch on equal)
fn b_eq(mem: &mut ExecutableMemory, offset_in_instrs: i32) {
    emit(
        mem,
        0x54000000 | (((offset_in_instrs as u32) & 0x7FFFF) << 5),
    );
}

/// B.NE #offset  (conditional branch on not equal)
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

/// Save callee-saved registers: x19-x26 (8 regs, 64 bytes)
fn push_callee_saved(mem: &mut ExecutableMemory) {
    // STP x29, x30, [sp, #-16]! — frame record
    emit(mem, 0xA9BF7BFD);
    // STP x19, x20, [sp, #-16]!
    emit(mem, 0xA9BF13F3);
    // STP x21, x22, [sp, #-16]!
    emit(mem, 0xA9BF17F5);
    // STP x23, x24, [sp, #-16]!
    emit(mem, 0xA9BF1BF7);
    // STP x25, x26, [sp, #-16]!
    emit(mem, 0xA9BF1FF9);
}

fn pop_callee_saved(mem: &mut ExecutableMemory) {
    // LDP x25, x26, [sp], #16
    emit(mem, 0xA8C11FF9);
    // LDP x23, x24, [sp], #16
    emit(mem, 0xA8C11BF7);
    // LDP x21, x22, [sp], #16
    emit(mem, 0xA8C117F5);
    // LDP x19, x20, [sp], #16
    emit(mem, 0xA8C113F3);
    // LDP x29, x30, [sp], #16 — frame record
    emit(mem, 0xA8C17BFD);
}

/// Compile a trace into the given ExecutableMemory buffer.
/// The caller is responsible for calling make_executable() and managing lifetime.
pub fn emit_trace_into(mem: &mut ExecutableMemory, ops: &[(Opcode, Vec<i64>, u64)]) {
    push_callee_saved(mem);
    mov_reg(mem, VM_REG, 0);
    mov_reg(mem, GC_REG, 1);
    mov_reg(mem, LOC_REG, 2);
    sub_imm(mem, 31, 31, 512);
    for &(ref opcode, ref operands, _shape_id) in ops {
        compile_op(mem, *opcode, operands);
    }
    sub_imm(mem, 31, 31, 8);
    ldr_off(mem, 0, 31, 0);
    add_imm(mem, 31, 31, 512);
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

/// Compile a single trace opcode to aarch64 instructions.
fn compile_op(mem: &mut ExecutableMemory, opcode: Opcode, operands: &[i64]) {
    match opcode {
        Opcode::LoadSmi => {
            let val = operands[0];
            let smi_raw = ((val as u64) << 1) | 1;
            mov_imm64(mem, 0, smi_raw); // x0 = smi
            str_off(mem, 0, 31, 0); // str x0, [sp, #0]
            add_imm(mem, 31, 31, 8); // ADD sp, sp, #8
        }
        Opcode::LoadUndefined => {
            movz(mem, 0, 0); // x0 = 0 (undefined)
            str_off(mem, 0, 31, 0);
            add_imm(mem, 31, 31, 8);
        }
        Opcode::LoadNull => {
            movz(mem, 0, 0);
            orr_imm1(mem, 0, 0); // x0 |= 2 — wait, ORR x0, x0, #1 = 1, not 2
            // Actually: ORR x0, x0, #2 is needed for null (0x02)
            // Load null value: 2
            movz(mem, 0, 2);
            str_off(mem, 0, 31, 0);
            add_imm(mem, 31, 31, 8);
        }
        Opcode::LoadBoolean => {
            let raw = if operands[0] != 0 { 6u64 } else { 4u64 };
            mov_imm64(mem, 0, raw); // x0 = true(6) or false(4)
            str_off(mem, 0, 31, 0);
            add_imm(mem, 31, 31, 8);
        }
        Opcode::LoadLocal => {
            let idx = operands[0] as u32;
            ldr_off(mem, 0, LOC_REG, idx * 8); // ldr x0, [x21, #idx*8]
            str_off(mem, 0, 31, 0);
            add_imm(mem, 31, 31, 8);
        }
        Opcode::StoreLocal => {
            sub_imm(mem, 31, 31, 8); // pop
            ldr_off(mem, 0, 31, 0); // ldr x0, [sp]
            let idx = operands[0] as u32;
            str_off(mem, 0, LOC_REG, idx * 8); // str x0, [x21, #idx*8]
        }
        Opcode::Pop => {
            sub_imm(mem, 31, 31, 8);
        }
        Opcode::Add => {
            // pop b
            sub_imm(mem, 31, 31, 8);
            ldr_off(mem, 1, 31, 0); // x1 = b
            // pop a
            sub_imm(mem, 31, 31, 8);
            ldr_off(mem, 0, 31, 0); // x0 = a
            // Smi add: (a - 1) + b  (untag a, then add b)
            sub_imm(mem, 0, 0, 1); // x0 = a - 1 (clear smi tag)
            add_reg(mem, 0, 0, 1); // x0 = (a-1) + b
            // push result
            str_off(mem, 0, 31, 0);
            add_imm(mem, 31, 31, 8);
        }
        Opcode::Sub => {
            sub_imm(mem, 31, 31, 8);
            ldr_off(mem, 1, 31, 0); // x1 = b
            sub_imm(mem, 31, 31, 8);
            ldr_off(mem, 0, 31, 0); // x0 = a
            sub_reg(mem, 0, 0, 1); // x0 = a - b
            orr_imm1(mem, 0, 0); // x0 |= 1 (re-tag)
            str_off(mem, 0, 31, 0);
            add_imm(mem, 31, 31, 8);
        }
        Opcode::Mul => {
            sub_imm(mem, 31, 31, 8);
            ldr_off(mem, 1, 31, 0); // x1 = b
            sub_imm(mem, 31, 31, 8);
            ldr_off(mem, 0, 31, 0); // x0 = a
            // Smi mul: (a >> 1) * (b >> 1) << 1 | 1
            // ASR x0, x0, #1
            emit(mem, 0x9341FC00 | (0 << 5) | 0); // ASR x0, x0, #1
            emit(mem, 0x9341FC21 | (1 << 5) | 1); // ASR x1, x1, #1
            // MUL x0, x0, x1
            emit(mem, 0x9B007C00 | (1 << 16) | (0 << 5) | 0); // MUL x0, x0, x1
            // LSL x0, x0, #1
            emit(mem, 0xD37EF800 | (0 << 5) | 0); // LSL x0, x0, #1
            orr_imm1(mem, 0, 0); // x0 |= 1
            str_off(mem, 0, 31, 0);
            add_imm(mem, 31, 31, 8);
        }
        Opcode::Lt => {
            sub_imm(mem, 31, 31, 8);
            ldr_off(mem, 1, 31, 0); // x1 = b
            sub_imm(mem, 31, 31, 8);
            ldr_off(mem, 0, 31, 0); // x0 = a
            cmp_reg(mem, 0, 1); // CMP a, b
            // CSET x0, LT → x0 = 1 if LT else 0
            // Then encode as Smi: x0 = (x0 << 1) | 1
            emit(mem, 0x9A9FB7E0); // CSET x0, LT
            emit(mem, 0xD37EF800 | (0 << 5) | 0); // LSL x0, x0, #1
            orr_imm1(mem, 0, 0); // x0 |= 1
            str_off(mem, 0, 31, 0);
            add_imm(mem, 31, 31, 8);
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
        let mut mem = ExecutableMemory::allocate(4096);
        let ops = vec![(Opcode::LoadSmi, vec![42], 0)];
        emit_trace_into(&mut mem, &ops);
        mem.make_executable();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe {
            func(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
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
        let ops = vec![
            (Opcode::LoadSmi, vec![10], 0),
            (Opcode::LoadSmi, vec![32], 0),
            (Opcode::Add, vec![], 0),
        ];
        let mut mem = ExecutableMemory::allocate(4096);
        emit_trace_into(&mut mem, &ops);
        mem.make_executable();
        let func: unsafe fn(*mut u8, *mut u8, *mut u64) -> u64 =
            unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe {
            func(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, ((42u64 << 1) | 1));
    }

    #[test]
    fn test_trace_sub() {
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
        let result = unsafe {
            func(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, ((42u64 << 1) | 1));
    }
}
