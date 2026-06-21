use rune_bytecode::opcode::BytecodeProgram;
use rune_core::value::Value;

/// Saved state of a suspended generator frame.
///
/// Lives on the Rust heap (not GC-managed). Its `locals` Vec is registered
/// as GC roots while the generator is suspended so that copying collection
/// updates the stored Values.
pub struct Generator {
    pub locals: Vec<Value>,
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
            pc: 0,
            prog,
            started: false,
            done: false,
        }
    }
}
