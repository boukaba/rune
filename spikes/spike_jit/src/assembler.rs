//! Minimal AArch64 assembler for copy-and-patch JIT spike.

/// Encode ADD Rd, Rn, Rm (64-bit, shifted register, LSL#0)
///
/// Encoding:
///   31: sf = 1
///   30-29: opc = 00
///   28: S = 0
///   27-24: 1011 (ADD shifted register)
///   23-22: shift = 00 (LSL)
///   21: 0
///   20-16: Rm
///   15-10: imm6 = 000000
///   9-5: Rn
///   4-0: Rd
///
/// Base: 0x8B000000
fn a64_add(rd: u8, rn: u8, rm: u8) -> u32 {
    let base = 0x8B000000u32;
    base | (rm as u32) << 16 | (rn as u32) << 5 | (rd as u32)
}

/// Encode RET (return to X30/LR)
fn a64_ret() -> u32 {
    0xD65F03C0u32
}

/// Emit an instruction (4 bytes) into the buffer.
fn emit(buf: &mut Vec<u8>, instr: u32) {
    buf.extend_from_slice(&instr.to_le_bytes());
}

/// Result of executing a compiled JIT function.
pub type JitFn = unsafe extern "C" fn(i64, i64, i64) -> i64;

/// Compile add3(a, b, c) = a + b + c
///
/// Arguments: x0=a, x1=b, x2=c
/// Result: x0
pub fn compile_toy_jit() -> (Vec<u8>, JitFn) {
    let mut code = Vec::new();

    // ADD x0, x0, x1  → x0 = a + b
    emit(&mut code, a64_add(0, 0, 1));
    // ADD x0, x0, x2  → x0 = (a+b) + c
    emit(&mut code, a64_add(0, 0, 2));
    // RET
    emit(&mut code, a64_ret());

    let func: JitFn = unsafe { std::mem::transmute(code.as_ptr()) };
    (code, func)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_encoding() {
        let instr = a64_add(0, 0, 1);
        // Verify opcode pattern: bits 31-21 should be 10001011000
        assert_eq!(
            (instr >> 21) & 0x7FF,
            0b10001011000,
            "ADD encoding pattern wrong"
        );
        assert_eq!((instr >> 16) & 0x1f, 1, "Rm should be 1");
        assert_eq!((instr >> 5) & 0x1f, 0, "Rn should be 0");
        assert_eq!(instr & 0x1f, 0, "Rd should be 0");
    }

    #[test]
    fn test_ret_encoding() {
        assert_eq!(a64_ret(), 0xD65F03C0);
    }
}
