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
use crate::{BailoutPoint, BailoutReason, BailoutTable, CompiledFunction};
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
    bailout_table: Vec<BailoutPoint>,
    stack_depth: u32,
}

impl CodeGen {
    pub fn new(instruction_count: usize) -> Self {
        let mem = ExecutableMemory::allocate(64 * 1024);
        CodeGen {
            mem,
            bc_to_native: vec![0; instruction_count],
            pending_patches: Vec::new(),
            bailout_table: Vec::new(),
            stack_depth: 0,
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

    /// Call the lexical helper function stored in Vm::jit_helpers (offset 512 from vm_ptr).
    /// Sets up System V AMD64 calling convention: rdi=vm_ptr, rsi=op, rdx=arg1, rcx=arg2.
    /// The return value is in rax. Clobbers rdi, rsi, rdx, rcx, rax.
    fn emit_lexical_call(&mut self, op: u64, arg1: u64, arg2: u64) {
        // rdi = r15 (vm_ptr)
        self.mem.emit_mov_r64_rm64(7, 15);
        // rsi = op
        self.mem.emit_mov_r64_imm64(6, op);
        // rdx = arg1
        self.mem.emit_mov_r64_imm64(2, arg1);
        // rcx = arg2
        self.mem.emit_mov_r64_imm64(1, arg2);
        // Load helper addr from [r15 + 512] into rax
        self.mem.emit_rex_w();
        self.mem.emit_byte(0x8B);            // MOV r64, r/m64
        self.mem.emit_byte(0x87);            // mod=10, reg=0(rax), r/m=7(r15)
        self.mem.emit_u32(512);              // disp32 = offset of jit_helpers.lexical_helper
        self.mem.emit_call_r64(0);           // call rax
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
        self.stack_depth += 1;
    }

    fn emit_jit_stack_pop(&mut self) {
        // sub rbx, 8
        self.mem.emit_sub_r64_imm32(3, 8);
        // mov rax, [rbx]  → REX.W 8B 03  (mod=00, reg=0(rax), r/m=3(rbx))
        self.mem.emit_rex_w();
        self.mem.emit_byte(0x8B);
        self.mem.emit_byte(0x03);
        self.stack_depth = self.stack_depth.saturating_sub(1);
    }

    fn record_bailout_point(&mut self, bc_pc: usize, reason: BailoutReason) {
        self.bailout_table.push(BailoutPoint {
            bc_pc,
            stack_depth: self.stack_depth,
            reason,
        });
    }

    // -----------------------------------------------------------------------
    // Prologue / epilogue
    // -----------------------------------------------------------------------

    fn emit_prologue(&mut self) {
        self.mem.emit_push_r64(5); // push rbp
        self.mem.emit_push_r64(15); // push r15 (VM ptr)
        self.mem.emit_push_r64(14); // push r14 (GC ptr)
        self.mem.emit_push_r64(13); // push r13 (locals ptr)
        self.mem.emit_push_r64(3); // push rbx (JIT stack ptr)
        self.mem.emit_mov_r64_rm64(15, 7); // r15 = rdi (VM ptr)
        self.mem.emit_mov_r64_rm64(14, 6); // r14 = rsi (GC ptr)
        self.mem.emit_mov_r64_rm64(13, 2); // r13 = rdx (locals ptr)
        self.mem.emit_sub_r64_imm32(4, JIT_STACK_SIZE); // sub rsp, 2048
        self.mem.emit_mov_r64_rm64(3, 4); // rbx = rsp
        // Store initial JIT stack pointer as jit_stack_base (offset 576 from vm_ptr).
        // jit_stack[64] (512) + jit_helpers[8] (64) = 576
        self.mem.emit_rex_w();
        self.mem.emit_byte(0x89);            // MOV r/m64, r64
        self.mem.emit_byte(0x9F);            // mod=10, reg=3(rbx), r/m=7(r15)
        self.mem.emit_u32(576);              // disp32 = offset of jit_stack_base
    }

    fn emit_epilogue(&mut self) {
        self.mem.emit_add_r64_imm32(4, JIT_STACK_SIZE); // add rsp, 2048
        self.mem.emit_pop_r64(3); // pop rbx
        self.mem.emit_pop_r64(13); // pop r13
        self.mem.emit_pop_r64(14); // pop r14
        self.mem.emit_pop_r64(15); // pop r15
        self.mem.emit_pop_r64(5); // pop rbp
        self.mem.emit_ret();
    }

    // -----------------------------------------------------------------------
    // Smi arithmetic helpers (overflow-guarded)
    // -----------------------------------------------------------------------

    const MAX_I31: i32 = 0x3FFFFFFF;   // 2^30 − 1
    const MIN_I31_AS_I32: i32 = -1_073_741_824; // −2^30, sign-extends to 0xFFFF_FFFF_C000_0000

    /// Emit the overflow check / bailout sequence for a Smi-encoded value in rax.
    /// On overflow: pushes r9 then r8 back, records bailout, calls helper, epilogue.
    /// On no overflow: falls through, rax holds the valid Smi result.
    fn emit_smi_overflow_bailout_or_continue(
        &mut self,
        bc_idx: usize,
        r9_holds_a: bool,
        r8_holds_b: bool,
    ) {
        // Save result in rcx and untag for comparison
        self.mem.emit_mov_r64_rm64(1, 0);          // rcx = rax (result)
        self.mem.emit_sar_r64_1(1);                // rcx >>= 1 (untag)
        // Check > max
        self.mem.emit_cmp_r64_imm32(1, Self::MAX_I31);
        let jg_ov = self.mem.emit_jg_rel32(0);
        // Check < min
        self.mem.emit_cmp_r64_imm32(1, Self::MIN_I31_AS_I32);
        let jl_ov = self.mem.emit_jl_rel32(0);
        // No overflow — skip bailout
        let jmp_ok = self.mem.emit_jmp_rel32(0);
        // Overflow: restore stack, record bailout, call helper, epilogue
        let ov_label = self.mem.current_offset();
        if r9_holds_a {
            self.mem.emit_mov_r64_rm64(0, 9);      // rax = a (saved in r9)
            self.emit_jit_stack_push();             // restore a
        }
        if r8_holds_b {
            self.mem.emit_mov_r64_rm64(0, 8);      // rax = b (saved in r8)
            self.emit_jit_stack_push();             // restore b
        }
        self.record_bailout_point(bc_idx, BailoutReason::Overflow);
        // Call bailout_helper(rdi=r15, rsi=bc_idx, rdx=rbx)
        self.mem.emit_mov_r64_rm64(7, 15);
        self.mem.emit_mov_r64_imm64(6, bc_idx as u64);
        self.mem.emit_mov_r64_rm64(2, 3);
        self.mem.emit_rex_w();
        self.mem.emit_byte(0x8B);
        self.mem.emit_byte(0x87);
        self.mem.emit_u32(520);
        self.mem.emit_call_r64(0);
        self.mem.emit_rex_w();
        self.mem.emit_byte(0x31);
        self.mem.emit_byte(0xC0);                  // xor eax, eax
        self.emit_jit_stack_push();
        self.emit_epilogue();
        // Patch jumps
        let ok_label = self.mem.current_offset();
        let four: u32 = 4;
        self.mem.patch_u32(jg_ov, ov_label as u32 - (jg_ov as u32 + four));
        self.mem.patch_u32(jl_ov, ov_label as u32 - (jl_ov as u32 + four));
        self.mem.patch_u32(jmp_ok, ok_label as u32 - (jmp_ok as u32 + four));
    }

    /// Check if rax holds a Smi (bit 0 = 1). If yes, fall through.
    /// If not, restore JIT stack from saved registers, record NonSmiInput
    /// bailout, call bailout_helper, and return.
    /// `saved`: register indices of previously-popped values in chronological
    /// order (earliest first). Restored on bail after current rax.
    fn emit_smi_check(&mut self, bc_idx: usize, saved: &[u8]) {
        // TEST rax, 1
        self.mem.emit_rex_w();
        self.mem.emit_byte(0xF7);
        self.mem.emit_byte(0xC0);
        self.mem.emit_u32(1);
        let je_bail = self.mem.emit_je_rel32(0); // ZF=1 → bit 0=0 → not Smi → bail
        let jmp_ok = self.mem.emit_jmp_rel32(0);
        let bail_label = self.mem.current_offset();
        // Restore JIT stack: push current rax, then saved values
        self.emit_jit_stack_push(); // push rax (current failed check)
        for &reg in saved.iter() {
            self.mem.emit_mov_r64_rm64(0, reg);
            self.emit_jit_stack_push();
        }
        self.record_bailout_point(bc_idx, BailoutReason::NonSmiInput);
        // Call bailout_helper(rdi=r15, rsi=bc_pc, rdx=rbx)
        self.mem.emit_mov_r64_rm64(7, 15);
        self.mem.emit_mov_r64_imm64(6, bc_idx as u64);
        self.mem.emit_mov_r64_rm64(2, 3);
        self.mem.emit_rex_w();
        self.mem.emit_byte(0x8B);
        self.mem.emit_byte(0x87);
        self.mem.emit_u32(520);
        self.mem.emit_call_r64(0);
        self.mem.emit_rex_w();
        self.mem.emit_byte(0x31);
        self.mem.emit_byte(0xC0);
        self.emit_jit_stack_push();
        self.emit_epilogue();
        let ok_label = self.mem.current_offset();
        let four: u32 = 4;
        self.mem.patch_u32(je_bail, bail_label as u32 - (je_bail as u32 + four));
        self.mem.patch_u32(jmp_ok, ok_label as u32 - (jmp_ok as u32 + four));
    }

    /// Pop two Smis from the JIT stack and add them:
    ///   (a & ~1) + b
    fn emit_smi_add(&mut self, bc_idx: usize) {
        self.emit_jit_stack_pop();                 // rax = b
        self.emit_smi_check(bc_idx, &[]);          // check b
        self.mem.emit_mov_r64_rm64(1, 0);          // rcx = b
        self.emit_jit_stack_pop();                 // rax = a
        self.mem.emit_mov_r64_rm64(9, 0);          // r9 = a
        self.emit_smi_check(bc_idx, &[1]);         // check a; saved=[rcx(b)]
        self.mem.emit_mov_r64_rm64(8, 1);          // r8 = b
        self.mem.emit_and_r64_imm8(0, -2);         // rax = a & ~1
        self.mem.emit_add_r64_r64(0, 1);           // rax += rcx
        self.emit_smi_overflow_bailout_or_continue(bc_idx, true, true);
    }

    /// Pop two Smis and subtract (a - b):
    ///   (a - b) | 1
    fn emit_smi_sub(&mut self, bc_idx: usize) {
        self.emit_jit_stack_pop();                 // rax = b
        self.emit_smi_check(bc_idx, &[]);          // check b
        self.mem.emit_mov_r64_rm64(1, 0);          // rcx = b
        self.emit_jit_stack_pop();                 // rax = a
        self.mem.emit_mov_r64_rm64(9, 0);          // r9 = a
        self.emit_smi_check(bc_idx, &[1]);         // check a; saved=[rcx(b)]
        self.mem.emit_mov_r64_rm64(8, 1);          // r8 = b
        self.mem.emit_sub_r64_r64(0, 1);           // rax -= rcx
        self.mem.emit_or_r64_imm8(0, 1);           // rax |= 1
        self.emit_smi_overflow_bailout_or_continue(bc_idx, true, true);
    }

    /// Pop two Smis and multiply (a * b):
    ///   decode → mul → encode
    fn emit_smi_mul(&mut self, bc_idx: usize) {
        self.emit_jit_stack_pop();                 // rax = b
        self.emit_smi_check(bc_idx, &[]);          // check b
        self.mem.emit_mov_r64_rm64(1, 0);          // rcx = b
        self.emit_jit_stack_pop();                 // rax = a
        self.mem.emit_mov_r64_rm64(9, 0);          // r9 = a
        self.emit_smi_check(bc_idx, &[1]);         // check a; saved=[rcx(b)]
        self.mem.emit_mov_r64_rm64(8, 1);          // r8 = b
        self.mem.emit_sar_r64_1(0);                // rax >>= 1 (untag a)
        self.mem.emit_sar_r64_1(1);                // rcx >>= 1 (untag b)
        self.mem.emit_imul_r64_r64(0, 1);          // rax *= rcx
        self.mem.emit_shl_r64_1(0);                // rax <<= 1 (Smi)
        self.mem.emit_or_r64_imm8(0, 1);           // rax |= 1
        self.emit_smi_overflow_bailout_or_continue(bc_idx, true, true);
    }

    // -----------------------------------------------------------------------
    // Main compilation
    // -----------------------------------------------------------------------

    /// Compile a `BytecodeProgram` into native code.
    ///
    /// On completion the returned `CompiledFunction` contains writable memory
    /// — the caller must call `make_executable()` on `.mem` before execution.
    /// The `.bailout_table` maps bc PCs to stack depths and reasons.
    pub fn compile(mut self, program: &BytecodeProgram) -> CompiledFunction {
        self.emit_prologue();

        for (bc_idx, instr) in program.instructions.iter().enumerate() {
            self.bc_to_native[bc_idx] = self.mem.current_offset();

            match instr.opcode {
                Opcode::LoadSmi => {
                    let smi_raw = (instr.operands[0] << 1) | 1;
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
                    // Value::boolean(true) = 0x06, Value::boolean(false) = 0x04
                    let val = if instr.operands[0] != 0 { 6u64 } else { 4u64 };
                    self.mem.emit_mov_r64_imm64(0, val);
                    self.emit_jit_stack_push();
                }
                Opcode::LoadFloat64 => {
                    let idx = instr.operands[0] as usize;
                    let val = program.float_pool.get(idx).copied().unwrap_or(0.0);
                    let i = val as i64;
                    let smi_raw = ((i as u64) << 1) | 1;
                    self.mem.emit_mov_r64_imm64(0, smi_raw);
                    self.emit_jit_stack_push();
                }
                Opcode::LoadStringConst => {
                    let string_idx = instr.operands[0] as u64;
                    let prog_ptr = program as *const BytecodeProgram as *const u8 as u64;
                    // rdi = r15 (vm_ptr)
                    self.mem.emit_mov_r64_rm64(7, 15);
                    // rsi = r14 (gc_ptr)
                    self.mem.emit_mov_r64_rm64(6, 14);
                    // rdx = prog_ptr (immediate)
                    self.mem.emit_mov_r64_imm64(2, prog_ptr);
                    // rcx = string_idx (immediate)
                    self.mem.emit_mov_r64_imm64(1, string_idx);
                    // Load string_helper from [r15 + 536] into rax
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);                   // MOV rax, [r15 + 536]
                    self.mem.emit_byte(0x87);                   // mod=10, reg=0(rax), r/m=7(r15)
                    self.mem.emit_u32(536);                     // disp32
                    self.mem.emit_call_r64(0);                  // call rax
                    // push result (string Value in rax)
                    self.emit_jit_stack_push();
                }
                Opcode::Return => {
                    self.emit_jit_stack_pop();
                    self.emit_epilogue();
                }
                Opcode::Neg => {
                    // Smi(-n) = -(2n+1) + 2
                    self.emit_jit_stack_pop(); // rax = value
                    self.mem.emit_mov_r64_rm64(9, 0); // r9 = value (save)
                    self.emit_smi_check(bc_idx, &[]); // check input Smi
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0xF7);
                    self.mem.emit_byte(0xD8); // neg rax
                    self.mem.emit_add_r64_imm32(0, 2); // add rax, 2
                    self.emit_smi_overflow_bailout_or_continue(bc_idx, true, false);
                    self.emit_jit_stack_push();
                }
                Opcode::Not => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]);    // check input Smi
                    self.mem.emit_mov_r64_rm64(1, 0);    // rcx = value
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x83);
                    self.mem.emit_byte(0xF9);            // cmp rcx, imm8
                    self.mem.emit_byte(2);               // imm8 = 2
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0x96);
                    self.mem.emit_byte(0xC0);            // setbe al
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x83);
                    self.mem.emit_byte(0xF9);            // cmp rcx, imm8
                    self.mem.emit_byte(4);               // imm8 = 4
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0x94);
                    self.mem.emit_byte(0xC1);            // sete cl
                    self.mem.emit_byte(0x08);
                    self.mem.emit_byte(0xC8);            // or al, cl
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0xB6);
                    self.mem.emit_byte(0xC0);            // movzx eax, al
                    self.mem.emit_shl_r64_1(0);
                    self.mem.emit_or_r64_imm8(0, 1);
                    self.emit_jit_stack_push();
                }
                Opcode::Void => {
                    self.emit_jit_stack_pop();
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x31);
                    self.mem.emit_byte(0xC0);            // xor eax, eax (undefined = 0)
                    self.emit_jit_stack_push();
                }
                Opcode::UnaryPlus => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]); // check Smi
                    self.emit_jit_stack_push();       // push back (no transformation)
                }
                Opcode::BitNot => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]); // check input Smi
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0xF7);
                    self.mem.emit_byte(0xD0);            // not rax
                    self.mem.emit_add_r64_imm32(0, 1);   // add rax, 1
                    self.emit_jit_stack_push();
                }
                Opcode::StrictNe => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]);    // check b
                    self.mem.emit_mov_r64_rm64(1, 0);    // rcx = b
                    self.emit_jit_stack_pop();            // rax = a
                    self.emit_smi_check(bc_idx, &[1]);   // check a; saved=[rcx(b)]
                    self.mem.emit_cmp_r64_r64(0, 1);     // cmp a, b
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0x95);
                    self.mem.emit_byte(0xC0);            // setne al
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0xB6);
                    self.mem.emit_byte(0xC0);            // movzx eax, al
                    self.mem.emit_byte(0xD1);
                    self.mem.emit_byte(0xE0);            // shl eax, 1
                    self.mem.emit_or_r64_imm8(0, 1);    // or rax, 1
                    self.emit_jit_stack_push();
                }
                Opcode::Swap => {
                    self.emit_jit_stack_pop();            // rax = b
                    self.mem.emit_mov_r64_rm64(1, 0);    // rcx = b
                    self.emit_jit_stack_pop();            // rax = a
                    self.mem.emit_mov_r64_rm64(2, 0);    // rdx = a
                    self.mem.emit_mov_r64_rm64(0, 1);    // rax = b
                    self.emit_jit_stack_push();
                    self.mem.emit_mov_r64_rm64(0, 2);    // rax = a
                    self.emit_jit_stack_push();
                }
                Opcode::Eq => {
                    self.emit_jit_stack_pop();            // rax = b
                    self.emit_smi_check(bc_idx, &[]);    // check b
                    self.mem.emit_mov_r64_rm64(1, 0);    // rcx = b
                    self.emit_jit_stack_pop();            // rax = a
                    self.emit_smi_check(bc_idx, &[1]);   // check a; saved=[rcx(b)]
                    // a == b
                    self.mem.emit_cmp_r64_r64(0, 1);
                    let je_same = self.mem.emit_je_rel32(0);
                    // null == undefined: (a==0 && b==2) || (a==2 && b==0)
                    self.mem.emit_mov_r64_imm64(2, 0);   // rdx = 0
                    self.mem.emit_cmp_r64_r64(0, 2);
                    let je_a0 = self.mem.emit_je_rel32(0);
                    self.mem.emit_mov_r64_imm64(2, 2);   // rdx = 2 (null)
                    self.mem.emit_cmp_r64_r64(0, 2);
                    let je_a2 = self.mem.emit_je_rel32(0);
                    // boolean checks
                    self.mem.emit_mov_r64_imm64(2, 4);   // false
                    self.mem.emit_cmp_r64_r64(0, 2);
                    let je_a_false = self.mem.emit_je_rel32(0);
                    self.mem.emit_mov_r64_imm64(2, 6);   // true
                    self.mem.emit_cmp_r64_r64(0, 2);
                    let je_a_true = self.mem.emit_je_rel32(0);
                    let jmp_done = self.mem.emit_jmp_rel32(0); // fall through false
                    // a == null(2): check b == undefined(0)
                    let label_a2 = self.mem.current_offset();
                    self.mem.patch_u32(je_a2, (label_a2 - (je_a2 + 6)) as u32);
                    self.mem.emit_mov_r64_imm64(2, 0);
                    self.mem.emit_cmp_r64_r64(1, 2);
                    let je_null_undef = self.mem.emit_je_rel32(0);
                    let jmp_not_eq = self.mem.emit_jmp_rel32(0);
                    // a == undefined(0): check b == null(2)
                    let label_a0 = self.mem.current_offset();
                    self.mem.patch_u32(je_a0, (label_a0 - (je_a0 + 6)) as u32);
                    self.mem.emit_mov_r64_imm64(2, 2);
                    self.mem.emit_cmp_r64_r64(1, 2);
                    let je_undef_null = self.mem.emit_je_rel32(0);
                    let jmp_not_eq2 = self.mem.emit_jmp_rel32(0);
                    // a == false: check b == Smi(0)=1
                    let label_a_false = self.mem.current_offset();
                    self.mem.patch_u32(je_a_false, (label_a_false - (je_a_false + 6)) as u32);
                    self.mem.emit_mov_r64_imm64(2, 1);
                    self.mem.emit_cmp_r64_r64(1, 2);
                    let je_eq = self.mem.emit_je_rel32(0);
                    let jmp_not_eq3 = self.mem.emit_jmp_rel32(0);
                    // a == true: check b == Smi(1)=3
                    let label_a_true = self.mem.current_offset();
                    self.mem.patch_u32(je_a_true, (label_a_true - (je_a_true + 6)) as u32);
                    self.mem.emit_mov_r64_imm64(2, 3);
                    self.mem.emit_cmp_r64_r64(1, 2);
                    let je_eq2 = self.mem.emit_je_rel32(0);
                    let jmp_not_eq4 = self.mem.emit_jmp_rel32(0);
                    // equal
                    let label_eq = self.mem.current_offset();
                    self.mem.patch_u32(je_same, (label_eq - (je_same + 6)) as u32);
                    self.mem.patch_u32(je_null_undef, (label_eq - (je_null_undef + 6)) as u32);
                    self.mem.patch_u32(je_undef_null, (label_eq - (je_undef_null + 6)) as u32);
                    self.mem.patch_u32(je_eq, (label_eq - (je_eq + 6)) as u32);
                    self.mem.patch_u32(je_eq2, (label_eq - (je_eq2 + 6)) as u32);
                    self.mem.emit_mov_r64_imm64(0, 3);   // Smi(1)
                    let jmp_done2 = self.mem.emit_jmp_rel32(0);
                    // not equal
                    let label_not_eq = self.mem.current_offset();
                    self.mem.patch_u32(jmp_not_eq, (label_not_eq - (jmp_not_eq + 5)) as u32);
                    self.mem.patch_u32(jmp_not_eq2, (label_not_eq - (jmp_not_eq2 + 5)) as u32);
                    self.mem.patch_u32(jmp_not_eq3, (label_not_eq - (jmp_not_eq3 + 5)) as u32);
                    self.mem.patch_u32(jmp_not_eq4, (label_not_eq - (jmp_not_eq4 + 5)) as u32);
                    self.mem.emit_mov_r64_imm64(0, 1);   // Smi(0)
                    // done
                    self.mem.patch_u32(jmp_done, (label_not_eq - (jmp_done + 5)) as u32);
                    self.mem.patch_u32(jmp_done2, (self.mem.current_offset() - (jmp_done2 + 5)) as u32);
                    self.emit_jit_stack_push();
                }
                Opcode::Ne => {
                    // Same branching as Eq but XOR result
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]);
                    self.mem.emit_mov_r64_rm64(1, 0);
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[1]);
                    self.mem.emit_cmp_r64_r64(0, 1);
                    let je_same = self.mem.emit_je_rel32(0);
                    self.mem.emit_mov_r64_imm64(2, 0);
                    self.mem.emit_cmp_r64_r64(0, 2);
                    let je_a0 = self.mem.emit_je_rel32(0);
                    self.mem.emit_mov_r64_imm64(2, 2);
                    self.mem.emit_cmp_r64_r64(0, 2);
                    let je_a2 = self.mem.emit_je_rel32(0);
                    self.mem.emit_mov_r64_imm64(2, 4);
                    self.mem.emit_cmp_r64_r64(0, 2);
                    let je_a_false = self.mem.emit_je_rel32(0);
                    self.mem.emit_mov_r64_imm64(2, 6);
                    self.mem.emit_cmp_r64_r64(0, 2);
                    let je_a_true = self.mem.emit_je_rel32(0);
                    let jmp_done = self.mem.emit_jmp_rel32(0);
                    let label_a2 = self.mem.current_offset();
                    self.mem.patch_u32(je_a2, (label_a2 - (je_a2 + 6)) as u32);
                    self.mem.emit_mov_r64_imm64(2, 0);
                    self.mem.emit_cmp_r64_r64(1, 2);
                    let je_null_undef = self.mem.emit_je_rel32(0);
                    let jmp_not_eq = self.mem.emit_jmp_rel32(0);
                    let label_a0 = self.mem.current_offset();
                    self.mem.patch_u32(je_a0, (label_a0 - (je_a0 + 6)) as u32);
                    self.mem.emit_mov_r64_imm64(2, 2);
                    self.mem.emit_cmp_r64_r64(1, 2);
                    let je_undef_null = self.mem.emit_je_rel32(0);
                    let jmp_not_eq2 = self.mem.emit_jmp_rel32(0);
                    let label_a_false = self.mem.current_offset();
                    self.mem.patch_u32(je_a_false, (label_a_false - (je_a_false + 6)) as u32);
                    self.mem.emit_mov_r64_imm64(2, 1);
                    self.mem.emit_cmp_r64_r64(1, 2);
                    let je_eq = self.mem.emit_je_rel32(0);
                    let jmp_not_eq3 = self.mem.emit_jmp_rel32(0);
                    let label_a_true = self.mem.current_offset();
                    self.mem.patch_u32(je_a_true, (label_a_true - (je_a_true + 6)) as u32);
                    self.mem.emit_mov_r64_imm64(2, 3);
                    self.mem.emit_cmp_r64_r64(1, 2);
                    let je_eq2 = self.mem.emit_je_rel32(0);
                    let jmp_not_eq4 = self.mem.emit_jmp_rel32(0);
                    let label_eq = self.mem.current_offset();
                    self.mem.patch_u32(je_same, (label_eq - (je_same + 6)) as u32);
                    self.mem.patch_u32(je_null_undef, (label_eq - (je_null_undef + 6)) as u32);
                    self.mem.patch_u32(je_undef_null, (label_eq - (je_undef_null + 6)) as u32);
                    self.mem.patch_u32(je_eq, (label_eq - (je_eq + 6)) as u32);
                    self.mem.patch_u32(je_eq2, (label_eq - (je_eq2 + 6)) as u32);
                    self.mem.emit_mov_r64_imm64(0, 1);   // Smi(0) — NE (inverted from Eq)
                    let jmp_done2 = self.mem.emit_jmp_rel32(0);
                    let label_not_eq = self.mem.current_offset();
                    self.mem.patch_u32(jmp_not_eq, (label_not_eq - (jmp_not_eq + 5)) as u32);
                    self.mem.patch_u32(jmp_not_eq2, (label_not_eq - (jmp_not_eq2 + 5)) as u32);
                    self.mem.patch_u32(jmp_not_eq3, (label_not_eq - (jmp_not_eq3 + 5)) as u32);
                    self.mem.patch_u32(jmp_not_eq4, (label_not_eq - (jmp_not_eq4 + 5)) as u32);
                    self.mem.emit_mov_r64_imm64(0, 3);   // Smi(1) — NE (inverted)
                    self.mem.patch_u32(jmp_done, (label_not_eq - (jmp_done + 5)) as u32);
                    self.mem.patch_u32(jmp_done2, (self.mem.current_offset() - (jmp_done2 + 5)) as u32);
                    self.emit_jit_stack_push();
                }
                Opcode::Add => {
                    self.emit_smi_add(bc_idx);
                    self.emit_jit_stack_push();
                }
                Opcode::Sub => {
                    self.emit_smi_sub(bc_idx);
                    self.emit_jit_stack_push();
                }
                Opcode::Mul => {
                    self.emit_smi_mul(bc_idx);
                    self.emit_jit_stack_push();
                }
                Opcode::Jump => {
                    let target = instr.operands[0] as usize;
                    let patch = self.mem.emit_jmp_rel32(0);
                    self.pending_patches.push((patch, target));
                }
                Opcode::JumpIfFalse => {
                    let target = instr.operands[0] as usize;
                    self.emit_jit_stack_pop(); // rax = condition
                    self.emit_smi_check(bc_idx, &[]); // check Smi
                    self.mem.emit_mov_r64_imm64(1, 2); // rcx = 2 (null sentinel)
                    self.mem.emit_cmp_r64_r64(0, 1); // cmp rax, 2
                    let patch1 = self.mem.emit_jbe_rel32(0); // ≤2: falsy
                    self.pending_patches.push((patch1, target));
                    self.mem.emit_mov_r64_imm64(1, 4); // rcx = 4 (false sentinel)
                    self.mem.emit_cmp_r64_r64(0, 1); // cmp rax, 4
                    let patch2 = self.mem.emit_je_rel32(0); // =4: falsy
                    self.pending_patches.push((patch2, target));
                }
                Opcode::JumpIfTrue => {
                    let target = instr.operands[0] as usize;
                    self.emit_jit_stack_pop();          // rax = value
                    self.emit_smi_check(bc_idx, &[]);  // check Smi
                    self.mem.emit_mov_r64_imm64(1, 2);  // rcx = 2
                    self.mem.emit_cmp_r64_r64(0, 1);    // cmp value, 2
                    let skip_falsy = self.mem.emit_ja_rel32(0);
                    let jmp_done = self.mem.emit_jmp_rel32(0);
                    let ja_end = skip_falsy + 6;
                    let after_jmp_falsy = ja_end + 5;
                    self.mem.patch_u32(skip_falsy, (after_jmp_falsy - ja_end) as u32);
                    self.mem.emit_mov_r64_imm64(1, 4);
                    self.mem.emit_cmp_r64_r64(0, 1);
                    let je_done = self.mem.emit_je_rel32(0);
                    let jmp_target = self.mem.emit_jmp_rel32(0);
                    self.pending_patches.push((jmp_target, target));
                    let done = self.mem.current_offset();
                    self.mem.patch_u32(jmp_done, (done - (jmp_done + 5)) as u32);
                    self.mem.patch_u32(je_done, (done - (je_done + 6)) as u32);
                }
                Opcode::StorePropertyIC => {
                    let shape_id = instr.operands[0] as u64;
                    let offset = instr.operands[1] as u32;
                    let _proto_depth = instr.operands.get(2).copied().unwrap_or(0) as u32;
                    self.emit_jit_stack_pop(); // rax = value
                    self.mem.emit_mov_r64_rm64(1, 0);    // rcx = value
                    self.emit_jit_stack_pop(); // rax = object (skip key)
                    self.mem.emit_mov_r64_rm64(2, 0);    // rdx = object
                    // Test bit 0: Smi → miss
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0xF7);
                    self.mem.emit_byte(0xC2);
                    self.mem.emit_u32(0x0000_0001);      // TEST rdx, 1
                    let jne_miss_patch = self.mem.emit_jne_rel32(0);
                    // CMP rdx, 6; JBE miss
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x83);
                    self.mem.emit_byte(0xFA);
                    self.mem.emit_byte(0x06);
                    let jbe_miss_patch = self.mem.emit_jbe_rel32(0);
                    // MOV rax, [rdx + 8] (shape ptr)
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);
                    self.mem.emit_byte(0x42);
                    self.mem.emit_byte(0x08);
                    // MOV r8, [rax] (shape.id)
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);
                    self.mem.emit_byte(0x00);
                    // MOV r9, shape_id
                    self.mem.emit_mov_r64_imm64(9, shape_id);
                    // CMP r8, r9
                    self.mem.emit_cmp_r64_r64(8, 9);
                    let jne_shape_patch = self.mem.emit_jne_rel32(0);
                    // MOV [rdx + 32 + offset*8], rcx (store value)
                    let disp = 32 + offset * 8;
                    if disp <= i8::MAX as u32 {
                        self.mem.emit_rex_w();
                        self.mem.emit_byte(0x89);
                        self.mem.emit_byte(0x4A);
                        self.mem.emit_byte(disp as u8);
                    } else {
                        self.mem.emit_rex_w();
                        self.mem.emit_byte(0x89);
                        self.mem.emit_byte(0x8A);
                        self.mem.emit_u32(disp);
                    }
                    // Push value back
                    self.mem.emit_mov_r64_rm64(0, 1); // rax = rcx (value)
                    self.emit_jit_stack_push();
                    let jmp_done_patch = self.mem.emit_jmp_rel32(0);
                    // miss: push object back, then value back, bail to interpreter
                    let miss_offset = self.mem.current_offset();
                    self.record_bailout_point(bc_idx, BailoutReason::ShapeMiss);
                    self.mem.emit_mov_r64_rm64(0, 2);   // rax = rdx (object)
                    self.emit_jit_stack_push();         // restore object on JIT stack
                    self.mem.emit_mov_r64_rm64(0, 1);   // rax = rcx (value)
                    self.emit_jit_stack_push();         // restore value on JIT stack
                    // Call bailout_helper(rdi=r15, rsi=bc_idx, rdx=rbx)
                    self.mem.emit_mov_r64_rm64(7, 15);       // rdi = vm_ptr
                    self.mem.emit_mov_r64_imm64(6, bc_idx as u64); // rsi = bc_pc
                    self.mem.emit_mov_r64_rm64(2, 3);        // rdx = jit_sp
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);                // MOV rax, [r15 + 520]
                    self.mem.emit_byte(0x87);                // mod=10, reg=0(rax), r/m=7(r15)
                    self.mem.emit_u32(520);                  // disp32
                    self.mem.emit_call_r64(0);               // call rax
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x31);
                    self.mem.emit_byte(0xC0);                // xor eax, eax
                    self.emit_jit_stack_push();               // push undefined (safety)
                    self.emit_epilogue();                     // return from JIT
                    // Patch jumps: displacement = target - (patch_addr + 4)
                    // Jcc rel32: 2-byte opcode + 4-byte disp (6 total), disp field at offset+2,
                    //   end of instruction = patch_offset + 4.
                    // JMP rel32: 1-byte opcode + 4-byte disp (5 total), disp field at offset+1,
                    //   end of instruction = patch_offset + 4.
                    self.mem.patch_u32(jne_miss_patch, (miss_offset - (jne_miss_patch + 4)) as u32);
                    self.mem.patch_u32(jbe_miss_patch, (miss_offset - (jbe_miss_patch + 4)) as u32);
                    self.mem.patch_u32(jne_shape_patch, (miss_offset - (jne_shape_patch + 4)) as u32);
                    let done = self.mem.current_offset();
                    self.mem.patch_u32(jmp_done_patch, (done - (jmp_done_patch + 4)) as u32);
                }
                Opcode::LoadPropertyIC => {
                    let shape_id = instr.operands[0] as u64;
                    let offset = instr.operands[1] as u32;
                    let _proto_depth = instr.operands.get(2).copied().unwrap_or(0) as u32;
                    self.emit_jit_stack_pop(); // rax = object Value
                    // Test bit 0: if set → Smi/sentinel → miss
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0xF7);
                    self.mem.emit_byte(0xC0);  // TEST rax, imm32
                    self.mem.emit_u32(0x0000_0001);
                    let jne_miss_patch = self.mem.emit_jne_rel32(0);
                    // CMP rax, 6; JBE miss (sentinels: 0/2/4/6)
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x83);
                    self.mem.emit_byte(0xF8);
                    self.mem.emit_byte(0x06);
                    let jbe_miss_patch = self.mem.emit_jbe_rel32(0);
                    // MOV rcx, [rax + 8]  (shape ptr)
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);
                    self.mem.emit_byte(0x48);
                    self.mem.emit_byte(0x08);
                    // MOV rdx, [rcx]      (shape.id)
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);
                    self.mem.emit_byte(0x11);
                    // MOV r8, shape_id
                    self.mem.emit_mov_r64_imm64(8, shape_id);
                    // CMP rdx, r8
                    self.mem.emit_cmp_r64_r64(2, 8);
                    let jne_shape_miss = self.mem.emit_jne_rel32(0);
                    // MOV rax, [rax + 32 + offset*8]
                    let slot_disp: i32 = (32 + offset * 8) as i32;
                    if (-128..=127).contains(&(slot_disp as i8)) {
                        self.mem.emit_rex_w();
                        self.mem.emit_byte(0x8B);
                        self.mem.emit_byte(0x40); // mod=01, reg=0, r/m=0
                        self.mem.emit_byte(slot_disp as u8);
                    } else {
                        self.mem.emit_rex_w();
                        self.mem.emit_byte(0x8B);
                        self.mem.emit_byte(0x80); // mod=10, reg=0, r/m=0
                        self.mem.emit_u32(slot_disp as u32);
                    }
                    self.emit_jit_stack_push();
                    // JMP done (skip miss handler)
                    let jmp_done_patch = self.mem.emit_jmp_rel32(0);
                    // miss: push object back, bail to interpreter
                    let miss_label = self.mem.current_offset();
                    // rax still holds the originally popped value (checks don't modify it)
                    self.record_bailout_point(bc_idx, BailoutReason::ShapeMiss);
                    self.emit_jit_stack_push();        // restore JIT stack
                    // Call bailout_helper(rdi=r15, rsi=bc_idx, rdx=rbx)
                    self.mem.emit_mov_r64_rm64(7, 15);       // rdi = vm_ptr
                    self.mem.emit_mov_r64_imm64(6, bc_idx as u64); // rsi = bc_pc
                    self.mem.emit_mov_r64_rm64(2, 3);        // rdx = jit_sp
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);                // MOV rax, [r15 + 520]
                    self.mem.emit_byte(0x87);                // mod=10, reg=0(rax), r/m=7(r15)
                    self.mem.emit_u32(520);                  // disp32
                    self.mem.emit_call_r64(0);               // call rax
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x31);
                    self.mem.emit_byte(0xC0);                // xor eax, eax
                    self.emit_jit_stack_push();               // push undefined (safety)
                    self.emit_epilogue();                     // return from JIT
                    // done:
                    let done_label = self.mem.current_offset();
                    // Patch forward jumps: displacement = target - (patch_addr + 4)
                    let four: u32 = 4;
                    self.mem.patch_u32(jne_miss_patch, miss_label as u32 - (jne_miss_patch as u32 + four));
                    self.mem.patch_u32(jbe_miss_patch, miss_label as u32 - (jbe_miss_patch as u32 + four));
                    self.mem.patch_u32(jne_shape_miss, miss_label as u32 - (jne_shape_miss as u32 + four));
                    self.mem.patch_u32(jmp_done_patch, done_label as u32 - (jmp_done_patch as u32 + four));
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
                    self.emit_jit_stack_pop(); // rax = value
                    self.mem.emit_mov_mem_disp32_r64(13, disp, 0); // locals[idx] = rax
                    self.emit_jit_stack_push(); // push value back
                }
                Opcode::Pop => {
                    self.emit_jit_stack_pop();
                }
                Opcode::Lt => {
                    self.emit_jit_stack_pop(); // rax = b
                    self.emit_smi_check(bc_idx, &[]); // check b
                    self.mem.emit_mov_r64_rm64(1, 0); // rcx = b
                    self.emit_jit_stack_pop(); // rax = a
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[rcx(b)]
                    self.mem.emit_cmp_r64_r64(0, 1); // cmp a, b
                    // setl (0F 9C /0) sets al = 1 if a < b (signed), 0 otherwise
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0x9C);
                    self.mem.emit_byte(0xC0); // setl al
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0xB6);
                    self.mem.emit_byte(0xC0); // movzx eax, al
                    self.mem.emit_byte(0xD1);
                    self.mem.emit_byte(0xE0); // shl eax, 1
                    self.mem.emit_or_r64_imm8(0, 1); // or rax, 1
                    self.emit_jit_stack_push();
                }
                Opcode::Gt => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    self.mem.emit_mov_r64_rm64(1, 0);
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[rcx(b)]
                    self.mem.emit_cmp_r64_r64(0, 1);
                    // setg: 0F 9F /0
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0x9F);
                    self.mem.emit_byte(0xC0);
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0xB6);
                    self.mem.emit_byte(0xC0);
                    self.mem.emit_byte(0xD1);
                    self.mem.emit_byte(0xE0);
                    self.mem.emit_or_r64_imm8(0, 1);
                    self.emit_jit_stack_push();
                }
                Opcode::Le => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    self.mem.emit_mov_r64_rm64(1, 0);
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[rcx(b)]
                    self.mem.emit_cmp_r64_r64(0, 1);
                    // setle: 0F 9E /0
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0x9E);
                    self.mem.emit_byte(0xC0);
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0xB6);
                    self.mem.emit_byte(0xC0);
                    self.mem.emit_byte(0xD1);
                    self.mem.emit_byte(0xE0);
                    self.mem.emit_or_r64_imm8(0, 1);
                    self.emit_jit_stack_push();
                }
                Opcode::Ge => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    self.mem.emit_mov_r64_rm64(1, 0);
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[rcx(b)]
                    self.mem.emit_cmp_r64_r64(0, 1);
                    // setge: 0F 9D /0
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0x9D);
                    self.mem.emit_byte(0xC0);
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0xB6);
                    self.mem.emit_byte(0xC0);
                    self.mem.emit_byte(0xD1);
                    self.mem.emit_byte(0xE0);
                    self.mem.emit_or_r64_imm8(0, 1);
                    self.emit_jit_stack_push();
                }
                Opcode::StrictEq => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    self.mem.emit_mov_r64_rm64(1, 0);
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[rcx(b)]
                    self.mem.emit_cmp_r64_r64(0, 1);
                    // sete: 0F 94 /0
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0x94);
                    self.mem.emit_byte(0xC0);
                    self.mem.emit_byte(0x0F);
                    self.mem.emit_byte(0xB6);
                    self.mem.emit_byte(0xC0);
                    self.mem.emit_byte(0xD1);
                    self.mem.emit_byte(0xE0);
                    self.mem.emit_or_r64_imm8(0, 1);
                    self.emit_jit_stack_push();
                }
                Opcode::Shl => {
                    self.emit_jit_stack_pop(); // rax = b
                    self.emit_smi_check(bc_idx, &[]); // check b
                    self.mem.emit_mov_r64_rm64(1, 0); // rcx = b
                    self.emit_jit_stack_pop(); // rax = a
                    self.mem.emit_mov_r64_rm64(9, 0); // r9 = a (save)
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[rcx(b)]
                    self.mem.emit_mov_r64_rm64(8, 1); // r8 = b (save)
                    self.mem.emit_sar_r64_1(0); // sar rax, 1 (untag a)
                    self.mem.emit_sar_r64_1(1); // sar rcx, 1 (untag b)
                    // shl rax, cl
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0xD3);
                    self.mem.emit_byte(0xE0); // mod=11, reg=4(shl), r/m=0(rax)
                    self.mem.emit_shl_r64_1(0); // shl rax, 1 (retag)
                    self.mem.emit_or_r64_imm8(0, 1);
                    self.emit_smi_overflow_bailout_or_continue(bc_idx, true, true);
                    self.emit_jit_stack_push();
                }
                Opcode::Shr => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    self.mem.emit_mov_r64_rm64(1, 0);
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[rcx(b)]
                    self.mem.emit_sar_r64_1(0);
                    self.mem.emit_sar_r64_1(1);
                    // sar rax, cl
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0xD3);
                    self.mem.emit_byte(0xF8); // mod=11, reg=7(sar), r/m=0(rax)
                    self.mem.emit_shl_r64_1(0);
                    self.mem.emit_or_r64_imm8(0, 1);
                    self.emit_jit_stack_push();
                }
                Opcode::BitAnd => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    self.mem.emit_mov_r64_rm64(1, 0);
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[rcx(b)]
                    self.mem.emit_sar_r64_1(0);
                    self.mem.emit_sar_r64_1(1);
                    // and rax, rcx
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x23);
                    self.mem.emit_byte(0xC1); // mod=11, reg=0(rax), r/m=1(rcx)
                    self.mem.emit_shl_r64_1(0);
                    self.mem.emit_or_r64_imm8(0, 1);
                    self.emit_jit_stack_push();
                }
                Opcode::BitOr => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    self.mem.emit_mov_r64_rm64(1, 0);
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[rcx(b)]
                    self.mem.emit_sar_r64_1(0);
                    self.mem.emit_sar_r64_1(1);
                    // or rax, rcx
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x0B);
                    self.mem.emit_byte(0xC1); // mod=11, reg=0(rax), r/m=1(rcx)
                    self.mem.emit_shl_r64_1(0);
                    self.mem.emit_or_r64_imm8(0, 1);
                    self.emit_jit_stack_push();
                }
                Opcode::BitXor => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    self.mem.emit_mov_r64_rm64(1, 0);
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[rcx(b)]
                    self.mem.emit_sar_r64_1(0);
                    self.mem.emit_sar_r64_1(1);
                    // xor rax, rcx
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x33);
                    self.mem.emit_byte(0xC1); // mod=11, reg=0(rax), r/m=1(rcx)
                    self.mem.emit_shl_r64_1(0);
                    self.mem.emit_or_r64_imm8(0, 1);
                    self.emit_jit_stack_push();
                }
                Opcode::ShrU => {
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[]); // check b
                    self.mem.emit_mov_r64_rm64(1, 0);
                    self.emit_jit_stack_pop();
                    self.emit_smi_check(bc_idx, &[1]); // check a; saved=[rcx(b)]
                    self.mem.emit_sar_r64_1(0);
                    self.mem.emit_sar_r64_1(1);
                    // shr rax, cl (unsigned shift right)
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0xD3);
                    self.mem.emit_byte(0xE8); // mod=11, reg=5(shr), r/m=0(rax)
                    self.mem.emit_shl_r64_1(0);
                    self.mem.emit_or_r64_imm8(0, 1);
                    self.emit_jit_stack_push();
                }
                Opcode::IncLocal => {
                    let idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    let disp = (idx * 8) as i32;
                    // Load old value
                    self.mem.emit_mov_r64_mem_disp32(0, 13, disp); // rax = locals[idx]
                    self.mem.emit_mov_r64_rm64(1, 0); // rcx = old
                    // Smi increment: old_raw + 2 = Smi(n+1)
                    self.mem.emit_add_r64_imm32(0, 2); // rax = new
                    self.mem.emit_mov_mem_disp32_r64(13, disp, 0); // locals[idx] = new
                    // Push result
                    if is_prefix {
                        self.emit_jit_stack_push(); // push new
                    } else {
                        self.mem.emit_mov_r64_rm64(0, 1); // rax = old
                        self.emit_jit_stack_push(); // push old
                    }
                }
                Opcode::DecLocal => {
                    let idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    let disp = (idx * 8) as i32;
                    self.mem.emit_mov_r64_mem_disp32(0, 13, disp); // rax = locals[idx]
                    self.mem.emit_mov_r64_rm64(1, 0); // rcx = old
                    // Smi decrement: old_raw - 2 = Smi(n-1)
                    self.mem.emit_sub_r64_imm32(0, 2); // rax = new
                    self.mem.emit_mov_mem_disp32_r64(13, disp, 0); // locals[idx] = new
                    if is_prefix {
                        self.emit_jit_stack_push();
                    } else {
                        self.mem.emit_mov_r64_rm64(0, 1);
                        self.emit_jit_stack_push();
                    }
                }
                Opcode::LoadThis => {
                    self.emit_lexical_call(6, 0, 0); // LEX_LOAD_THIS
                    self.emit_jit_stack_push();
                }
                Opcode::BlockEnter => {
                    let count = *instr.operands.first().unwrap_or(&0) as u64;
                    self.emit_lexical_call(0, count, 0); // LEX_BLOCK_ENTER
                }
                Opcode::BlockLeave => {
                    self.emit_lexical_call(1, 0, 0); // LEX_BLOCK_LEAVE
                }
                Opcode::DeclareLet => {
                    let slot = *instr.operands.first().unwrap_or(&0) as u64;
                    self.emit_lexical_call(2, slot, 0); // LEX_DECLARE_LET
                }
                Opcode::DeclareConst => {
                    let slot = *instr.operands.first().unwrap_or(&0) as u64;
                    self.emit_lexical_call(3, slot, 0); // LEX_DECLARE_CONST
                }
                Opcode::LoadLexical => {
                    let slot = *instr.operands.first().unwrap_or(&0) as u64;
                    self.emit_lexical_call(4, slot, 0); // LEX_LOAD
                    self.emit_jit_stack_push(); // push value from rax
                }
                Opcode::StoreLexical => {
                    let slot = *instr.operands.first().unwrap_or(&0) as u64;
                    self.emit_jit_stack_pop(); // rax = value
                    self.mem.emit_mov_r64_rm64(1, 0); // rcx = value (arg2)
                    // Set up args: rdi=vm_ptr, rsi=LEX_STORE, rdx=slot, rcx=value
                    self.mem.emit_mov_r64_rm64(7, 15); // rdi = r15 (vm_ptr)
                    self.mem.emit_mov_r64_imm64(6, 5); // rsi = LEX_STORE (5)
                    self.mem.emit_mov_r64_imm64(2, slot); // rdx = slot
                    // rcx already has value
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);
                    self.mem.emit_byte(0x87);
                    self.mem.emit_u32(512);
                    self.mem.emit_call_r64(0);
                    self.emit_jit_stack_push(); // push result from rax
                }
                Opcode::TypeOf => {
                    // Pop value from JIT stack
                    self.emit_jit_stack_pop();                  // rax = value
                    // rdi = r15 (vm_ptr)
                    self.mem.emit_mov_r64_rm64(7, 15);
                    // rsi = rax (value_raw)
                    self.mem.emit_mov_r64_rm64(6, 0);
                    // Load typeof_helper from [r15 + 528] into rax
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);                   // MOV rax, [r15 + 528]
                    self.mem.emit_byte(0x87);                   // mod=10, reg=0(rax), r/m=7(r15)
                    self.mem.emit_u32(528);                     // disp32
                    self.mem.emit_call_r64(0);                  // call rax
                    // push result (string Value in rax)
                    self.emit_jit_stack_push();
                }
                Opcode::MakeArgumentsArray => {
                    // Phase B: bail on entry — always deopt to interpreter.
                    self.record_bailout_point(bc_idx, BailoutReason::BailOnEntry);
                    self.mem.emit_mov_r64_rm64(7, 15);       // rdi = r15
                    self.mem.emit_mov_r64_imm64(6, bc_idx as u64); // rsi = bc_pc
                    self.mem.emit_mov_r64_rm64(2, 3);        // rdx = rbx
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);                // MOV rax, [r15 + 520]
                    self.mem.emit_byte(0x87);                // mod=10, reg=0(rax), r/m=7(r15)
                    self.mem.emit_u32(520);                  // disp32
                    self.mem.emit_call_r64(0);               // call rax
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x31);
                    self.mem.emit_byte(0xC0);                // xor eax, eax
                    self.emit_jit_stack_push();
                    self.emit_epilogue();
                }
                Opcode::LoadGlobal => {
                    let name_idx = instr.operands[0] as u64;
                    let prog_ptr = program as *const BytecodeProgram as *const u8 as u64;
                    // rdi = r15 (vm_ptr)
                    self.mem.emit_mov_r64_rm64(7, 15);
                    // rsi = r14 (gc_ptr)
                    self.mem.emit_mov_r64_rm64(6, 14);
                    // rdx = prog_ptr (immediate)
                    self.mem.emit_mov_r64_imm64(2, prog_ptr);
                    // rcx = 0 (op: LoadGlobal)
                    self.mem.emit_mov_r64_imm64(1, 0);
                    // r8 = name_idx
                    self.mem.emit_mov_r64_imm64(8, name_idx);
                    // r9 = 0 (value_raw, unused for load)
                    self.mem.emit_mov_r64_imm64(9, 0);
                    // Load global_helper from [r15 + 544] into rax
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);                   // MOV rax, [r15 + 544]
                    self.mem.emit_byte(0x87);                   // mod=10, reg=0(rax), r/m=7(r15)
                    self.mem.emit_u32(544);                     // disp32
                    self.mem.emit_call_r64(0);                  // call rax
                    // push result (Value in rax)
                    self.emit_jit_stack_push();
                }
                Opcode::StoreGlobal => {
                    let name_idx = instr.operands[0] as u64;
                    let prog_ptr = program as *const BytecodeProgram as *const u8 as u64;
                    // Pop value to store from JIT stack into r9
                    self.emit_jit_stack_pop();                  // rax = value
                    self.mem.emit_mov_r64_rm64(9, 0);           // r9 = value_raw
                    // rdi = r15 (vm_ptr)
                    self.mem.emit_mov_r64_rm64(7, 15);
                    // rsi = r14 (gc_ptr)
                    self.mem.emit_mov_r64_rm64(6, 14);
                    // rdx = prog_ptr (immediate)
                    self.mem.emit_mov_r64_imm64(2, prog_ptr);
                    // rcx = 1 (op: StoreGlobal)
                    self.mem.emit_mov_r64_imm64(1, 1);
                    // r8 = name_idx
                    self.mem.emit_mov_r64_imm64(8, name_idx);
                    // Load global_helper from [r15 + 544] into rax
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);                   // MOV rax, [r15 + 544]
                    self.mem.emit_byte(0x87);                   // mod=10, reg=0(rax), r/m=7(r15)
                    self.mem.emit_u32(544);                     // disp32
                    self.mem.emit_call_r64(0);                  // call rax
                    // push result (stored Value in rax)
                    self.emit_jit_stack_push();
                }
                Opcode::IncGlobal | Opcode::DecGlobal => {
                    let name_idx = instr.operands[0] as u64;
                    let is_prefix = instr.operands[1];
                    let op = if matches!(instr.opcode, Opcode::IncGlobal) { 2u64 } else { 3u64 };
                    let prog_ptr = program as *const BytecodeProgram as *const u8 as u64;
                    // rdi = r15 (vm_ptr)
                    self.mem.emit_mov_r64_rm64(7, 15);
                    // rsi = r14 (gc_ptr)
                    self.mem.emit_mov_r64_rm64(6, 14);
                    // rdx = prog_ptr (immediate)
                    self.mem.emit_mov_r64_imm64(2, prog_ptr);
                    // rcx = op
                    self.mem.emit_mov_r64_imm64(1, op);
                    // r8 = name_idx
                    self.mem.emit_mov_r64_imm64(8, name_idx);
                    // r9 = is_prefix
                    self.mem.emit_mov_r64_imm64(9, is_prefix as u64);
                    // Load global_helper from [r15 + 544] into rax
                    self.mem.emit_rex_w();
                    self.mem.emit_byte(0x8B);                   // MOV rax, [r15 + 544]
                    self.mem.emit_byte(0x87);                   // mod=10, reg=0(rax), r/m=7(r15)
                    self.mem.emit_u32(544);                     // disp32
                    self.mem.emit_call_r64(0);                  // call rax
                    // push result (Value in rax)
                    self.emit_jit_stack_push();
                }
                _ => {
                    self.mem.emit_byte(0xCC);
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
            captured_env_size: 0,
        }
    }

    /// Returns a pointer to a 1 KB buffer with bailout_helper set at offset 520.
    /// The allocation is intentionally leaked — it lives for the test duration.
    #[cfg(target_arch = "x86_64")]
    fn vm_stub() -> *mut u8 {
        extern "C" fn bailout_stub(_vm: *mut u8, _bc_pc: usize, _jit_sp: *mut u64) -> u64 {
            0
        }
        let buf = Box::new([0u8; 1024]);
        let ptr = Box::into_raw(buf) as *mut u8;
        unsafe {
            let bailout_ptr = ptr.add(520) as *mut usize;
            *bailout_ptr = bailout_stub as *const () as usize;
        }
        ptr
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_load_smi_return() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        // vm_ptr and gc_ptr are unused for this simple program
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, 0u64); // undefined = Value(0)
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_null() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadNull, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, 2u64); // null = Value(2)
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_load_true() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadBoolean, vec![1]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, 6u64); // Value::boolean(true) = 0x06
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_load_false() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadBoolean, vec![0]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, 4u64); // Value::boolean(false) = 0x04
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_chained_arithmetic() {
        // (10 + 20) * 3 - 5
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![10]),
            Instruction::new(Opcode::LoadSmi, vec![20]),
            Instruction::new(Opcode::Add, vec![]), // 30
            Instruction::new(Opcode::LoadSmi, vec![3]),
            Instruction::new(Opcode::Mul, vec![]), // 90
            Instruction::new(Opcode::LoadSmi, vec![5]),
            Instruction::new(Opcode::Sub, vec![]), // 85
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();

        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, 199u64); // Smi(99) = 199
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_conditional_undefined_falsy() {
        // if (0) { return 42; } else { return 99; }
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![0]),
            Instruction::new(Opcode::JumpIfFalse, vec![4]),
            Instruction::new(Opcode::LoadSmi, vec![42]),
            Instruction::new(Opcode::Return, vec![]),
            Instruction::new(Opcode::LoadSmi, vec![99]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(result, 199u64); // Smi(99) = 199
    }

    // -------------------------------------------------------------------
    // Non-execution tests (verify emit offset / byte count)
    // -------------------------------------------------------------------

    #[test]
    fn test_compile_empty_then_return() {
        // A program with just Return: should emit prologue, pop (which underflows
        // but the bytes are still valid), and epilogue. Verify it doesn't panic.
        let prog = make_prog(vec![Instruction::new(Opcode::Return, vec![])]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        // We can't easily verify the exact offset without duplicating the
        // codegen logic, but we can verify that it emitted something.
        assert!(compiled.mem.offset > 0);
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        // Verify it emitted a reasonable number of bytes (within 55-85)
        assert!(compiled.mem.offset >= 60, "offset was {}", compiled.mem.offset);
        assert!(compiled.mem.offset <= 95, "offset was {}", compiled.mem.offset);
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        // Provide a local slot via a stack-allocated array
        let mut locals: [u64; 1] = [0; 1];
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                locals.as_mut_ptr(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let mut locals: [u64; 1] = [0; 1];
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                locals.as_mut_ptr(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let mut locals: [u64; 1] = [0; 1];
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                locals.as_mut_ptr(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let mut locals: [u64; 1] = [0; 1];
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                locals.as_mut_ptr(),
            )
        };
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
            Instruction::new(Opcode::JumpIfFalse, vec![18]),
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let mut locals: [u64; 2] = [0; 2];
        let result = unsafe {
            func(
                vm_stub(),
                std::ptr::null_mut(),
                locals.as_mut_ptr(),
            )
        };
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        assert!(compiled.mem.offset >= 65, "offset was {}", compiled.mem.offset);
        assert!(compiled.mem.offset <= 120, "offset was {}", compiled.mem.offset);
    }

    #[test]
    fn test_compile_lt_offset() {
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![3]),
            Instruction::new(Opcode::LoadSmi, vec![5]),
            Instruction::new(Opcode::Lt, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        assert!(compiled.mem.offset >= 200, "offset was {}", compiled.mem.offset);
        assert!(compiled.mem.offset <= 500, "offset was {}", compiled.mem.offset);
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
            Instruction::new(Opcode::JumpIfFalse, vec![18]),
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
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        assert!(compiled.mem.offset >= 800, "offset was {}", compiled.mem.offset);
        assert!(compiled.mem.offset <= 1500, "offset was {}", compiled.mem.offset);
    }

    // -----------------------------------------------------------------------
    // Overflow guard tests (x86_64)
    // -----------------------------------------------------------------------

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_add_overflow() {
        // (2^30 − 1) + 1 = 2^30 → exceeds i31 → bailout
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![1073741823]),
            Instruction::new(Opcode::LoadSmi, vec![1]),
            Instruction::new(Opcode::Add, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm_stub(), std::ptr::null_mut(), std::ptr::null_mut()) };
        // Overflow → bailout returns undefined = 0
        assert_eq!(result, 0, "Add overflow: expected 0 (bailout), got {}", result);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_sub_overflow() {
        // −2^30 − 1 = −(2^30+1) < −2^30 → underflow → bailout
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![-1073741824]),
            Instruction::new(Opcode::LoadSmi, vec![1]),
            Instruction::new(Opcode::Sub, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm_stub(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 0, "Sub underflow: expected 0 (bailout), got {}", result);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_mul_overflow() {
        // 2^16 × 2^16 = 2^32 > 2^30−1 → overflow → bailout
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![65536]),
            Instruction::new(Opcode::LoadSmi, vec![65536]),
            Instruction::new(Opcode::Mul, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm_stub(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 0, "Mul overflow: expected 0 (bailout), got {}", result);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_neg_overflow() {
        // −(−2^30) = 2^30 > 2^30−1 → overflow → bailout
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![-1073741824]),
            Instruction::new(Opcode::Neg, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm_stub(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 0, "Neg overflow: expected 0 (bailout), got {}", result);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_jit_shl_overflow() {
        // 1 << 31 = 2^31 > 2^30−1 → overflow → bailout
        let prog = make_prog(vec![
            Instruction::new(Opcode::LoadSmi, vec![1]),
            Instruction::new(Opcode::LoadSmi, vec![31]),
            Instruction::new(Opcode::Shl, vec![]),
            Instruction::new(Opcode::Return, vec![]),
        ]);
        let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
        compiled.mem.make_executable();
        let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
        let result = unsafe { func(vm_stub(), std::ptr::null_mut(), std::ptr::null_mut()) };
        assert_eq!(result, 0, "Shl overflow: expected 0 (bailout), got {}", result);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_non_smi_input_bailouts() {
        let run = |instrs: Vec<Instruction>| -> u64 {
            let prog = make_prog(instrs);
            let compiled = CodeGen::new(prog.instructions.len()).compile(&prog);
            compiled.mem.make_executable();
            let func: JitEntryFn = unsafe { std::mem::transmute(compiled.mem.code_ptr()) };
            unsafe { func(vm_stub(), std::ptr::null_mut(), std::ptr::null_mut()) }
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

        // Binary ops — non-Smi a (second operand), Smi b — skips first guard, hits second
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
        for &(op, name) in &[(Opcode::JumpIfFalse, "JumpIfFalse"), (Opcode::JumpIfTrue, "JumpIfTrue")] {
            let r = run(vec![
                Instruction::new(Opcode::LoadUndefined, vec![]),
                Instruction::new(op, vec![3]),
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::Return, vec![]),
            ]);
            assert_eq!(r, 0, "{}: non-Smi should bail, got {}", name, r);
        }
    }
}
