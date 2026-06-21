/// All bytecode opcodes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Opcode {
    // Literals
    LoadSmi,
    LoadUndefined,
    LoadNull,
    LoadBoolean,
    LoadString,
    LoadStringConst,
    LoadFloat64,
    LoadThis,
    // Locals
    LoadLocal,
    StoreLocal,
    // Stack
    Pop,
    Dup,
    // Unary
    Neg,
    Not,
    BitNot,
    TypeOf,
    Void,
    // Binary
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Exp,
    // Bitwise
    Shl,
    Shr,
    ShrU,
    BitOr,
    BitXor,
    BitAnd,
    // Logical (short-circuit)
    LogicalAnd,
    LogicalOr,
    // Comparisons
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    Lt,
    Gt,
    Le,
    Ge,
    // Objects
    NewObject,
    NewArray,
    LoadProperty,
    StoreProperty,
    DefineProperty,
    // Globals
    LoadGlobal,
    StoreGlobal,
    // Control flow
    Jump,
    JumpIfTrue,
    JumpIfFalse,
    Throw,
    TryBegin,
    TryEnd,
    FinallyDone,
    // Functions
    MakeFunction,
    Call,
    New,
    Return,
    // Generators
    Yield,
    YieldStar,
    Resume,
    InitGenerator,
    // for-in
    ForInInit,
    ForInNext,
}

#[derive(Clone, Debug)]
pub struct Instruction {
    pub opcode: Opcode,
    pub operands: Vec<i64>,
    /// Optional index into the Vm's IC table for LoadProperty/StoreProperty caching.
    /// -1 means no IC attached; other values index into Vm.ics[].
    pub ic_index: i64,
}

impl Instruction {
    pub fn new(opcode: Opcode, operands: Vec<i64>) -> Self {
        Instruction { opcode, operands, ic_index: -1 }
    }
}

/// A complete bytecode program with its constant pool and nested functions.
///
/// ## Multi-entry convention
///
/// Generator functions have two entry points:
/// - `pc = 0` → `InitGenerator` (first-time start)
/// - `pc = 0` on resume → `InitGenerator` is skipped (saved pc = 1 after first yield)
///
/// `InitGenerator` is always the very first instruction of a generator program
/// (inserted by `Emitter::into_bytecode()`). Non-generator programs never
/// contain `InitGenerator` or `Resume`. The `Resume` opcode is a no-op
/// placeholder for future try/catch/finally restore logic — it currently
/// pushes `undefined` onto the stack for the resumption value position.
#[derive(Clone, Debug)]
pub struct BytecodeProgram {
    pub instructions: Vec<Instruction>,
    pub string_pool: Vec<String>,
    pub float_pool: Vec<f64>,
    pub functions: Vec<BytecodeProgram>,
    pub named_function: bool,
    pub is_generator: bool,
    pub local_names: Vec<String>,
}

impl BytecodeProgram {
    pub fn new(
        instructions: Vec<Instruction>,
        string_pool: Vec<String>,
        functions: Vec<BytecodeProgram>,
    ) -> Self {
        BytecodeProgram { instructions, string_pool, float_pool: vec![], functions, named_function: false, is_generator: false, local_names: vec![] }
    }

    /// Intern a string into the pool and return its index.
    pub fn intern(&mut self, s: &str) -> usize {
        if let Some(idx) = self.string_pool.iter().position(|x| x == s) {
            return idx;
        }
        let idx = self.string_pool.len();
        self.string_pool.push(s.to_string());
        idx
    }

    /// Build the control-flow graph from this program's instructions.
    pub fn build_cfg(&self) -> crate::block::ControlFlowGraph {
        crate::block::build_cfg(&self.instructions)
    }

    /// Run liveness analysis on this program.
    pub fn liveness(&self) -> crate::analysis::LivenessInfo {
        let cfg = self.build_cfg();
        crate::analysis::liveness(&cfg, &self.instructions, self.local_names.len())
    }

    /// Assign IC indices to all LoadProperty/StoreProperty instructions.
    /// Recursively processes nested function programs.
    pub fn assign_ic_indices(&mut self) {
        let mut ic_count = 0;
        for instr in &mut self.instructions {
            if matches!(instr.opcode, Opcode::LoadProperty | Opcode::StoreProperty) {
                instr.ic_index = ic_count;
                ic_count += 1;
            }
        }
        for func in &mut self.functions {
            func.assign_ic_indices();
        }
    }
}
