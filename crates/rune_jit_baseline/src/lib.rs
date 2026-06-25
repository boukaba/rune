pub mod assembler;
pub mod codegen;
#[cfg(target_arch = "aarch64")]
pub mod codegen_aarch64;
pub mod ic;
pub mod templates;

pub use codegen::{CodeGen, JitEntryFn};
#[cfg(target_arch = "aarch64")]
pub use codegen_aarch64::{compile_trace, Aarch64CodeGen};

// ---------------------------------------------------------------------------
// Bailout infrastructure
// ---------------------------------------------------------------------------

/// Reason a JIT-compiled function bailed to the interpreter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BailoutReason {
    Overflow = 0,
    NonSmiInput = 1,
    BailOnEntry = 2,
    ShapeMiss = 3,
    Unimplemented = 4,
}

/// One entry per bytecode PC where a bailout can originate.
#[derive(Clone, Copy, Debug)]
pub struct BailoutPoint {
    pub bc_pc: usize,
    pub stack_depth: u32,
    pub reason: BailoutReason,
}

/// Heap-allocated side table, one per JIT-compiled function.
/// Stored as `Box`; owned by `Vm` keyed by entry pointer (see §10.3).
#[derive(Clone, Debug)]
pub struct BailoutTable {
    pub points: Vec<BailoutPoint>,
}

/// Return type from `CodeGen::compile` / `Aarch64CodeGen::compile`.
pub struct CompiledFunction {
    pub mem: ExecutableMemory,
    pub bailout_table: BailoutTable,
}

use assembler::ExecutableMemory;

/// Check if an f64 value fits in Smi range (i31).
fn float64_is_smi_compatible(val: f64) -> bool {
    let is_int = val.fract() == 0.0 && val.is_finite();
    is_int && {
        let i = val as i64;
        i >= -(1 << 30) as i64 && i < (1 << 30) as i64
    }
}

/// Check if a BytecodeProgram only uses opcodes the JIT can currently handle.
pub fn is_jit_compatible(prog: &rune_bytecode::opcode::BytecodeProgram) -> bool {
    use rune_bytecode::opcode::Opcode;
    for instr in &prog.instructions {
        match instr.opcode {
            Opcode::LoadFloat64 => {
                let idx = instr.operands[0] as usize;
                let val = prog.float_pool.get(idx).copied().unwrap_or(f64::NAN);
                if !float64_is_smi_compatible(val) {
                    return false;
                }
            }
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
            | Opcode::StoreLexical
            | Opcode::TypeOf
            | Opcode::MakeArgumentsArray => {}
            _ => return false,
        }
    }
    true
}
