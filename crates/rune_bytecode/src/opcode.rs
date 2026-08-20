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
    Dup2,
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
    LoadPropertyIC,  // shape-guarded fast path (after N hits)
    StorePropertyIC, // shape-guarded store fast path (after N hits)
    StoreProperty,
    DeleteProperty,
    DefineProperty,
    DefineAccessor,
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
    JumpIfNullOrUndefined,
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
    // for-of (iteration protocol)
    ForOfInit,           // pop iterable → push [iterator, nextMethod]
    ForOfNext,           // operands[0] = end target; call next → done ? jump end : push value
    ToArrayFromIterable, // pop value → push array (via @@iterator)
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
    // Async
    Await,
    // RegExp
    LoadRegExp,
    // Class extends
    SetSuperclass,  // pop func, pop superclass → store superclass in func's struct
    LoadSuperclass, // push current func's superclass onto stack
    // Private field/method
    PrivateNameScope, // operands[0] = number of private names; creates PrivateEnvironment
    LoadPrivateProperty, // pop obj, private slot index → PrivateGet
    StorePrivateProperty, // pop value, obj, private slot index → PrivateSet
    DefinePrivateField, // pop value, obj, private slot index → PrivateFieldAdd
    MakeAccessorPair, // pop setter, pop getter → push AccessorPair (private accessors)
    // ESM modules
    ImportModule, // operands[0] = index into program.module.imports; evaluates the
    // dependency (DFS, cycle-safe) and seeds namespace-import locals
    LoadModuleImport, // operands[0] = index into program.module.imports; push the
    // dependency's live binding value (undefined if absent)
    StoreModuleImport, // pop value; write into the dependency's module environment;
    // push value back
    ExportSync, // pop value; store into the current module environment under the
    // string-pool name operands[0]; push value back
    ModuleTdz, // operands[0] = string-pool name; mark the module binding as
               // uninitialized (TDZ sentinel) — reads throw ReferenceError until
               // the initializer runs (§9.2.2.2 InitializeBinding / TDZ reads)
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
    pub is_async: bool,
    pub local_names: Vec<String>,
    /// Number of slots in this function's lexical environment object (0 = no env).
    /// Set by the emitter when escape analysis detects that variables in this
    /// function are captured by nested closures.
    pub captured_env_size: usize,
    /// Pre-compiled regex patterns and flags for LoadRegExp.
    pub regex_pool: Vec<(String, String)>,
    /// True when this program is a module top-level program (not a function).
    /// Module programs must not seed locals from / sync back to the globals map.
    pub is_module: bool,
    /// ESM linkage metadata; `None` for scripts and function programs.
    pub module: Option<ModuleInfo>,
}

/// Linkage metadata for an ESM module program.
///
/// `local_exports` maps exported names to the module's OWN local binding name
/// (e.g. `export const x = 1` → `("x", "x")`, `export {a as b}` → `("b", "a")`,
/// `export default expr` → `("default", "*default*")`). `indirect_exports`
/// maps exported names to `(specifier, imported_name)` for `export {a} from`,
/// and `star_exports` holds the specifiers of `export * from` clauses.
/// `namespace_exports` holds `(namespace_name, specifier)` for
/// `export * as ns from`. A namespace object built from a module merges its
/// local exports, indirect exports, and (minus conflicts) star exports.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ModuleInfo {
    pub imports: Vec<ModuleImport>,
    pub local_exports: Vec<(String, String)>,
    pub indirect_exports: Vec<(String, String, String)>,
    pub star_exports: Vec<String>,
    pub namespace_exports: Vec<(String, String)>,
}

/// One import entry. `imported` is the exported name ("*default*" for default
/// imports, "*ns*" for namespace imports); `local` is the importing module's
/// local binding name ("" for namespace imports, whose local is seeded by the
/// VM at the ImportModule site).
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ModuleImport {
    pub specifier: String,
    pub imported: String,
    pub local: String,
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
            is_async: false,
            local_names: vec![],
            captured_env_size: 0,
            regex_pool: vec![],
            is_module: false,
            module: None,
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

    /// Intern a regex into the pool and return its index.
    pub fn intern_regex(&mut self, pattern: &str, flags: &str) -> usize {
        let idx = self.regex_pool.len();
        self.regex_pool
            .push((pattern.to_string(), flags.to_string()));
        idx
    }

    /// Run liveness analysis on this program.
    pub fn liveness(&self) -> crate::analysis::LivenessInfo {
        let cfg = self.build_cfg();
        crate::analysis::liveness(&cfg, &self.instructions, self.local_names.len())
    }

    /// Returns true if this function needs a Frame for lexical-scope access
    /// (BlockEnter/Leave, DeclareLet/Const, LoadLexical/StoreLexical, LoadThis,
    /// and closure-env ops CopyLexical/MakeEnv/RestoreEnv/LoadCaptured/
    /// StoreCaptured).
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
                    | Opcode::CopyLexical
                    | Opcode::MakeEnv
                    | Opcode::RestoreEnv
                    | Opcode::LoadCaptured
                    | Opcode::StoreCaptured
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
