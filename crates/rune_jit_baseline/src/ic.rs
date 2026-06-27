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
