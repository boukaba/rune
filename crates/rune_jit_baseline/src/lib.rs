pub mod assembler;
pub mod codegen;
#[cfg(target_arch = "aarch64")]
pub mod codegen_aarch64;
pub mod ic;
pub mod templates;

pub use codegen::{CodeGen, JitEntryFn};
#[cfg(target_arch = "aarch64")]
pub use codegen_aarch64::{compile_trace, Aarch64CodeGen};

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
            | Opcode::Gt
            | Opcode::Le
            | Opcode::Ge
            | Opcode::StrictEq
            | Opcode::Neg
            | Opcode::Not
            | Opcode::Void
            | Opcode::StrictNe
            | Opcode::Shl
            | Opcode::Shr
            | Opcode::BitAnd
            | Opcode::BitOr
            | Opcode::BitXor
            | Opcode::Pop
            | Opcode::Dup
            | Opcode::Return
            | Opcode::Jump
            | Opcode::JumpIfFalse
            | Opcode::JumpIfTrue
            | Opcode::IncLocal
            | Opcode::DecLocal
            | Opcode::UnaryPlus
            | Opcode::BitNot
            | Opcode::LoadPropertyIC
            | Opcode::StorePropertyIC
            | Opcode::ShrU
            | Opcode::Eq
            | Opcode::Ne
            | Opcode::Swap
            | Opcode::LoadThis
            | Opcode::BlockEnter
            | Opcode::BlockLeave
            | Opcode::DeclareLet
            | Opcode::DeclareConst
            | Opcode::LoadLexical
            | Opcode::StoreLexical => {}
            _ => return false,
        }
    }
    true
}
