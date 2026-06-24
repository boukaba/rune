pub mod assembler;
pub mod codegen;
#[cfg(target_arch = "aarch64")]
pub mod codegen_aarch64;
pub mod ic;
pub mod templates;

pub use codegen::{CodeGen, JitEntryFn};
#[cfg(target_arch = "aarch64")]
pub use codegen_aarch64::compile_trace;

/// Check if a BytecodeProgram only uses opcodes the JIT can currently handle.
pub fn is_jit_compatible(prog: &rune_bytecode::opcode::BytecodeProgram) -> bool {
    use rune_bytecode::opcode::Opcode;
    for instr in &prog.instructions {
        match instr.opcode {
            Opcode::LoadSmi
            | Opcode::LoadUndefined
            | Opcode::LoadNull
            | Opcode::LoadBoolean
            | Opcode::LoadLocal
            | Opcode::StoreLocal
            | Opcode::Add
            | Opcode::Sub
            | Opcode::Mul
            | Opcode::Lt
            | Opcode::Pop
            | Opcode::Dup
            | Opcode::Return
            | Opcode::Jump
            | Opcode::JumpIfFalse
            | Opcode::JumpIfTrue
            | Opcode::DecLocal
            | Opcode::LoadPropertyIC => {}
            _ => return false,
        }
    }
    true
}
