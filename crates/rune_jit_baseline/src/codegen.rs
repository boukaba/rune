/// Bytecode-to-machine-code compiler (copy-and-patch).
///
/// Registers (callee-saved):
///   R15 — VM pointer (from RDI)
///   R14 — GC SemiSpace pointer (from RSI)
///   R13 — locals pointer (from RDX)
///   RBX — JIT value stack pointer (points into native stack)
///
/// The JIT allocates a value stack on the native stack in the prologue
/// (256 Value slots = 2 KB), and restores it in the epilogue.

use crate::assembler::ExecutableMemory;
use rune_bytecode::opcode::{BytecodeProgram, Opcode};

/// Size of the JIT local value stack (256 Value × 8 bytes).
const JIT_STACK_SIZE: i32 = 256 * 8;

/// A JIT-compiled function entry point.
///
/// Arguments (System V AMD64 ABI):
///   RDI = vm_ptr
///   RSI = gc_ptr
///   RDX = locals_ptr (pointer to current frame's Vec<Value>)
///
/// Returns a raw u64 Value.
///
/// # Safety
///
/// Callers must pass valid pointers and ensure the code is executable.
pub type JitEntryFn = unsafe fn(vm_ptr: *mut u8, gc_ptr: *mut u8, locals_ptr: *mut u64) -> u64;

pub struct CodeGen {
    mem: ExecutableMemory,
    bc_to_native: Vec<usize>,
    pending_patches: Vec<(usize, usize)>,
}

impl CodeGen {
    pub fn new(instruction_count: usize) -> Self {
        let mem = ExecutableMemory::allocate(64 * 1024);
        CodeGen {
            mem,
            bc_to_native: vec![0; instruction_count],
            pending_patches: Vec::new(),
        }
    }

    /// Resolve all pending forward jumps once native offsets are known.
    fn resolve_patches(&mut self) {
        for &(patch_offset, bc_target) in &self.pending_patches {
            let native_target = self.bc_to_native[bc_target];
            let rel32 = (native_target as i64) - ((patch_offset as i64) + 4);
            self.mem.patch_u32(patch_offset, rel32 as u32);
        }
        self.pending_patches.clear();
    }

    // -----------------------------------------------------------------------
    // JIT value stack helpers
    // -----------------------------------------------------------------------

    fn emit_jit_stack_push(&mut self) {
        // mov [rbx], rax  → REX.W 89 03  (mod=00, reg=0(rax), r/m=3(rbx))
        self.mem.emit_rex_w();
        self.mem.emit_byte(0x89);
        self.mem.emit_byte(0x03);
        // add rbx, 8
        self.mem.emit_add_r64_imm32(3, 8);
    }

    fn emit_jit_stack_pop(&mut self) {
        // sub rbx, 8
        self.mem.emit_sub_r64_imm32(3, 8);
        // mov rax, [rbx]  → REX.W 8B 03  (mod=00, reg=0(rax), r/m=3(rbx))
        self.mem.emit_rex_w();
        self.mem.emit_byte(0x8B);
        self.mem.emit_byte(0x03);
    }

    // -----------------------------------------------------------------------
    // Prologue / epilogue
    // -----------------------------------------------------------------------

    fn emit_prologue(&mut self) {
        self.mem.emit_push_r64(5);   // push rbp
        self.mem.emit_push_r64(15);  // push r15 (VM ptr)
        self.mem.emit_push_r64(14);  // push r14 (GC ptr)
        self.mem.emit_push_r64(13);  // push r13 (locals ptr)
        self.mem.emit_push_r64(3);   // push rbx (JIT stack ptr)
        self.mem.emit_mov_r64_rm64(15, 7); // r15 = rdi (VM ptr)
        self.mem.emit_mov_r64_rm64(14, 6); // r14 = rsi (GC ptr)
        self.mem.emit_mov_r64_rm64(13, 2); // r13 = rdx (locals ptr)
        self.mem.emit_sub_r64_imm32(4, JIT_STACK_SIZE); // sub rsp, 2048
        self.mem.emit_mov_r64_rm64(3, 4);  // rbx = rsp
    }

    fn emit_epilogue(&mut self) {
        self.mem.emit_add_r64_imm32(4, JIT_STACK_SIZE); // add rsp, 2048
        self.mem.emit_pop_r64(3);   // pop rbx
        self.mem.emit_pop_r64(13);  // pop r13
        self.mem.emit_pop_r64(14);  // pop r14
        self.mem.emit_pop_r64(15);  // pop r15
        self.mem.emit_pop_r64(5);   // pop rbp
        self.mem.emit_ret();
    }

    // -----------------------------------------------------------------------
    // Smi arithmetic helpers
    // -----------------------------------------------------------------------

    /// Pop two Smis from the JIT stack and add them:
    ///   (a & ~1) + b
    ///
    /// Clears the tag bit of `a` before adding `b` so the result tag is correct.
    fn emit_smi_add(&mut self) {
        self.emit_jit_stack_pop();           // rax = b
        self.mem.emit_mov_r64_rm64(1, 0);    // rcx = b
        self.emit_jit_stack_pop();           // rax = a
        self.mem.emit_and_r64_imm8(0, -2);   // rax = a & ~1
        self.mem.emit_add_r64_r64(0, 1);     // rax += rcx
    }

    /// Pop two Smis and subtract (a - b):
    ///   (a - b) | 1
    fn emit_smi_sub(&mut self) {
        self.emit_jit_stack_pop();           // rax = b
        self.mem.emit_mov_r64_rm64(1, 0);    // rcx = b
        self.emit_jit_stack_pop();           // rax = a
        self.mem.emit_sub_r64_r64(0, 1);     // rax -= rcx
        self.mem.emit_or_r64_imm8(0, 1);     // rax |= 1
    }

    /// Pop two Smis and multiply (a * b):
    ///   decode → mul → encode
    fn emit_smi_mul(&mut self) {
        self.emit_jit_stack_pop();           // rax = b
        self.mem.emit_mov_r64_rm64(1, 0);    // rcx = b
        self.emit_jit_stack_pop();           // rax = a
        self.mem.emit_sar_r64_1(0);          // rax >>= 1 (a)
        self.mem.emit_sar_r64_1(1);          // rcx >>= 1 (b)
        self.mem.emit_imul_r64_r64(0, 1);    // rax *= rcx
        self.mem.emit_shl_r64_1(0);          // rax <<= 1
        self.mem.emit_or_r64_imm8(0, 1);     // rax |= 1
    }

    // -----------------------------------------------------------------------
    // Main compilation
    // -----------------------------------------------------------------------

    /// Compile a `BytecodeProgram` into native code.
    ///
    /// On completion the returned `ExecutableMemory` is still in writable
    /// state — the caller must call `make_executable()` before execution.
    pub fn compile(mut self, program: &BytecodeProgram) -> ExecutableMemory {
        self.emit_prologue();

        for (bc_idx, instr) in program.instructions.iter().enumerate() {
            self.bc_to_native[bc_idx] = self.mem.current_offset();

            match instr.opcode {
                Opcode::LoadSmi => {
                    let smi_raw = ((instr.operands[0] as i64) << 1) | 1;
                    self.mem.emit_mov_r64_imm64(0, smi_raw as u64);
                    self.emit_jit_stack_push();
                }
                Opcode::LoadUndefined => {
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x31);
                    self.mem.emit_byte(0xC0);
                    self.emit_jit_stack_push();
                }
                Opcode::LoadNull => {
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x31);
                    self.mem.emit_byte(0xC0);
                    self.mem.emit_or_r64_imm8(0, 2);
                    self.emit_jit_stack_push();
                }
                Opcode::LoadBoolean => {
                    let val = if instr.operands[0] != 0 { 7u64 } else { 3u64 };
                    self.mem.emit_mov_r64_imm64(0, val);
                    self.emit_jit_stack_push();
                }
                Opcode::Return => {
                    self.emit_jit_stack_pop();
                    self.emit_epilogue();
                }
                Opcode::Add => {
                    self.emit_smi_add();
                    self.emit_jit_stack_push();
                }
                Opcode::Sub => {
                    self.emit_smi_sub();
                    self.emit_jit_stack_push();
                }
                Opcode::Mul => {
                    self.emit_smi_mul();
                    self.emit_jit_stack_push();
                }
                Opcode::Jump => {
                    let target = instr.operands[0] as usize;
                    let patch = self.mem.emit_jmp_rel32(0);
                    self.pending_patches.push((patch, target));
                }
                Opcode::JumpIfFalse => {
                    let target = instr.operands[0] as usize;
                    self.emit_jit_stack_pop();               // rax = condition
                    self.mem.emit_mov_r64_imm64(1, 2);       // rcx = 2
                    self.mem.emit_cmp_r64_r64(0, 1);          // cmp rax, rcx
                    let patch = self.mem.emit_jbe_rel32(0);   // jbe target (falsy)
                    self.pending_patches.push((patch, target));
                }
                Opcode::LoadLocal => {
                    let idx = instr.operands[0] as usize;
                    let disp = (idx * 8) as i32;
                    self.mem.emit_mov_r64_mem_disp32(0, 13, disp); // rax = locals[idx]
                    self.emit_jit_stack_push();
                }
                Opcode::StoreLocal => {
                    let idx = instr.operands[0] as usize;
                    let disp = (idx * 8) as i32;
                    self.emit_jit_stack_pop();                    // rax = value
                    self.mem.emit_mov_mem_disp32_r64(13, disp, 0); // locals[idx] = rax
                    self.emit_jit_stack_push();                   // push value back
                }
                Opcode::Pop => {
                    self.emit_jit_stack_pop();
                }
                Opcode::Lt => {
                    self.emit_jit_stack_pop();                    // rax = b
                    self.mem.emit_mov_r64_rm64(1, 0);             // rcx = b
                    self.emit_jit_stack_pop();                    // rax = a
                    self.mem.emit_cmp_r64_r64(0, 1);              // cmp a, b
                    // setl al -> movzx eax, al -> shl eax, 1 -> or rax, 1
                    // setl (0F 9C /0) sets al = 1 if a < b (signed), 0 otherwise
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0x9C);
                    self.mem.emit_byte(0xC0);                     // setl al
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0xB6);
                    self.mem.emit_byte(0xC0);                     // movzx eax, al
                    self.mem.emit_byte(0xD1);
                    self.mem.emit_byte(0xE0);                     // shl eax, 1
                    self.mem.emit_or_r64_imm8(0, 1);              // or rax, 1
                    self.emit_jit_stack_push();
                }
                Opcode::IncLocal => {
                    let idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    let disp = (idx * 8) as i32;
                    // Load old value
                    self.mem.emit_mov_r64_mem_disp32(0, 13, disp); // rax = locals[idx]
                    self.mem.emit_mov_r64_rm64(1, 0);             // rcx = old
                    // Smi increment: old_raw + 2 = Smi(n+1)
                    self.mem.emit_add_r64_imm32(0, 2);             // rax = new
                    self.mem.emit_mov_mem_disp32_r64(13, disp, 0); // locals[idx] = new
                    // Push result
                    if is_prefix {
                        self.emit_jit_stack_push();                // push new
                    } else {
                        self.mem.emit_mov_r64_rm64(0, 1);          // rax = old
                        self.emit_jit_stack_push();                // push old
                    }
                }
                Opcode::DecLocal => {
                    let idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    let disp = (idx * 8) as i32;
                    self.mem.emit_mov_r64_mem_disp32(0, 13, disp); // rax = locals[idx]
                    self.mem.emit_mov_r64_rm64(1, 0);             // rcx = old
                    // Smi decrement: old_raw - 2 = Smi(n-1)
                    self.mem.emit_sub_r64_imm32(0, 2);             // rax = new
                    self.mem.emit_mov_mem_disp32_r64(13, disp, 0); // locals[idx] = new
                    if is_prefix {
                        self.emit_jit_stack_push();
                    } else {
                        self.mem.emit_mov_r64_rm64(0, 1);
                        self.emit_jit_stack_push();
                    }
                }
                _ => {
                    self.mem.emit_byte(0xCC);
                }
            }
        }

        self.resolve_patches();
        self.mem
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_bytecode::opcode::Instruction;

    fn make_prog(instructions: Vec<Instruction>) -> BytecodeProgram {
        BytecodeProgram {
            instructions,
            string_pool: vec![],
            float_pool: vec![],
            functions: vec![],
            named_function: false,
            is_generator: false,
            local_names: vec![],
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_load_smi_return() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        // vm_ptr and gc_ptr are unused for this simple program
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        // Smi(42) = (42 << 1) | 1 = 85
        assert_eq!(result, 85u64);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_add_smi() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![10]),
            Instruction::new(Opcode::LoadSmi, vec![20]),
            Instruction::new(Opcode::Add, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        // Smi(10) + Smi(20) = Smi(30) = (30 << 1) | 1 = 61
        assert_eq!(result, 61u64);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_sub_smi() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![30]),
            Instruction::new(Opcode::LoadSmi, vec![10]),
            Instruction::new(Opcode::Sub, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        // Smi(30) - Smi(10) = Smi(20) = (20 << 1) | 1 = 41
        assert_eq!(result, 41u64);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_mul_smi() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![6]),
            Instruction::new(Opcode::LoadSmi, vec![7]),
            Instruction::new(Opcode::Mul, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        // Smi(6) * Smi(7) = Smi(42) = (42 << 1) | 1 = 85
        assert_eq!(result, 85u64);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_undefined() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadUndefined, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 0u64); // undefined = Value(0)
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_null() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadNull, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 2u64); // null = Value(2)
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_load_true() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadBoolean, vec![1]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 7u64); // true = Value(7) = Smi(3)? No: true=7
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_chained_arithmetic() {
        // (10 + 20) * 3 - 5
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![10]),
            Instruction::new(Opcode::LoadSmi, vec![20]),
            Instruction::new(Opcode::Add, vec![]),       // 30
            Instruction::new(Opcode::LoadSmi, vec![3]),
            Instruction::new(Opcode::Mul, vec![]),       // 90
            Instruction::new(Opcode::LoadSmi, vec![5]),
            Instruction::new(Opcode::Sub, vec![]),       // 85
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        // Smi(85) = (85 << 1) | 1 = 171
        assert_eq!(result, 171u64);
    }

    // -------------------------------------------------------------------
    // Control flow tests — execution (x86_64 only)
    // -------------------------------------------------------------------

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_conditional_true() {
        // if (1) { return 42; } else { return 99; }
        // LoadSmi(1) → JumpIfFalse(5) → LoadSmi(42) → Return → LoadSmi(99) → Return
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![1]),
            Instruction::new(Opcode::JumpIfFalse, vec![5]),
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::Return, vec![]),
            Instruction::new(Opcode::LoadSmi, vec![99]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 85u64); // Smi(42) = 85
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_conditional_false() {
        // if (0) { return 42; } else { return 99; }
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![0]),
            Instruction::new(Opcode::JumpIfFalse, vec![4]),
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::Return, vec![]),
            Instruction::new(Opcode::LoadSmi, vec![99]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 199u64); // Smi(99) = 199
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_conditional_undefined_falsy() {
        // if (undefined) { return 42; } else { return 99; }
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadUndefined, vec![]),
            Instruction::new(Opcode::JumpIfFalse, vec![4]),
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::Return, vec![]),
            Instruction::new(Opcode::LoadSmi, vec![99]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 199u64); // Smi(99) = 199
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_jump() {
        // Unconditional jump over a block: Jump(3), LoadSmi(42), Return, LoadSmi(99), Return
        let prog = make_prog(vec![
            Instruction::new(Opcode::Jump, vec![3]),
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::Return, vec![]),
            Instruction::new(Opcode::LoadSmi, vec![99]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 199u64); // Smi(99) = 199
    }

    // -------------------------------------------------------------------
    // Non-execution tests (verify emit offset / byte count)
    // -------------------------------------------------------------------

    #[test]
    fn test_compile_empty_then_return() {
        // A program with just Return: should emit prologue, pop (which underflows
        // but the bytes are still valid), and epilogue. Verify it doesn't panic.
        let prog = make_prog(vec![
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        // We can't easily verify the exact offset without duplicating the
        // codegen logic, but we can verify that it emitted something.
        assert!(mem.offset > 0);
    }

    #[test]
    fn test_compile_load_smi_offset() {
        // Known byte count for LoadSmi + Return:
        //   prologue: push rbp(1)+push r15(2)+push r14(2)+push r13(2)+push rbx(1)+
        //             mov r15,rdi(3)+mov r14,rsi(3)+mov r13,rdx(3)+sub rsp,2048(7)+mov rbx,rsp(3) = 27
        //   LoadSmi: mov rax,85(10)+push(3+4=7) = 17
        //   Return:  pop(3+4=7)+epilogue(7+1+1+2+2+2+1=16)+ret(1) = 24
        //   total approx: 27 + 17 + 24 = 68
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        // Verify it emitted a reasonable number of bytes (within 55-85)
        assert!(mem.offset >= 55, "offset was {}", mem.offset);
        assert!(mem.offset <= 85, "offset was {}", mem.offset);
    }

    // -------------------------------------------------------------------
    // Local variable + comparison tests — execution (x86_64 only)
    // -------------------------------------------------------------------

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_load_local() {
        // locals[0] = Smi(42); return locals[0];
        // StoreLocal(0), Pop, LoadLocal(0), Return
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::StoreLocal, vec![0]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::LoadLocal, vec![0]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        // Provide a local slot via a stack-allocated array
        let mut locals: [u64; 1] = [0; 1];
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), locals.as_mut_ptr()) };
        // Smi(42) = 85
        assert_eq!(result, 85u64);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_store_local_roundtrip() {
        // locals[0] = Smi(10); locals[0] += Smi(20); return locals[0];
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![10]),
            Instruction::new(Opcode::StoreLocal, vec![0]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::LoadLocal, vec![0]),
            Instruction::new(Opcode::LoadSmi, vec![20]),
            Instruction::new(Opcode::Add, vec![]),
            Instruction::new(Opcode::StoreLocal, vec![0]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::LoadLocal, vec![0]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let mut locals: [u64; 1] = [0; 1];
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), locals.as_mut_ptr()) };
        // Smi(30) = 61
        assert_eq!(result, 61u64);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_lt() {
        // 3 < 5 → Smi(1)=3 (true)
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![3]),
            Instruction::new(Opcode::LoadSmi, vec![5]),
            Instruction::new(Opcode::Lt, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        // Smi(1) = 3
        assert_eq!(result, 3u64);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_lt_false() {
        // 5 < 3 → Smi(0)=1 (false)
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![5]),
            Instruction::new(Opcode::LoadSmi, vec![3]),
            Instruction::new(Opcode::Lt, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        // Smi(0) = 1
        assert_eq!(result, 1u64);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_lt_negative() {
        // -3 < 5 → Smi(1)=3 (true)
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![-3]),
            Instruction::new(Opcode::LoadSmi, vec![5]),
            Instruction::new(Opcode::Lt, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
        // Smi(1) = 3
        assert_eq!(result, 3u64);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_inc_local_postfix() {
        // locals[0] = Smi(5); locals[0]++; return locals[0];
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![5]),
            Instruction::new(Opcode::StoreLocal, vec![0]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::IncLocal, vec![0, 0]), // postfix: pushes old (5), stores 6
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::LoadLocal, vec![0]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let mut locals: [u64; 1] = [0; 1];
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), locals.as_mut_ptr()) };
        // Smi(6) = 13
        assert_eq!(result, 13u64);
        // Verify local was incremented
        assert_eq!(locals[0], 13); // Smi(6)
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_dec_local_prefix() {
        // locals[0] = Smi(5); --locals[0]; return locals[0];
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![5]),
            Instruction::new(Opcode::StoreLocal, vec![0]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::DecLocal, vec![0, 1]), // prefix: pushes new (4), stores 4
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::LoadLocal, vec![0]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let mut locals: [u64; 1] = [0; 1];
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), locals.as_mut_ptr()) };
        // Smi(4) = 9
        assert_eq!(result, 9u64);
        assert_eq!(locals[0], 9); // Smi(4)
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_loop() {
        // var i = 0; var sum = 0;
        // while (i < 5) { sum = sum + i; i++; }
        // return sum;
        //
        // Bytecode layout (indices):
        //   0: LoadSmi(0)        // sum = 0
        //   1: StoreLocal(0)
        //   2: Pop
        //   3: LoadSmi(0)        // i = 0
        //   4: StoreLocal(1)
        //   5: Pop
        //   6: LoadLocal(1)      // loop: load i
        //   7: LoadSmi(5)
        //   8: Lt                // i < 5
        //   9: JumpIfFalse(19)   // exit
        //  10: LoadLocal(0)      // sum
        //  11: LoadLocal(1)      // i
        //  12: Add               // sum + i
        //  13: StoreLocal(0)
        //  14: Pop
        //  15: IncLocal(1, 0)   // i++ (postfix)
        //  16: Pop
        //  17: Jump(6)          // back to loop
        //  18: LoadLocal(0)     // exit: return sum
        //  19: Return

        // Sum 0..4 = 10. Smi(10) = 21.
        let instructions = vec![
            Instruction::new(Opcode::LoadSmi, vec![0]),
            Instruction::new(Opcode::StoreLocal, vec![0]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::LoadSmi, vec![0]),
            Instruction::new(Opcode::StoreLocal, vec![1]),
            Instruction::new(Opcode::Pop, vec![]),
            // Loop header
            Instruction::new(Opcode::LoadLocal, vec![1]),
            Instruction::new(Opcode::LoadSmi, vec![5]),
            Instruction::new(Opcode::Lt, vec![]),
            Instruction::new(Opcode::JumpIfFalse, vec![19]),
            // Body
            Instruction::new(Opcode::LoadLocal, vec![0]),
            Instruction::new(Opcode::LoadLocal, vec![1]),
            Instruction::new(Opcode::Add, vec![]),
            Instruction::new(Opcode::StoreLocal, vec![0]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::IncLocal, vec![1, 0]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::Jump, vec![6]),
            // Exit
            Instruction::new(Opcode::LoadLocal, vec![0]),
            Instruction::new(Opcode::Return, vec![]),
        ];
        let prog = make_prog(instructions);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let mut locals: [u64; 2] = [0; 2];
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut(), locals.as_mut_ptr()) };
        // Smi(10) = 21
        assert_eq!(result, 21u64);
    }

    // -------------------------------------------------------------------
    // Non-execution offset tests (all architectures)
    // -------------------------------------------------------------------

    #[test]
    fn test_compile_store_local_offset() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::StoreLocal, vec![0]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        assert!(mem.offset >= 60, "offset was {}", mem.offset);
        assert!(mem.offset <= 100, "offset was {}", mem.offset);
    }

    #[test]
    fn test_compile_lt_offset() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![3]),
            Instruction::new(Opcode::LoadSmi, vec![5]),
            Instruction::new(Opcode::Lt, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        assert!(mem.offset >= 85, "offset was {}", mem.offset);
        assert!(mem.offset <= 170, "offset was {}", mem.offset);
    }

    #[test]
    fn test_compile_loop_offset() {
        let instructions = vec![
            Instruction::new(Opcode::LoadSmi, vec![0]),
            Instruction::new(Opcode::StoreLocal, vec![0]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::LoadSmi, vec![0]),
            Instruction::new(Opcode::StoreLocal, vec![1]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::LoadLocal, vec![1]),
            Instruction::new(Opcode::LoadSmi, vec![5]),
            Instruction::new(Opcode::Lt, vec![]),
            Instruction::new(Opcode::JumpIfFalse, vec![19]),
            Instruction::new(Opcode::LoadLocal, vec![0]),
            Instruction::new(Opcode::LoadLocal, vec![1]),
            Instruction::new(Opcode::Add, vec![]),
            Instruction::new(Opcode::StoreLocal, vec![0]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::IncLocal, vec![1, 0]),
            Instruction::new(Opcode::Pop, vec![]),
            Instruction::new(Opcode::Jump, vec![6]),
            Instruction::new(Opcode::LoadLocal, vec![0]),
            Instruction::new(Opcode::Return, vec![]),
        ];
        let prog = make_prog(instructions);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        assert!(mem.offset >= 140, "offset was {}", mem.offset);
        assert!(mem.offset <= 500, "offset was {}", mem.offset);
    }
}
