/// All bytecode opcodes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
    UnaryPlus,
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
    // Comparisons / relational
    Eq,
    Ne,
    StrictEq,
    StrictNe,
    Lt,
    Gt,
    Le,
    Ge,
    In,
    Instanceof,
    // Objects
    NewObject,
    NewArray,
    ArrayPush,
    ArrayExtend,
    ArraySlice,
    SpreadIntoObject,
    LoadProperty,
    LoadPropertyIC, // shape-guarded fast path (after N hits)
    StorePropertyIC, // shape-guarded store fast path (after N hits)
    StoreProperty,
    DeleteProperty,
    DefineProperty,
    // Template literals
    ToString,
    StringConcat,
    // Globals
    LoadGlobal,
    StoreGlobal,
    // Control flow
    Jump,
    JumpIfTrue,
    JumpIfFalse,
    Throw,
    ThrowIfNullish,
    TryBegin,
    TryEnd,
    FinallyDone,
    // Functions
    MakeFunction,
    Call,
    CallFromArray,
    New,
    Return,
    MakeRestArray,
    MakeArgumentsArray,
    CopyLexical,
    // Stack
    Swap,
    // Generators
    Yield,
    YieldStar,
    Resume,
    InitGenerator,
    // Lexical scoping (let/const/TDZ)
    BlockEnter,
    BlockLeave,
    DeclareLet,
    DeclareConst,
    LoadLexical,
    StoreLexical,
    // for-in
    ForInInit,
    ForInNext,
    // Environment (closure capture)
    MakeEnv,
    RestoreEnv,
    LoadCaptured,
    StoreCaptured,
    // Increment / decrement
    IncLocal,
    DecLocal,
    IncGlobal,
    DecGlobal,
}

#[derive(Clone, Debug)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Instruction {
    pub opcode: Opcode,
    pub operands: Vec<i64>,
    /// Optional index into the Vm's IC table for property caching.
    /// -1 means no IC attached; other values index into Vm.ics[].
    pub ic_index: i64,
    /// Optional index into the Vm's call IC table for Call caching.
    /// -1 means no call IC attached; other values index into Vm.call_ics[].
    pub call_ic_index: i64,
}

impl Instruction {
    pub fn new(opcode: Opcode, operands: Vec<i64>) -> Self {
        Instruction {
            opcode,
            operands,
            ic_index: -1,
            call_ic_index: -1,
        }
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
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(
    serialize_bounds(__S: rkyv::ser::Allocator + rkyv::ser::Writer + rkyv::ser::Sharing),
    deserialize_bounds(__D: rkyv::rancor::Fallible<Error: rkyv::rancor::Source>),
    bytecheck(bounds(__C: rkyv::validation::ArchiveContext + rkyv::rancor::Fallible<Error: rkyv::rancor::Source>))
)]
pub struct BytecodeProgram {
    pub instructions: Vec<Instruction>,
    pub string_pool: Vec<String>,
    pub float_pool: Vec<f64>,
    #[rkyv(omit_bounds)]
    pub functions: Vec<BytecodeProgram>,
    pub named_function: bool,
    pub is_generator: bool,
    pub local_names: Vec<String>,
    /// Number of slots in this function's lexical environment object (0 = no env).
    /// Set by the emitter when escape analysis detects that variables in this
    /// function are captured by nested closures.
    pub captured_env_size: usize,
}

impl BytecodeProgram {
    pub fn new(
        instructions: Vec<Instruction>,
        string_pool: Vec<String>,
        functions: Vec<BytecodeProgram>,
    ) -> Self {
        BytecodeProgram {
            instructions,
            string_pool,
            float_pool: vec![],
            functions,
            named_function: false,
            is_generator: false,
            local_names: vec![],
            captured_env_size: 0,
        }
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

    /// Returns true if this function needs a Frame for lexical-scope access
    /// (BlockEnter/Leave, DeclareLet/Const, LoadLexical/StoreLexical, LoadThis).
    /// Most JIT-compiled leaf functions (e.g. add(a,b){return a+b;}) do not.
    pub fn needs_frame(&self) -> bool {
        self.instructions.iter().any(|instr| {
            matches!(
                instr.opcode,
                Opcode::BlockEnter
                    | Opcode::BlockLeave
                    | Opcode::DeclareLet
                    | Opcode::DeclareConst
                    | Opcode::LoadLexical
                    | Opcode::StoreLexical
                    | Opcode::LoadThis
            )
        })
    }

    /// Assign IC indices to all LoadProperty/StoreProperty/Call instructions.
    /// Recursively processes nested function programs.
    pub fn assign_ic_indices(&mut self) {
        let mut ic_count = 0;
        let mut call_ic_count = 0;
        for instr in &mut self.instructions {
            if matches!(instr.opcode, Opcode::LoadProperty | Opcode::StoreProperty) {
                instr.ic_index = ic_count;
                ic_count += 1;
            }
            if matches!(instr.opcode, Opcode::Call) {
                instr.call_ic_index = call_ic_count;
                call_ic_count += 1;
            }
        }
        for func in &mut self.functions {
            func.assign_ic_indices();
        }
    }
}
