/// Bytecode-to-machine-code compiler (copy-and-patch).
///
/// Registers (callee-saved):
///   R15 — VM pointer (from RDI)
///   R14 — GC SemiSpace pointer (from RSI)
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
/// # Safety
///
/// Callers must pass valid pointers and ensure the code is executable.
pub type JitEntryFn = unsafe fn(vm_ptr: *mut u8, gc_ptr: *mut u8) -> u64;

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
        self.mem.emit_push_r64(3);   // push rbx (JIT stack ptr)
        self.mem.emit_mov_r64_rm64(15, 7); // r15 = rdi (VM ptr)
        self.mem.emit_mov_r64_rm64(14, 6); // r14 = rsi (GC ptr)
        self.mem.emit_sub_r64_imm32(4, JIT_STACK_SIZE); // sub rsp, 2048
        self.mem.emit_mov_r64_rm64(3, 4);  // rbx = rsp
    }

    fn emit_epilogue(&mut self) {
        self.mem.emit_add_r64_imm32(4, JIT_STACK_SIZE); // add rsp, 2048
        self.mem.emit_pop_r64(3);   // pop rbx
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
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
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
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
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
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
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
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
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
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
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
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
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
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
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
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
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
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 85u64); // Smi(42) = 85
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_conditional_false() {
        // if (0) { return 42; } else { return 99; }
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![0]),
            Instruction::new(Opcode::JumpIfFalse, vec![5]),
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::Return, vec![]),
            Instruction::new(Opcode::LoadSmi, vec![99]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 199u64); // Smi(99) = 199
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_conditional_undefined_falsy() {
        // if (undefined) { return 42; } else { return 99; }
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadUndefined, vec![]),
            Instruction::new(Opcode::JumpIfFalse, vec![5]),
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::Return, vec![]),
            Instruction::new(Opcode::LoadSmi, vec![99]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
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
        let result = unsafe { func(std::ptr::null_mut(), std::ptr::null_mut()) };
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
        //   prologue: push rbp(1)+push r15(2)+push r14(2)+push rbx(1)+
        //             mov r15,rdi(3)+mov r14,rsi(3)+sub rsp,2048(7)+mov rbx,rsp(3) = 22
        //   LoadSmi: mov rax,85(10)+push(3+4=7) = 17
        //   Return:  pop(3+4=7)+epilogue(7+1+1+2+2+1=14)+ret(1) = 22
        //   total approx: 22 + 17 + 22 = 61
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let mem = CodeGen::new(prog.instructions.len()).compile(&prog);
        // Verify it emitted a reasonable number of bytes (within 50-75)
        assert!(mem.offset >= 50, "offset was {}", mem.offset);
        assert!(mem.offset <= 75, "offset was {}", mem.offset);
    }
}
