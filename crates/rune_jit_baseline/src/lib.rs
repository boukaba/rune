pub mod assembler;
pub mod codegen;
#[cfg(target_arch = "aarch64")]
pub mod codegen_aarch64;
pub mod ic;
pub use ic::{InlineEntry, InlinePlan, InlineProfile, TraceIcEntry, TraceIcTable};
pub mod templates;

pub use codegen::{CodeGen, JitEntryFn};
#[cfg(target_arch = "aarch64")]
pub use codegen_aarch64::Aarch64CodeGen;

// ---------------------------------------------------------------------------
// JIT value-stack layout (convention v2-lite)
//
// These are Vm-layout facts shared by every build (the interpreter references
// some of them even where the aarch64 codegen is absent), so they live here
// rather than behind `target_arch = "aarch64"`.
// ---------------------------------------------------------------------------

/// Number of u64 slots in the JIT value-stack area at VM offset 0.
///
/// Convention v2 (#4e-lite): the stack is divided into per-frame REGIONS.
/// Every native entry (function tier-up, call-IC hit, trace, or nested
/// `rune_jit_call_helper` dispatch) reserves one region by bumping
/// `jit_stack_cursor`; the frame's base lands ABOVE every ancestor's live
/// slots, so a callee can never clobber caller operands (`f(x)+g(y)` class)
/// and bailout snapshots are FRAME-relative (`fb..sp`) — matching the
/// compile-time model exactly.
pub const JIT_STACK_SIZE: usize = 2048;
/// Slots reserved per native frame region. Bounds native-call nesting:
/// deeper chains overflow-guard to a BailOnEntry (interpreter takes over).
///
/// KNOWN LIMITATION (Stability#5 follow-up): larger budgets (128/256) expose
/// a corruption in the overflow/resume dance — fib(19+) returns NaN because
/// kept-frame zombies from earlier aborts get resumed by nested run_loops
/// with stale operand expectations. Budget 8 keeps fib-class recursion fully
/// native (live depth ≤ ~5 slots fits the 64-byte region; claims are LIFO so
/// unused region slack is harmless) and matches shipped v0.9.3 behavior.
/// Root-causing the dance is the documented NEXT TARGET.
pub const JIT_FRAME_BUDGET: usize = 8;

/// Byte offset of the helper fn-pointer table (after the value-stack area).
pub const JIT_HELPERS_OFFSET: u32 = (JIT_STACK_SIZE * 8) as u32; // 16384
/// Byte offset of jit_stack_base (global stack start; kept for diagnostics).
pub const JIT_STACK_BASE_OFFSET: u32 = JIT_HELPERS_OFFSET + 64; // 16448
/// Byte offset of the monotonic region cursor (byte offset from vm base).
pub const JIT_CURSOR_OFFSET: u32 = JIT_STACK_BASE_OFFSET + 8; // 16456
/// Byte offset of the pending-bailout flag (was jit_stack[63] @ 504).
pub const JIT_FLAG_OFFSET: u32 = JIT_CURSOR_OFFSET + 8; // 16464

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

/// Check if a BytecodeProgram only uses opcodes the JIT can currently handle.
pub fn is_jit_compatible(prog: &rune_bytecode::opcode::BytecodeProgram) -> bool {
    use rune_bytecode::opcode::Opcode;
    for instr in &prog.instructions {
        match instr.opcode {
            // J2#4: any f64 literal is eligible — LoadFloat64 codegen emits
            // mov_imm64 of its NaN-boxed bits directly.
            Opcode::LoadSmi
            | Opcode::LoadFloat64
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
            | Opcode::LoadProperty
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
            | Opcode::CopyLexical
            | Opcode::MakeEnv
            | Opcode::RestoreEnv
            | Opcode::LoadCaptured
            | Opcode::StoreCaptured
            | Opcode::TypeOf
            | Opcode::LoadStringConst
            | Opcode::MakeArgumentsArray
            | Opcode::LoadGlobal
            | Opcode::StoreGlobal
            | Opcode::IncGlobal
            | Opcode::DecGlobal
            | Opcode::Call
            | Opcode::Mod
            | Opcode::Div
            | Opcode::Exp
            | Opcode::JumpIfNullOrUndefined
            | Opcode::In
            | Opcode::Instanceof => {}
            _ => return false,
        }
    }
    true
}
