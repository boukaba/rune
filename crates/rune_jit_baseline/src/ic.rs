/// Entry in a trace-embedded inline cache table.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TraceIcEntry {
    pub shape_id: u64,
    /// Byte offset from object start: `32 + raw_offset * 8`.
    pub slot_offset: u64,
}

/// Polymorphic inline cache table embedded in JIT-compiled trace code.
/// Stores up to 16 `(shape_id, slot_offset)` pairs for the scalar scan
/// dispatch. Built in `Vm::compile_trace_native` from the interpreter's
/// `InlineCache`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TraceIcTable {
    pub entries: [TraceIcEntry; 16],
    pub count: usize,
}

impl Default for TraceIcTable {
    fn default() -> Self {
        Self {
            entries: [TraceIcEntry { shape_id: 0, slot_offset: 0 }; 16],
            count: 0,
        }
    }
}

/// Profile data for one call site within a loop trace or function JIT.
/// Collected during trace recording / function JIT compilation.
/// Stored but unused during F-1; consumed by the inlining engine in F-2.
///
/// # GC safety
/// `callee_prog_ptr` points to a `BytecodeProgram` that is `Pin<Box>`'d
/// (not GC-managed). Nested `functions: Vec<BytecodeProgram>` inside it
/// use Rust-heap-allocated buffers, never mutated after construction.
/// Both the program pointer and the func index are stable across GC.
#[derive(Clone, Debug)]
pub struct InlineProfile {
    /// Bytecode PC of the Call instruction.
    pub call_pc: usize,
    /// Number of times this call site has been executed during recording.
    pub hit_count: u64,
    /// Number of times the callee was JIT-compiled at this site.
    pub jit_count: u64,
    /// Index into `callee_prog.functions[]` for the callee's bytecode.
    /// -1 means no user function (e.g. builtin or non-Func callee).
    pub callee_func_idx: i64,
    /// Pointer to the containing `BytecodeProgram` (pinned, not GC-managed).
    /// Used together with `callee_func_idx` to locate the callee's bytecode.
    pub callee_prog_ptr: *const u8,
    /// Callee's JIT entry point, if monomorphic and JIT-compiled.
    pub callee_jit_entry: Option<*const u8>,
    /// Whether the callee needs a Frame (lexical-scope opcodes).
    pub callee_needs_frame: bool,
    /// Size of callee body in bytecode instructions.
    pub callee_bytecode_size: u32,
}
