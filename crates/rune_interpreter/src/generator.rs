use rune_bytecode::opcode::BytecodeProgram;
use rune_core::value::Value;

/// Saved state of a suspended generator frame.
///
/// Lives on the Rust heap (not GC-managed). Its `locals` and `lexical_slots` Vecs
/// are registered as GC roots while the generator is suspended so that copying
/// collection updates the stored Values.
pub struct Generator {
    pub locals: Vec<Value>,
    pub lexical_slots: Vec<Value>,
    pub lexical_tdz: Vec<bool>,
    pub lexical_const: Vec<bool>,
    pub scope_boundaries: Vec<usize>,
    pub pc: usize,
    pub prog: *const BytecodeProgram,
    /// Whether the generator has been started (Yield has been hit at least once).
    pub started: bool,
    /// Whether the generator has completed (returned or finished).
    pub done: bool,
}

impl Generator {
    pub fn new(locals: Vec<Value>, prog: *const BytecodeProgram) -> Self {
        Generator {
            locals,
            lexical_slots: Vec::new(),
            lexical_tdz: Vec::new(),
            lexical_const: Vec::new(),
            scope_boundaries: Vec::new(),
            pc: 0,
            prog,
            started: false,
            done: false,
        }
    }
}
