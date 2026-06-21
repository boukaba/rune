/// Executable memory allocator (W^X-compliant) + x86-64 assembler.
///
/// Platform support:
/// - macOS: mmap with MAP_JIT, mprotect to PROT_READ|PROT_EXEC for finalize
/// - Linux: mmap with MAP_PRIVATE|MAP_ANONYMOUS, mprotect to PROT_READ|PROT_EXEC
use std::ptr;

pub struct ExecutableMemory {
    pub ptr: *mut u8,
    pub size: usize,
    pub offset: usize,
}

impl ExecutableMemory {
    #[cfg(target_os = "macos")]
    pub fn allocate(size: usize) -> Self {
        let page_size = 4096;
        let alloc_size = (size + page_size - 1) & !(page_size - 1);
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                alloc_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_JIT,
                -1,
                0,
            )
        };
        assert_ne!(
            ptr,
            libc::MAP_FAILED,
            "ExecutableMemory::allocate mmap MAP_JIT failed"
        );
        ExecutableMemory {
            ptr: ptr as *mut u8,
            size: alloc_size,
            offset: 0,
        }
    }

    #[cfg(target_os = "linux")]
    pub fn allocate(size: usize) -> Self {
        let page_size = 4096;
        let alloc_size = (size + page_size - 1) & !(page_size - 1);
        let ptr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                alloc_size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(
            ptr,
            libc::MAP_FAILED,
            "ExecutableMemory::allocate mmap failed"
        );
        ExecutableMemory {
            ptr: ptr as *mut u8,
            size: alloc_size,
            offset: 0,
        }
    }

    pub fn emit_byte(&mut self, b: u8) {
        assert!(
            self.offset < self.size,
            "ExecutableMemory emit_byte overflow"
        );
        unsafe {
            ptr::write(self.ptr.add(self.offset), b);
        }
        self.offset += 1;
    }

    pub fn emit_u32(&mut self, v: u32) {
        assert!(
            self.offset + 4 <= self.size,
            "ExecutableMemory emit_u32 overflow"
        );
        unsafe {
            ptr::write_unaligned(self.ptr.add(self.offset) as *mut u32, v);
        }
        self.offset += 4;
    }

    pub fn emit_u64(&mut self, v: u64) {
        assert!(
            self.offset + 8 <= self.size,
            "ExecutableMemory emit_u64 overflow"
        );
        unsafe {
            ptr::write_unaligned(self.ptr.add(self.offset) as *mut u64, v);
        }
        self.offset += 8;
    }

    pub fn patch_u32(&mut self, offset: usize, v: u32) {
        assert!(
            offset + 4 <= self.size,
            "ExecutableMemory patch_u32 overflow"
        );
        unsafe {
            ptr::write_unaligned(self.ptr.add(offset) as *mut u32, v);
        }
    }

    pub fn make_executable(&self) {
        let ret = unsafe {
            libc::mprotect(
                self.ptr as *mut libc::c_void,
                self.size,
                libc::PROT_READ | libc::PROT_EXEC,
            )
        };
        assert_eq!(ret, 0, "ExecutableMemory mprotect to RX failed");
    }

    pub fn code_ptr(&self) -> *const u8 {
        self.ptr as *const u8
    }

    pub fn current_offset(&self) -> usize {
        self.offset
    }
}

impl Drop for ExecutableMemory {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut libc::c_void, self.size);
        }
    }
}

// ---------------------------------------------------------------------------
// REX byte helpers
// ---------------------------------------------------------------------------

fn rex_byte(w: bool, r: bool, x: bool, b: bool) -> u8 {
    0x40 | (if w { 0x08 } else { 0 })
        | (if r { 0x04 } else { 0 })
        | (if x { 0x02 } else { 0 })
        | (if b { 0x01 } else { 0 })
}

/// Emit a ModRM byte for register/register mode (mod = 11).
/// `opcode_ext` is the reg/opcode field; `rm` is the r/m field.
fn modrm_reg_reg(opcode_ext: u8, rm: u8) -> u8 {
    0xC0 | ((opcode_ext & 7) << 3) | (rm & 7)
}

// ---------------------------------------------------------------------------
// x86-64 instruction emission helpers
// ---------------------------------------------------------------------------

impl ExecutableMemory {
    /// Emit a REX prefix byte.
    fn emit_rex(&mut self, w: bool, r: bool, x: bool, b: bool) {
        self.emit_byte(rex_byte(w, r, x, b));
    }

    /// ret  (0xC3)
    pub fn emit_ret(&mut self) {
        self.emit_byte(0xC3);
    }

    /// nop  (0x90)
    pub fn emit_nop(&mut self) {
        self.emit_byte(0x90);
    }

    /// mov r64, imm64  (REX.W + B8+rd + imm64)
    pub fn emit_mov_r64_imm64(&mut self, reg: u8, imm: u64) {
        let b = (reg >> 3) & 1;
        self.emit_rex(true, false, false, b != 0);
        self.emit_byte(0xB8 | (reg & 7));
        self.emit_u64(imm);
    }

    /// mov r64, r/m64  (REX.W + 8B /r)
    /// dst = ModRM.reg (extended by REX.R), src = ModRM.r/m (extended by REX.B)
    pub fn emit_mov_r64_rm64(&mut self, dst: u8, src: u8) {
        let r = (dst >> 3) & 1;
        let b = (src >> 3) & 1;
        self.emit_rex(true, r != 0, false, b != 0);
        self.emit_byte(0x8B);
        self.emit_byte(modrm_reg_reg(dst, src));
    }

    /// MOV r64, [r64 + disp32]  (REX.W + 8B /r + disp32)
    /// Loads a 64-bit value from memory at [base + disp32] into `reg`.
    pub fn emit_mov_r64_mem_disp32(&mut self, reg: u8, base: u8, disp: i32) {
        let r = (reg >> 3) & 1;
        let b = (base >> 3) & 1;
        self.emit_rex(true, r != 0, false, b != 0);
        self.emit_byte(0x8B);
        self.emit_byte(0x80 | ((reg & 7) << 3) | (base & 7));
        self.emit_u32(disp as u32);
    }

    /// MOV [r64 + disp32], r64  (REX.W + 89 /r + disp32)
    /// Stores a 64-bit value in `reg` to memory at [base + disp32].
    pub fn emit_mov_mem_disp32_r64(&mut self, base: u8, disp: i32, reg: u8) {
        let b = (base >> 3) & 1;
        let r = (reg >> 3) & 1;
        self.emit_rex(true, r != 0, false, b != 0);
        self.emit_byte(0x89);
        self.emit_byte(0x80 | ((reg & 7) << 3) | (base & 7));
        self.emit_u32(disp as u32);
    }

    /// Emit 81 /0 id: ADD r64, imm32
    pub fn emit_add_r64_imm32(&mut self, reg: u8, imm: i32) {
        let b = (reg >> 3) & 1;
        self.emit_rex(true, false, false, b != 0);
        self.emit_byte(0x81);
        self.emit_byte(modrm_reg_reg(0, reg));
        self.emit_u32(imm as u32);
    }

    /// Emit 81 /5 id: SUB r64, imm32
    pub fn emit_sub_r64_imm32(&mut self, reg: u8, imm: i32) {
        let b = (reg >> 3) & 1;
        self.emit_rex(true, false, false, b != 0);
        self.emit_byte(0x81);
        self.emit_byte(modrm_reg_reg(5, reg));
        self.emit_u32(imm as u32);
    }

    /// Emit 81 /7 id: CMP r64, imm32
    pub fn emit_cmp_r64_imm32(&mut self, reg: u8, imm: i32) {
        let b = (reg >> 3) & 1;
        self.emit_rex(true, false, false, b != 0);
        self.emit_byte(0x81);
        self.emit_byte(modrm_reg_reg(7, reg));
        self.emit_u32(imm as u32);
    }

    /// Emit E9 cd: JMP rel32
    /// Returns the offset of the 4-byte displacement (for later patching).
    pub fn emit_jmp_rel32(&mut self, offset: i32) -> usize {
        let patch_offset = self.offset + 1; // after the opcode byte
        self.emit_byte(0xE9);
        self.emit_u32(offset as u32);
        patch_offset
    }

    /// Emit 0F 84 cd: JE rel32 (jump if equal / zero)
    /// Returns the offset of the 4-byte displacement (for later patching).
    pub fn emit_je_rel32(&mut self, offset: i32) -> usize {
        let patch_offset = self.offset + 2; // after both opcode bytes
        self.emit_byte(0x0F);
        self.emit_byte(0x84);
        self.emit_u32(offset as u32);
        patch_offset
    }

    /// Emit 0F 85 cd: JNE rel32 (jump if not equal / not zero)
    /// Returns the offset of the 4-byte displacement (for later patching).
    pub fn emit_jne_rel32(&mut self, offset: i32) -> usize {
        let patch_offset = self.offset + 2; // after both opcode bytes
        self.emit_byte(0x0F);
        self.emit_byte(0x85);
        self.emit_u32(offset as u32);
        patch_offset
    }

    /// Emit FF /2: CALL r64
    pub fn emit_call_r64(&mut self, reg: u8) {
        let b = (reg >> 3) & 1;
        if b != 0 {
            self.emit_rex(false, false, false, true);
        }
        self.emit_byte(0xFF);
        self.emit_byte(modrm_reg_reg(2, reg));
    }

    /// Emit push r64  (50+rd; REX.B for extended registers)
    pub fn emit_push_r64(&mut self, reg: u8) {
        let b = (reg >> 3) & 1;
        if b != 0 {
            self.emit_byte(0x41); // REX.B
        }
        self.emit_byte(0x50 | (reg & 7));
    }

    /// Emit pop r64  (58+rd; REX.B for extended registers)
    pub fn emit_pop_r64(&mut self, reg: u8) {
        let b = (reg >> 3) & 1;
        if b != 0 {
            self.emit_byte(0x41); // REX.B
        }
        self.emit_byte(0x58 | (reg & 7));
    }

    // -- Additional x86-64 helpers for codegen --

    /// Emit just a REX.W prefix byte (0x48).
    pub fn emit_rex_w(&mut self) {
        self.emit_byte(0x48);
    }

    /// AND r64, imm8  (83 /4 ib)
    pub fn emit_and_r64_imm8(&mut self, reg: u8, imm: i8) {
        let b = (reg >> 3) & 1;
        self.emit_rex(true, false, false, b != 0);
        self.emit_byte(0x83);
        self.emit_byte(modrm_reg_reg(4, reg));
        self.emit_byte(imm as u8);
    }

    /// OR r64, imm8  (83 /1 ib)
    pub fn emit_or_r64_imm8(&mut self, reg: u8, imm: i8) {
        let b = (reg >> 3) & 1;
        self.emit_rex(true, false, false, b != 0);
        self.emit_byte(0x83);
        self.emit_byte(modrm_reg_reg(1, reg));
        self.emit_byte(imm as u8);
    }

    /// ADD r/m64, r64  (01 /r)  → r/m += reg
    pub fn emit_add_r64_r64(&mut self, r_m: u8, reg: u8) {
        let b_rm = (r_m >> 3) & 1;
        let b_reg = (reg >> 3) & 1;
        self.emit_rex(true, b_reg != 0, false, b_rm != 0);
        self.emit_byte(0x01);
        self.emit_byte(modrm_reg_reg(reg, r_m));
    }

    /// SUB r/m64, r64  (29 /r)  → r/m -= reg
    pub fn emit_sub_r64_r64(&mut self, r_m: u8, reg: u8) {
        let b_rm = (r_m >> 3) & 1;
        let b_reg = (reg >> 3) & 1;
        self.emit_rex(true, b_reg != 0, false, b_rm != 0);
        self.emit_byte(0x29);
        self.emit_byte(modrm_reg_reg(reg, r_m));
    }

    /// IMUL r64, r/m64  (0F AF /r)  → dst *= src
    pub fn emit_imul_r64_r64(&mut self, dst: u8, src: u8) {
        let r = (dst >> 3) & 1;
        let b = (src >> 3) & 1;
        self.emit_rex(true, r != 0, false, b != 0);
        self.emit_byte(0x0F);
        self.emit_byte(0xAF);
        self.emit_byte(modrm_reg_reg(dst, src));
    }

    /// SAR r/m64, 1  (D1 /7) — arithmetic shift right by 1
    pub fn emit_sar_r64_1(&mut self, reg: u8) {
        let b = (reg >> 3) & 1;
        self.emit_rex(true, false, false, b != 0);
        self.emit_byte(0xD1);
        self.emit_byte(modrm_reg_reg(7, reg));
    }

    /// SHL r/m64, 1  (D1 /4) — logical shift left by 1
    pub fn emit_shl_r64_1(&mut self, reg: u8) {
        let b = (reg >> 3) & 1;
        self.emit_rex(true, false, false, b != 0);
        self.emit_byte(0xD1);
        self.emit_byte(modrm_reg_reg(4, reg));
    }

    /// CMP r/m64, r64  (39 /r) — compare r/m with reg, sets flags
    pub fn emit_cmp_r64_r64(&mut self, r_m: u8, reg: u8) {
        let b_rm = (r_m >> 3) & 1;
        let b_reg = (reg >> 3) & 1;
        self.emit_rex(true, b_reg != 0, false, b_rm != 0);
        self.emit_byte(0x39);
        self.emit_byte(modrm_reg_reg(reg, r_m));
    }

    /// JBE rel32  (0F 86 cd) — jump if below or equal (unsigned ≤, CF=1 or ZF=1)
    pub fn emit_jbe_rel32(&mut self, offset: i32) -> usize {
        let patch_offset = self.offset + 2;
        self.emit_byte(0x0F);
        self.emit_byte(0x86);
        self.emit_u32(offset as u32);
        patch_offset
    }

    /// JB rel32  (0F 82 cd) — jump if below (unsigned <, CF=1)
    pub fn emit_jb_rel32(&mut self, offset: i32) -> usize {
        let patch_offset = self.offset + 2;
        self.emit_byte(0x0F);
        self.emit_byte(0x82);
        self.emit_u32(offset as u32);
        patch_offset
    }

    /// JA rel32  (0F 87 cd) — jump if above (unsigned >, CF=0 and ZF=0)
    pub fn emit_ja_rel32(&mut self, offset: i32) -> usize {
        let patch_offset = self.offset + 2;
        self.emit_byte(0x0F);
        self.emit_byte(0x87);
        self.emit_u32(offset as u32);
        patch_offset
    }

    /// JAE rel32  (0F 83 cd) — jump if above or equal (unsigned ≥, CF=0)
    pub fn emit_jae_rel32(&mut self, offset: i32) -> usize {
        let patch_offset = self.offset + 2;
        self.emit_byte(0x0F);
        self.emit_byte(0x83);
        self.emit_u32(offset as u32);
        patch_offset
    }
}

// ---------------------------------------------------------------------------
// Convenient wrapper for JIT compilation
// ---------------------------------------------------------------------------

/// A compiled JIT function that takes no arguments and returns a u64.
pub type JitFn0 = unsafe fn() -> u64;

/// Compile a simple "return 42" function as a smoke test.
#[cfg(target_arch = "x86_64")]
pub fn compile_return_42() -> ExecutableMemory {
    let mut mem = ExecutableMemory::allocate(128);
    mem.emit_mov_r64_imm64(0, 42); // mov rax, 42
    mem.emit_ret();
    mem.make_executable();
    mem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emit_ret() {
        let mut mem = ExecutableMemory::allocate(4096);
        mem.emit_ret();
        // don't call make_executable on non-x86_64 — ARM macOS has different
        // MAP_JIT semantics; the mprotect call is platform-agnostic but we keep
        // the test focused on byte emission
        mem.make_executable();
        assert_eq!(mem.size, 4096);
        assert_eq!(mem.offset, 1);
    }

    #[test]
    fn test_emit_mov_r64_imm64() {
        let mut mem = ExecutableMemory::allocate(4096);
        mem.emit_mov_r64_imm64(0, 42);
        mem.emit_ret();
        // REX.W(1) + B8+rd(1) + imm64(8) = 10 bytes + ret(1) = 11
        assert_eq!(mem.offset, 11);
    }

    #[test]
    fn test_emit_mov_r64_imm64_r8() {
        let mut mem = ExecutableMemory::allocate(4096);
        mem.emit_mov_r64_imm64(8, 42); // mov r8, 42
        mem.emit_ret();
        // REX.W|B(1) + B8+rd(1) + imm64(8) = 10 bytes + ret(1) = 11
        assert_eq!(mem.offset, 11);
    }

    #[test]
    fn test_emit_mov_r64_rm64() {
        // mov rax, rcx ; REX.W(48) + 8B + ModRM(C1) = 3 bytes
        let mut mem = ExecutableMemory::allocate(256);
        mem.emit_mov_r64_rm64(0, 1);
        assert_eq!(mem.offset, 3);
    }

    #[test]
    fn test_emit_arithmetic() {
        // add rax, 10
        let mut mem = ExecutableMemory::allocate(256);
        mem.emit_add_r64_imm32(0, 10);
        assert_eq!(mem.offset, 7); // REX.W(1) + 81(1) + ModRM(1) + imm32(4)
    }

    #[test]
    fn test_emit_push_pop() {
        let mut mem = ExecutableMemory::allocate(256);
        mem.emit_push_r64(0);
        mem.emit_pop_r64(0);
        assert_eq!(mem.offset, 2); // push rax(1) + pop rax(1)
    }

    #[test]
    fn test_emit_push_pop_r8() {
        let mut mem = ExecutableMemory::allocate(256);
        mem.emit_push_r64(8); // push r8  → 41 50
        mem.emit_pop_r64(8); // pop r8   → 41 58
        assert_eq!(mem.offset, 4); // REX.B(1) + opcode(1) each
    }

    #[test]
    fn test_emit_jmp_rel32() {
        let mut mem = ExecutableMemory::allocate(256);
        let patch = mem.emit_jmp_rel32(42);
        assert_eq!(mem.offset, 5); // opcode(1) + disp32(4)
        assert_eq!(patch, 1);
    }

    #[test]
    fn test_emit_je_rel32() {
        let mut mem = ExecutableMemory::allocate(256);
        let patch = mem.emit_je_rel32(42);
        assert_eq!(mem.offset, 6); // 0F(1) + 84(1) + disp32(4)
        assert_eq!(patch, 2);
    }

    #[test]
    fn test_emit_call_r64() {
        let mut mem = ExecutableMemory::allocate(256);
        mem.emit_call_r64(5); // call rbp — no REX needed
        assert_eq!(mem.offset, 2);
    }

    #[test]
    fn test_emit_call_r64_extended() {
        let mut mem = ExecutableMemory::allocate(256);
        mem.emit_call_r64(8); // call r8 — REX.B + FF + ModRM
        assert_eq!(mem.offset, 3);
    }

    #[test]
    fn test_patch_u32() {
        let mut mem = ExecutableMemory::allocate(256);
        let patch = mem.emit_jmp_rel32(0); // placeholder
        mem.patch_u32(patch, 1234);
        unsafe {
            let val = ptr::read_unaligned(mem.ptr.add(patch) as *const u32);
            assert_eq!(val, 1234);
        }
    }

    #[test]
    fn test_emit_cmp_r64_r64() {
        let mut mem = ExecutableMemory::allocate(256);
        mem.emit_cmp_r64_r64(0, 1); // cmp rax, rcx
        assert_eq!(mem.offset, 3); // REX.W(1) + 39(1) + ModRM(1)
    }

    #[test]
    fn test_emit_jbe_rel32() {
        let mut mem = ExecutableMemory::allocate(256);
        let patch = mem.emit_jbe_rel32(42);
        assert_eq!(mem.offset, 6); // 0F(1) + 86(1) + disp32(4)
        assert_eq!(patch, 2);
    }

    #[test]
    fn test_emit_jb_rel32() {
        let mut mem = ExecutableMemory::allocate(256);
        let patch = mem.emit_jb_rel32(42);
        assert_eq!(mem.offset, 6);
        assert_eq!(patch, 2);
    }

    #[test]
    fn test_emit_ja_rel32() {
        let mut mem = ExecutableMemory::allocate(256);
        let patch = mem.emit_ja_rel32(42);
        assert_eq!(mem.offset, 6);
        assert_eq!(patch, 2);
    }

    #[test]
    fn test_emit_jae_rel32() {
        let mut mem = ExecutableMemory::allocate(256);
        let patch = mem.emit_jae_rel32(42);
        assert_eq!(mem.offset, 6);
        assert_eq!(patch, 2);
    }

    // -----------------------------------------------------------------------
    // Execution tests: only run on x86-64 where the JIT code is native
    // -----------------------------------------------------------------------

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_executable_call() {
        let mem = compile_return_42();
        let func: unsafe fn() -> u64 = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func() };
        assert_eq!(result, 42);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_executable_add_constant() {
        // System V AMD64 ABI: first argument in RDI (reg 7).
        // Emit: mov rax, rdi; add rax, 7; ret
        let mut mem = ExecutableMemory::allocate(256);
        mem.emit_mov_r64_rm64(0, 7); // rax = rdi (first arg)
        mem.emit_add_r64_imm32(0, 7);
        mem.emit_ret();
        mem.make_executable();

        let func: unsafe fn(u64) -> u64 = unsafe { std::mem::transmute(mem.code_ptr()) };
        let result = unsafe { func(35) };
        assert_eq!(result, 42);
    }
}
