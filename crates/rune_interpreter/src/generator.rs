use crate::vm::{GeneratorResume, TryFrame};
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
    pub this: Value,
    pub env: *mut u8,
    /// Live operand-stack values above the frame base at suspension
    /// (e.g. for-of [iterator, nextMethod] pairs). Restored on resume.
    pub stack: Vec<Value>,
    /// Saved try/catch/finally frames at suspension (restored on resume).
    pub try_frames: Vec<TryFrame>,
    /// Operand-stack base at suspension, for rebasing banked try frames.
    pub stack_base_saved: usize,
    /// Reentrancy guard: true while a resume is on the Rust stack.
    pub executing: bool,
    /// Abrupt completion pending across a suspension: if a Throw/Return
    /// resume suspends again (e.g. yielding inside a `finally` that runs
    /// during unwinding), the abrupt takes precedence over whatever the
    /// next resume requests.
    pub abrupt: Option<GeneratorResume>,
    /// True when suspended at a `yield*` drain point (set by YieldStarYield,
    /// cleared by plain Yield). throw()/return() forward to the delegate.
    pub in_delegate: bool,
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
            this: Value::undefined(),
            env: std::ptr::null_mut(),
            stack: Vec::new(),
            try_frames: Vec::new(),
            stack_base_saved: 0,
            executing: false,
            abrupt: None,
            in_delegate: false,
        }
    }
}
