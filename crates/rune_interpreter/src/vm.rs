use crate::builtins::{Builtin, BuiltinFn, value_to_js_string};
use crate::generator::Generator;
use crate::ic::{IcEntry, IcStats, InlineCache, LoopTrace, TraceOp};
use rune_bytecode::opcode::{BytecodeProgram, Instruction, Opcode};
use rune_core::array::RuneArray;
use rune_core::env::EnvObject;
use rune_core::float::HeapFloat64;
use rune_core::function::Func;
use rune_core::gc::{
    GcHeader, RootProvider, SemiSpace, TAG_ARRAY, TAG_FLOAT64, TAG_FUNC, TAG_OBJECT, TAG_STRING,
};
use rune_core::object::JSObject;
use rune_core::shape::{DENSE_ARRAY_SHAPE, PROTOTYPE_KEY, PropertyKey, Shape};
use rune_core::string::HeapString;
use rune_core::value::Value;
#[cfg(all(feature = "jit", target_arch = "aarch64"))]
use rune_jit_baseline::Aarch64CodeGen;
#[cfg(all(feature = "jit", target_arch = "x86_64"))]
use rune_jit_baseline::CodeGen;
use rune_jit_baseline::JitEntryFn;
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::collections::HashSet;

/// Create a minimal Error object with `name` and `message` properties.
fn make_error_object(gc: &mut SemiSpace, name: &str, msg: &str) -> Value {
    let name_str: *mut u8 = HeapString::allocate(gc, name) as *mut u8;
    let msg_str: *mut u8 = HeapString::allocate(gc, msg) as *mut u8;
    let entries = vec![
        (PropertyKey::from_string("name"), 0usize),
        (PropertyKey::from_string("message"), 1usize),
    ];
    let key_names = vec!["name".to_string(), "message".to_string()];
    let shape = Shape::intern(entries, key_names);
    let obj = JSObject::allocate(
        gc,
        shape,
        &[
            Value::from_heap_ptr(name_str),
            Value::from_heap_ptr(msg_str),
        ],
    );
    Value::from_heap_ptr(obj as *mut u8)
}

/// Callback for the `eval` builtin: parses and executes JS source, returns result.
pub type EvalFn = Box<dyn FnMut(&mut SemiSpace, &str) -> Result<Value, String>>;

struct Frame {
    locals: Vec<Value>,
    /// Lexical (let/const) slots for block-scoped bindings.
    lexical_slots: Vec<Value>,
    /// Parallel TDZ flags: true = binding is in temporal dead zone.
    lexical_tdz: Vec<bool>,
    /// Parallel const flags: true = binding is immutable.
    lexical_const: Vec<bool>,
    /// Stack of scope boundary indices into the lexical arrays.
    scope_boundaries: Vec<usize>,
    /// Number of arguments passed to this function (for `arguments` object).
    passed_argc: usize,
    pc: usize,
    stack_base: usize,
    prog: *const BytecodeProgram,
    generator_id: Option<usize>,
    this: Value,
    is_constructor_call: bool,
    constructed_object: Value,
    /// Pointer to this frame's lexical environment object (may be null).
    /// Set by MakeEnv at function entry. Child closures capture this pointer.
    env: *mut u8,
}

/// Result of the bytecode loop: normal return, generator yield, or throw.
enum Exit {
    Return(Value),
    Yield(Value),
    Throw(Value),
}

/// Tracks a try-catch-finally block for exception unwinding.
#[derive(Copy, Clone)]
struct TryFrame {
    catch_pc: usize,
    finally_pc: usize,
    stack_depth: usize,
    frame_depth: usize,
    saved_exception: Option<Value>,
    in_catch: bool,
}

/// JIT helper function pointers, stored at a fixed offset from vm_ptr
/// (offset 512 = 64 * 8, right after jit_stack) so JIT code can load
/// and call them without cross-crate symbol resolution.
/// JIT bailout state, written by the bailout helper, read by vm.rs call site.
#[derive(Clone, Debug)]
pub struct JitBailoutState {
    /// Bytecode PC where bailout occurred.
    pub bc_pc: usize,
    /// Set by bailout helper to signal a bailout. Checked by call site
    /// instead of `bc_pc != 0` because MakeArgumentsArray at PC 0 would
    /// collide with the "no bailout" sentinel.
    pub pending: bool,
    /// Snapshot of the JIT value stack at bailout.
    pub stack_snapshot: Vec<u64>,
    /// Reason tag (for stats/debugging).
    pub reason: rune_jit_baseline::BailoutReason,
}

impl Default for JitBailoutState {
    fn default() -> Self {
        Self {
            bc_pc: 0,
            pending: false,
            stack_snapshot: Vec::new(),
            reason: rune_jit_baseline::BailoutReason::BailOnEntry,
        }
    }
}

#[repr(C)]
pub struct JitHelpers {
    pub lexical_helper: usize,
    pub bailout_helper: usize,
    pub typeof_helper: usize,
    pub string_helper: usize,
    pub global_helper: usize,
    _reserved: [usize; 3],
}

/// Stack-based bytecode interpreter with call frame support.
#[repr(C)]
pub struct Vm {
    /// JIT-compiled trace value stack. Must remain the first field so that
    /// emitted AArch64 trace code can address it at offset 0 from the VM
    /// pointer (x19). Using heap memory for the JIT stack avoids macOS
    /// Apple Silicon restrictions on writes through the real stack pointer
    /// from JIT pages.
    pub jit_stack: [u64; 64],
    /// JIT helper function pointer table. Must follow jit_stack immediately
    /// for the JIT to locate it at a known offset (512) from vm_ptr.
    pub jit_helpers: JitHelpers,
    /// JIT stack base pointer, written by the JIT prologue.
    /// On AArch64: points to the base of jit_stack[] (== vm_ptr).
    /// On x86-64: points to the allocated native stack area (== initial rbx).
    pub jit_stack_base: u64,
    /// Bailout state, set by bailout helper during JIT execution.
    pub jit_bailout: JitBailoutState,
    /// Owned bailout tables, keyed by JIT entry pointer (see §10.3).
    pub bailout_tables: std::collections::HashMap<usize, Box<rune_jit_baseline::BailoutTable>>,
    pub stack: Vec<Value>,
    frames: Vec<Frame>,
    try_stack: Vec<TryFrame>,
    pub generators: Vec<Generator>,
    pub builtins: Vec<Builtin>,
    pub globals: HashMap<String, Value>,
    /// Shape-Indexed Dispatch Tables for property access caching.
    pub ics: Vec<InlineCache>,
    /// Cached IC entries for bytecode-specialized callsites (LoadPropertyIC).
    pub ic_entries: Vec<IcEntry>,
    /// Per-callsite hit counters for bytecode patching threshold.
    ic_hit_counts: Vec<u32>,
    /// Aggregate IC statistics.
    pub ic_stats: IcStats,
    /// Cache of allocated HeapString pointers for each program's constant pool.
    /// Key: program pointer as usize, Value: Vec of string Value handles.
    string_cache: HashMap<usize, Vec<Value>>,
    /// Loop back-edge hotness: target_pc → execution count.
    /// Back-edges are Jump targets where target < current_pc.
    loop_counts: HashMap<usize, u64>,
    /// Recorded traces for hot loops (target_pc → LoopTrace).
    loop_traces: HashMap<usize, LoopTrace>,
    /// If Some(target_pc), we're currently recording a trace for that loop.
    recording_trace: Option<usize>,
    /// Whether the current hot loop has already been patched.
    loop_patched: HashSet<usize>,
    /// Executable memory for compiled loop traces. Kept alive so entry points
    /// remain valid.
    _compiled_trace_mem: Vec<rune_jit_baseline::assembler::ExecutableMemory>,
    /// Pre-built constructor objects (like `Object`) that expose methods via property access.
    builtin_wrappers: HashMap<String, Value>,
    /// AFPC: cached JIT entry points by function index. When a cache is loaded,
    /// native code blobs are mmap'd and their addresses stored here; MakeFunction
    /// installs them on the newly-created Func objects.
    pub cached_jit_entries: HashMap<usize, *const u8>,
    /// Number of times the JIT entry path was taken (including bailout).
    /// Used by tests to verify JIT actually executed.
    pub jit_entry_count: u64,
    /// Number of JIT bailouts (all reasons). Helps detect wasteful JIT entries
    /// where a function always bails (e.g., always receives non-Smi args).
    pub jit_bailout_count: u64,
    /// Pre-allocated string Values for typeof results (indexed by TYPEOF_* constants).
    pub typeof_strings: [Value; 6],
    last_locals: Vec<Value>,
    pub eval_fn: UnsafeCell<Option<EvalFn>>,
    /// Reference to Array.prototype for setting on newly created arrays.
    pub array_prototype: Value,
    /// Reference to String.prototype for string property access.
    pub string_prototype: Value,
    /// Reference to Object.prototype for setting on newly created objects.
    pub object_prototype: Value,
    /// Pending exception set by a builtin (checked after builtin dispatch).
    pub pending_exception: Option<Value>,
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

impl Vm {
    pub fn new() -> Self {
        Vm {
            jit_stack: [0; 64],
            jit_helpers: JitHelpers {
                lexical_helper: rune_jit_lexical_helper as *const () as usize,
                bailout_helper: rune_jit_bailout_helper as *const () as usize,
                typeof_helper: rune_jit_typeof_helper as *const () as usize,
                string_helper: rune_jit_string_helper as *const () as usize,
                global_helper: rune_jit_global_helper as *const () as usize,
                _reserved: [0; 3],
            },
            jit_stack_base: 0,
            jit_bailout: JitBailoutState::default(),
            bailout_tables: std::collections::HashMap::new(),
            stack: Vec::new(),
            frames: Vec::new(),
            try_stack: Vec::new(),
            generators: Vec::new(),
            builtins: Vec::new(),
            globals: HashMap::new(),
            ics: Vec::new(),
            ic_entries: Vec::new(),
            ic_hit_counts: Vec::new(),
            ic_stats: IcStats::default(),
            string_cache: HashMap::new(),
            loop_counts: HashMap::new(),
            loop_traces: HashMap::new(),
            recording_trace: None,
            loop_patched: HashSet::new(),
            _compiled_trace_mem: Vec::new(),
            builtin_wrappers: HashMap::new(),
            cached_jit_entries: HashMap::new(),
            jit_entry_count: 0,
            jit_bailout_count: 0,
            typeof_strings: [Value::undefined(); 6],
            last_locals: Vec::new(),
            eval_fn: UnsafeCell::new(None),
            array_prototype: Value::undefined(),
            string_prototype: Value::undefined(),
            object_prototype: Value::undefined(),
            pending_exception: None,
        }
    }

    /// Build pre-wired constructor objects (Object, etc.) in the GC heap.
    /// Must be called after all builtins are registered.
    pub fn init_builtin_wrappers(&mut self, gc: &mut SemiSpace) {
        fn find_handle(builtins: &[Builtin], name: &str) -> Option<Value> {
            builtins
                .iter()
                .position(|b| b.name == name)
                .map(|id| Value::smi(-(id as i32) - 1))
        }
        fn make_object(gc: &mut SemiSpace, pairs: &[(&str, Value)]) -> Value {
            let keys: Vec<(PropertyKey, usize)> = pairs
                .iter()
                .enumerate()
                .map(|(i, (k, _))| (PropertyKey::from_string(k), i))
                .collect();
            let key_names: Vec<String> = pairs.iter().map(|(k, _)| k.to_string()).collect();
            let shape = Shape::intern(keys, key_names);
            let vals: Vec<Value> = pairs.iter().map(|(_, v)| *v).collect();
            let obj_ptr = JSObject::allocate(gc, shape, &vals);
            Value::from_heap_ptr(obj_ptr as *mut u8)
        }

        // Object constructor with .create() method
        if let Some(handle) = find_handle(&self.builtins, "Object_create") {
            let obj_val = make_object(gc, &[("create", handle)]);
            self.builtin_wrappers.insert("Object".to_string(), obj_val);
        }

        // Array.prototype with push/pop methods
        let push_handle = find_handle(&self.builtins, "Array_prototype_push");
        let pop_handle = find_handle(&self.builtins, "Array_prototype_pop");
        if let (Some(push), Some(pop)) = (push_handle, pop_handle) {
            let arr_proto = make_object(gc, &[("push", push), ("pop", pop)]);
            self.builtin_wrappers
                .insert("Array.prototype".to_string(), arr_proto);
            self.array_prototype = arr_proto;
        }

        // String.prototype with charAt/slice methods
        let char_at_handle = find_handle(&self.builtins, "String_prototype_charAt");
        let slice_handle = find_handle(&self.builtins, "String_prototype_slice");
        if let (Some(char_at), Some(slice)) = (char_at_handle, slice_handle) {
            let str_proto = make_object(gc, &[("charAt", char_at), ("slice", slice)]);
            self.builtin_wrappers
                .insert("String.prototype".to_string(), str_proto);
            self.string_prototype = str_proto;
        }

        // Array constructor with .isArray()
        if let Some(handle) = find_handle(&self.builtins, "Array_isArray") {
            let arr_ctor = make_object(gc, &[("isArray", handle)]);
            self.builtin_wrappers.insert("Array".to_string(), arr_ctor);
        }

        // String constructor with .fromCharCode()
        if let Some(handle) = find_handle(&self.builtins, "String_fromCharCode") {
            let str_ctor = make_object(gc, &[("fromCharCode", handle)]);
            self.builtin_wrappers.insert("String".to_string(), str_ctor);
        }

        // Math namespace with all methods + constants
        let pi_val = {
            let ptr = HeapFloat64::allocate(gc, std::f64::consts::PI);
            Value::from_float64_ptr(ptr as *mut u8)
        };
        let e_val = {
            let ptr = HeapFloat64::allocate(gc, std::f64::consts::E);
            Value::from_float64_ptr(ptr as *mut u8)
        };
        let math_entries: Vec<(&str, Value)> = [
            ("floor", find_handle(&self.builtins, "Math_floor")),
            ("ceil", find_handle(&self.builtins, "Math_ceil")),
            ("abs", find_handle(&self.builtins, "Math_abs")),
            ("min", find_handle(&self.builtins, "Math_min")),
            ("max", find_handle(&self.builtins, "Math_max")),
            ("pow", find_handle(&self.builtins, "Math_pow")),
            ("sqrt", find_handle(&self.builtins, "Math_sqrt")),
            ("PI", Some(pi_val)),
            ("E", Some(e_val)),
        ]
        .iter()
        .filter_map(|(name, val)| val.map(|v| (*name, v)))
        .collect();
        if !math_entries.is_empty() {
            let math_obj = make_object(gc, &math_entries);
            self.builtin_wrappers.insert("Math".to_string(), math_obj);
        }

        // Object.prototype — an empty object that serves as default [[Prototype]]
        let obj_proto_shape = Shape::empty();
        let obj_proto_ptr = JSObject::allocate(gc, obj_proto_shape, &[]);
        self.object_prototype = Value::from_heap_ptr(obj_proto_ptr as *mut u8);

        // Global constants: NaN, Infinity, undefined
        let nan_val = {
            let ptr = HeapFloat64::allocate(gc, f64::NAN);
            Value::from_float64_ptr(ptr as *mut u8)
        };
        self.globals.insert("NaN".to_string(), nan_val);
        let inf_val = {
            let ptr = HeapFloat64::allocate(gc, f64::INFINITY);
            Value::from_float64_ptr(ptr as *mut u8)
        };
        self.globals.insert("Infinity".to_string(), inf_val);
        self.globals
            .insert("undefined".to_string(), Value::undefined());

        // assert wrapper object for Test262: assert.sameValue, assert.notSameValue, assert.throws
        let assert_same = find_handle(&self.builtins, "assert_sameValue");
        let assert_not_same = find_handle(&self.builtins, "assert_notSameValue");
        let assert_throws = find_handle(&self.builtins, "assert_throws");
        if let (Some(same), Some(not_same), Some(th)) =
            (assert_same, assert_not_same, assert_throws)
        {
            let assert_obj = make_object(
                gc,
                &[
                    ("sameValue", same),
                    ("notSameValue", not_same),
                    ("throws", th),
                ],
            );
            self.builtin_wrappers
                .insert("assert".to_string(), assert_obj);
        }
    }

    /// Register a built-in function and return its handle (negative Smi).
    pub fn register_builtin(&mut self, name: &'static str, func: BuiltinFn) -> Value {
        let id = self.builtins.len();
        self.builtins.push(Builtin { name, func });
        Value::smi(-(id as i32) - 1)
    }

    /// Look up a builtin handle by name.
    pub fn get_builtin(&self, name: &str) -> Option<Value> {
        self.builtins
            .iter()
            .position(|b| b.name == name)
            .map(|id| Value::smi(-(id as i32) - 1))
    }

    /// Check if all values in the slice are Smi (tag bit 0 = 1).
    #[allow(dead_code)]
    fn all_smi(values: &[Value]) -> bool {
        values.iter().all(|v| v.is_smi())
    }

    /// Set a pending exception (used by builtins that cannot return Exit).
    pub fn set_pending_exception(&mut self, val: Value) {
        self.pending_exception = Some(val);
    }

    /// Throw a ReferenceError from the run loop.
    fn throw_reference_error(&mut self, gc: &mut SemiSpace, msg: &str) -> Exit {
        let full_msg = format!("ReferenceError: {}", msg);
        let ptr = HeapString::allocate(gc, &full_msg);
        self.push(Value::from_heap_ptr(ptr as *mut u8));
        Exit::Throw(self.pop())
    }

    /// Throw a TypeError from the run loop.
    fn throw_type_error(&mut self, gc: &mut SemiSpace, msg: &str) -> Exit {
        let full_msg = format!("TypeError: {}", msg);
        let ptr = HeapString::allocate(gc, &full_msg);
        self.push(Value::from_heap_ptr(ptr as *mut u8));
        Exit::Throw(self.pop())
    }

    /// Register all GC root slots (stack, locals, try_stack saved values).
    /// Must be called after any change to stack/frames/try_stack before GC can run.
    pub fn register_roots(&mut self, gc: &mut SemiSpace) {
        gc.clear_roots();
        for val in &self.stack {
            gc.push_root(val as *const Value as *mut u64);
        }
        for frame in &self.frames {
            for local in &frame.locals {
                gc.push_root(local as *const Value as *mut u64);
            }
            for slot in &frame.lexical_slots {
                gc.push_root(slot as *const Value as *mut u64);
            }
            // Root the frame's captured environment pointer (a valid GC heap pointer)
            if !frame.env.is_null() {
                gc.push_root(&frame.env as *const *mut u8 as *mut u64);
            }
        }
        for tf in &self.try_stack {
            if let Some(ref val) = tf.saved_exception {
                gc.push_root(val as *const Value as *mut u64);
            }
        }
        for val in &self.last_locals {
            gc.push_root(val as *const Value as *mut u64);
        }
        for g in &self.generators {
            for local in &g.locals {
                gc.push_root(local as *const Value as *mut u64);
            }
            for slot in &g.lexical_slots {
                gc.push_root(slot as *const Value as *mut u64);
            }
        }
        // Root builtin prototype objects that are stored as Vm fields
        // (these are not on the stack but are used after GC cycles)
        gc.push_root(&self.object_prototype as *const Value as *mut u64);
        gc.push_root(&self.array_prototype as *const Value as *mut u64);
        gc.push_root(&self.string_prototype as *const Value as *mut u64);
        // Root pre-allocated typeof result strings (JIT typeof_helper reads these)
        for v in &self.typeof_strings {
            gc.push_root(v as *const Value as *mut u64);
        }
        // Root cached string constant handles (LoadStringConst cache)
        for handles in self.string_cache.values() {
            for v in handles {
                gc.push_root(v as *const Value as *mut u64);
            }
        }
    }

    /// Execute a bytecode program and return its result.
    pub fn execute(
        &mut self,
        gc: &mut SemiSpace,
        program: &BytecodeProgram,
    ) -> Result<Value, Value> {
        self.frames.clear();
        self.stack.clear();
        self.try_stack.clear();

        // Initialize top-level locals from persisted globals
        let locals: Vec<Value> = program
            .local_names
            .iter()
            .map(|name| {
                self.globals
                    .get(name)
                    .copied()
                    .unwrap_or(Value::undefined())
            })
            .collect();

        self.frames.push(Frame {
            locals,
            lexical_slots: Vec::new(),
            lexical_tdz: Vec::new(),
            lexical_const: Vec::new(),
            scope_boundaries: Vec::new(),
            passed_argc: 0,
            pc: 0,
            stack_base: 0,
            prog: program as *const BytecodeProgram,
            generator_id: None,
            this: Value::undefined(),
            is_constructor_call: false,
            constructed_object: Value::undefined(),
            env: std::ptr::null_mut(),
        });

        self.register_roots(gc);

        // Enable automatic root refresh before each GC cycle
        gc.root_provider = Some(self as *mut dyn RootProvider);

        let result = match self.run_loop(gc) {
            Exit::Return(v) => Ok(v),
            Exit::Yield(_) => Ok(Value::undefined()),
            Exit::Throw(v) => Err(v),
        };

        // Disable root provider until next execute
        gc.root_provider = None;

        // Sync locals back to globals for persistence
        for (i, name) in program.local_names.iter().enumerate() {
            if i < self.last_locals.len() {
                self.globals.insert(name.clone(), self.last_locals[i]);
            }
        }

        result
    }

    /// Resume a suspended generator with `arg` as the yield result value.
    /// Returns the next yielded (or returned) value.
    pub fn resume_generator(
        &mut self,
        gc: &mut SemiSpace,
        gen_id: usize,
        arg: Value,
    ) -> Result<Value, Value> {
        if self.generators[gen_id].done {
            return Ok(Value::undefined());
        }
        self.try_stack.clear();

        let (
            locals,
            lexical_slots,
            lexical_tdz,
            lexical_const,
            scope_boundaries,
            pc,
            prog,
            started,
        ) = {
            let g = &self.generators[gen_id];
            (
                g.locals.clone(),
                g.lexical_slots.clone(),
                g.lexical_tdz.clone(),
                g.lexical_const.clone(),
                g.scope_boundaries.clone(),
                g.pc,
                g.prog,
                g.started,
            )
        };

        self.frames.push(Frame {
            locals,
            lexical_slots,
            lexical_tdz,
            lexical_const,
            scope_boundaries,
            passed_argc: 0,
            pc,
            stack_base: self.stack.len(),
            prog,
            generator_id: Some(gen_id),
            this: Value::undefined(),
            is_constructor_call: false,
            constructed_object: Value::undefined(),
            env: std::ptr::null_mut(),
        });

        if started {
            self.push(arg);
        }
        self.generators[gen_id].started = true;

        match self.run_loop(gc) {
            Exit::Return(v) => Ok(v),
            Exit::Yield(v) => Ok(v),
            Exit::Throw(v) => Err(v),
        }
    }

    fn run_loop(&mut self, gc: &mut SemiSpace) -> Exit {
        loop {
            let fi = self.frames.len() - 1;
            let pc = self.frames[fi].pc;
            let prog_ptr = self.frames[fi].prog;
            let prog = unsafe { &*prog_ptr };

            if pc >= prog.instructions.len() {
                break;
            }

            let instr = prog.instructions[pc].clone();

            // Trace recording: capture opcodes while recording a hot loop
            if let Some(target_pc) = self.recording_trace
                && let Some(trace) = self.loop_traces.get_mut(&target_pc)
            {
                if trace.ops.len() < 200 {
                    trace.ops.push(TraceOp {
                        opcode: instr.opcode as u8,
                        operands: instr.operands.clone(),
                        shape_id: 0,
                        cost: 1,
                    });
                }
                // Stop recording when we've looped back to the target
                if pc == target_pc && trace.ops.len() > 1 {
                    self.recording_trace = None;
                    #[cfg(target_arch = "aarch64")]
                    self.compile_trace_native(target_pc);
                }
            }

            match instr.opcode {
                // ---- Literals ----
                Opcode::LoadSmi => {
                    let val = instr.operands[0] as i32;
                    self.push(Value::smi(val));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadUndefined => {
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadNull => {
                    self.push(Value::null());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadBoolean => {
                    let val = instr.operands[0] != 0;
                    self.push(Value::boolean(val));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadString => {
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadStringConst => {
                    let idx = instr.operands[0] as usize;
                    let cache_key = prog_ptr as usize;
                    // Look up or allocate cached string handle
                    let val = if let Some(handles) = self.string_cache.get_mut(&cache_key) {
                        if let Some(v) = handles.get(idx) {
                            if v.is_undefined() {
                                let s = prog.string_pool.get(idx).map(|s| s.as_str()).unwrap_or("");
                                let ptr = HeapString::allocate(gc, s);
                                let new_val = Value::from_heap_ptr(ptr as *mut u8);
                                handles[idx] = new_val;
                                new_val
                            } else {
                                *v
                            }
                        } else {
                            let s = prog.string_pool.get(idx).map(|s| s.as_str()).unwrap_or("");
                            let ptr = HeapString::allocate(gc, s);
                            Value::from_heap_ptr(ptr as *mut u8)
                        }
                    } else {
                        let mut handles = vec![Value::undefined(); prog.string_pool.len()];
                        let s = prog.string_pool.get(idx).map(|s| s.as_str()).unwrap_or("");
                        let ptr = HeapString::allocate(gc, s);
                        let new_val = Value::from_heap_ptr(ptr as *mut u8);
                        handles[idx] = new_val;
                        self.string_cache.insert(cache_key, handles);
                        new_val
                    };
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadFloat64 => {
                    let idx = instr.operands[0] as usize;
                    let val = prog.float_pool.get(idx).copied().unwrap_or(0.0);
                    let is_int = val.fract() == 0.0 && val.is_finite();
                    if is_int {
                        let i = val as i64;
                        if i >= -(1 << 30) as i64 && i < (1 << 30) as i64 {
                            self.push(Value::smi(val as i32));
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }
                    let ptr = HeapFloat64::allocate(gc, val);
                    self.push(Value::from_float64_ptr(ptr as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }

                // ---- `this` binding ----
                Opcode::LoadThis => {
                    self.push(self.frames[fi].this);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Locals ----
                Opcode::LoadLocal => {
                    let idx = instr.operands[0] as usize;
                    let val = if idx < self.frames[fi].locals.len() {
                        self.frames[fi].locals[idx]
                    } else {
                        Value::undefined()
                    };
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StoreLocal => {
                    let idx = instr.operands[0] as usize;
                    let val = self.pop();
                    if idx >= self.frames[fi].locals.len() {
                        self.frames[fi].locals.resize(idx + 1, Value::undefined());
                    }
                    self.frames[fi].locals[idx] = val;
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Stack ----
                Opcode::Pop => {
                    // Only pop if the stack is above this frame's base, so we
                    // don't steal an item belonging to a parent frame (this
                    // matters after StoreCaptured already consumed the value).
                    let stack_base = self.frames[fi].stack_base;
                    if self.stack.len() > stack_base {
                        self.stack.pop();
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Dup => {
                    let val = self.peek();
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Swap => {
                    let a = self.pop();
                    let b = self.pop();
                    self.push(a);
                    self.push(b);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Unary ----
                Opcode::UnaryPlus => {
                    let a = self.pop();
                    // §13.5.3: Return ToNumber(UnaryExpression)
                    let n = to_number(a);
                    let result = number_result(gc, n);
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Neg => {
                    let a = self.pop();
                    let result = if let Some(v) = a.as_smi() {
                        if v == 0 {
                            // Preserve -0.0 per spec (§13.5.5)
                            let ptr = HeapFloat64::allocate(gc, -0.0f64);
                            Value::from_float64_ptr(ptr as *mut u8)
                        } else if v == -(1 << 30) {
                            // Overflow: -(-2^30) = 2^30 doesn't fit in Smi
                            let ptr = HeapFloat64::allocate(gc, -(v as f64));
                            Value::from_float64_ptr(ptr as *mut u8)
                        } else {
                            Value::smi(-v)
                        }
                    } else if let Some(v) = a.as_float64() {
                        let ptr = HeapFloat64::allocate(gc, -v);
                        Value::from_float64_ptr(ptr as *mut u8)
                    } else {
                        let n = to_number(a);
                        let ptr = HeapFloat64::allocate(gc, -n);
                        Value::from_float64_ptr(ptr as *mut u8)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Not => {
                    let a = self.pop();
                    self.push(if a.to_bool() {
                        Value::boolean(false)
                    } else {
                        Value::boolean(true)
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitNot => {
                    let a = self.pop();
                    let n = to_int32(a);
                    // !n always fits in i32; use number_result for i31 safety
                    let result = number_result(gc, (!n) as f64);
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Void => {
                    self.pop();
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Binary ----
                Opcode::Add => {
                    let b = self.pop();
                    let a = self.pop();
                    let a_is_str = value_is_string(a);
                    let b_is_str = value_is_string(b);
                    let result = if a_is_str || b_is_str {
                        let sa = value_to_debug_string(a);
                        let sb = value_to_debug_string(b);
                        let combined = sa + &sb;
                        let ptr = HeapString::allocate(gc, &combined);
                        Value::from_heap_ptr(ptr as *mut u8)
                    } else {
                        let av = to_number(a);
                        let bv = to_number(b);
                        number_result(gc, av + bv)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Sub => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        if let Some(r) = av.checked_sub(bv) {
                            Value::smi(r)
                        } else {
                            number_result(gc, av as f64 - bv as f64)
                        }
                    } else {
                        let av = to_number(a);
                        let bv = to_number(b);
                        number_result(gc, av - bv)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Mul => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        if let Some(r) = av.checked_mul(bv) {
                            Value::smi(r)
                        } else {
                            number_result(gc, av as f64 * bv as f64)
                        }
                    } else {
                        let av = to_number(a);
                        let bv = to_number(b);
                        number_result(gc, av * bv)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Div => {
                    let b = self.pop();
                    let a = self.pop();
                    let av = to_number(a);
                    let bv = to_number(b);
                    let result = number_result(gc, av / bv);
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Mod => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        if bv == 0 {
                            number_result(gc, f64::NAN)
                        } else {
                            Value::smi(av % bv)
                        }
                    } else {
                        let av = to_number(a);
                        let bv = to_number(b);
                        number_result(gc, av % bv)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Exp => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        if bv < 0 {
                            number_result(gc, (av as f64).powf(bv as f64))
                        } else {
                            Value::smi(av.wrapping_pow(bv as u32))
                        }
                    } else {
                        let av = to_number(a);
                        let bv = to_number(b);
                        number_result(gc, av.powf(bv))
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Bitwise ----
                Opcode::Shl => {
                    let b = self.pop();
                    let a = self.pop();
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    self.push(Value::smi(av.wrapping_shl(bv as u32)));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Shr => {
                    let b = self.pop();
                    let a = self.pop();
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    self.push(Value::smi(av.wrapping_shr(bv as u32)));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ShrU => {
                    let b = self.pop();
                    let a = self.pop();
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    self.push(Value::smi((av as u32).wrapping_shr(bv as u32) as i32));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitOr => {
                    let b = self.pop();
                    let a = self.pop();
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    self.push(Value::smi(av | bv));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitXor => {
                    let b = self.pop();
                    let a = self.pop();
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    self.push(Value::smi(av ^ bv));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitAnd => {
                    let b = self.pop();
                    let a = self.pop();
                    let av = to_int32(a);
                    let bv = to_int32(b);
                    self.push(Value::smi(av & bv));
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Logical ----
                // ---- Comparisons ----
                Opcode::Eq => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(if values_loosely_equal(a, b) {
                        Value::boolean(true)
                    } else {
                        Value::boolean(false)
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StrictEq => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(if values_strictly_equal(a, b) {
                        Value::boolean(true)
                    } else {
                        Value::boolean(false)
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Ne => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(if !values_loosely_equal(a, b) {
                        Value::boolean(true)
                    } else {
                        Value::boolean(false)
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StrictNe => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(if !values_strictly_equal(a, b) {
                        Value::boolean(true)
                    } else {
                        Value::boolean(false)
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Lt => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = match (a.as_smi(), b.as_smi()) {
                        (Some(av), Some(bv)) => Value::boolean(av < bv),
                        _ => {
                            if let Some(v) = compare_strings_lt(a, b) {
                                Value::boolean(v)
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::undefined()
                                } else {
                                    Value::boolean(av < bv)
                                }
                            }
                        }
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Gt => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = match (a.as_smi(), b.as_smi()) {
                        (Some(av), Some(bv)) => Value::boolean(av > bv),
                        _ => {
                            if let Some(v) = compare_strings_lt(b, a) {
                                Value::boolean(v)
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::undefined()
                                } else {
                                    Value::boolean(av > bv)
                                }
                            }
                        }
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Le => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = match (a.as_smi(), b.as_smi()) {
                        (Some(av), Some(bv)) => Value::boolean(av <= bv),
                        _ => {
                            if let Some(v) = compare_strings_lt(a, b) {
                                Value::boolean(v)
                            } else if let Some(v) = compare_strings_lt(b, a) {
                                // Both are strings: if b < a then a <= b is false, else equal → true
                                Value::boolean(!v)
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::boolean(false)
                                } else {
                                    Value::boolean(av <= bv)
                                }
                            }
                        }
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Ge => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = match (a.as_smi(), b.as_smi()) {
                        (Some(av), Some(bv)) => Value::boolean(av >= bv),
                        _ => {
                            if let Some(v) = compare_strings_lt(b, a) {
                                Value::boolean(v)
                            } else if let Some(v) = compare_strings_lt(a, b) {
                                Value::boolean(!v)
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::boolean(false)
                                } else {
                                    Value::boolean(av >= bv)
                                }
                            }
                        }
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::In => {
                    let obj = self.pop();
                    let key = self.pop();
                    let found = has_property(obj, key);
                    self.push(Value::boolean(found));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Instanceof => {
                    // TODO: §13.10.3 — check rhs[Symbol.hasInstance] first when Symbol lands.
                    // If rhs has @@hasInstance, call it instead of OrdinaryHasInstance.
                    let rhs = self.pop();
                    let lhs = self.pop();
                    // §13.10.1: If Type(rhs) is not Object → TypeError
                    if !rhs.is_heap_object() {
                        let msg = HeapString::allocate(
                            gc,
                            "TypeError: invalid 'instanceof' operand (RHS is not an object)",
                        );
                        self.push(Value::from_heap_ptr(msg as *mut u8));
                        return Exit::Throw(self.pop());
                    }
                    let rhs_ptr = rhs.heap_ptr().unwrap();
                    let rhs_tag = unsafe { (*(rhs_ptr as *const GcHeader)).tag() };
                    // §13.10.1: If IsCallable(rhs) is false → TypeError
                    if rhs_tag != TAG_FUNC {
                        let msg = HeapString::allocate(
                            gc,
                            "TypeError: RHS of 'instanceof' is not callable",
                        );
                        self.push(Value::from_heap_ptr(msg as *mut u8));
                        return Exit::Throw(self.pop());
                    }
                    // OrdinaryHasInstance §13.10.2
                    let rhs_proto_ptr = unsafe { Func::prototype(rhs_ptr as *mut Func) };
                    if rhs_proto_ptr.is_null() {
                        let msg = HeapString::allocate(
                            gc,
                            "TypeError: function 'prototype' is not an object",
                        );
                        self.push(Value::from_heap_ptr(msg as *mut u8));
                        return Exit::Throw(self.pop());
                    }
                    // Walk lhs prototype chain
                    let result = ordinary_has_instance(lhs, rhs_proto_ptr);
                    self.push(Value::boolean(result));
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Objects ----
                Opcode::NewObject => {
                    let count = instr.operands[0] as usize;
                    let mut values: Vec<Value> = (0..count).map(|_| self.pop()).collect();
                    values.reverse();
                    let mut entries: Vec<(PropertyKey, usize)> = Vec::with_capacity(count);
                    let mut key_names: Vec<String> = Vec::with_capacity(count);
                    for i in 0..count {
                        let key_idx = instr.operands[1 + i] as usize;
                        let key_str = self.frames[fi].prog_str(key_idx).unwrap_or_default();
                        entries.push((PropertyKey::from_string(&key_str), i));
                        key_names.push(key_str);
                    }
                    let shape = Shape::intern(entries, key_names);
                    let obj = JSObject::allocate(gc, shape, &values);
                    if self.object_prototype.is_heap_object()
                        && let Some(proto_ptr) = self.object_prototype.heap_ptr()
                    {
                        unsafe {
                            JSObject::set_prototype(obj, proto_ptr);
                        }
                    }
                    self.push(Value::from_heap_ptr(obj as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::NewArray => {
                    let elem_count = instr.operands[0] as usize;
                    let mut elems: Vec<Value> = (0..elem_count).map(|_| self.pop()).collect();
                    elems.reverse();
                    let arr = RuneArray::allocate(gc, &elems);
                    // Set the DENSE_ARRAY_SHAPE and Array.prototype on the newly allocated array
                    unsafe {
                        let ptr = arr as *mut u8;
                        let shape_ptr = ptr.add(8) as *mut *const Shape;
                        *shape_ptr = *DENSE_ARRAY_SHAPE as *const Shape;
                        let proto_ptr = ptr.add(24) as *mut *mut u8;
                        if self.array_prototype.is_heap_object()
                            && let Some(proto) = self.array_prototype.heap_ptr()
                        {
                            *proto_ptr = proto;
                        }
                    }
                    self.push(Value::from_heap_ptr(arr as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ArrayPush => {
                    let val = self.pop();
                    let arr_val = self.pop();
                    if let Some(heap) = arr_val.heap_ptr() {
                        let arr_ptr = heap as *mut RuneArray;
                        unsafe {
                            let new_arr = RuneArray::push(gc, arr_ptr, val);
                            self.push(Value::from_heap_ptr(new_arr as *mut u8));
                        }
                    } else {
                        self.push(make_error_object(gc, "TypeError", "ArrayPush on non-array"));
                        return Exit::Throw(self.pop());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ArrayExtend => {
                    let src_val = self.pop();
                    let tgt_val = self.pop();
                    if let (Some(src_heap), Some(tgt_heap)) =
                        (src_val.heap_ptr(), tgt_val.heap_ptr())
                    {
                        let src_arr = src_heap as *mut RuneArray;
                        let mut tgt_arr = tgt_heap as *mut RuneArray;
                        let src_len = unsafe { RuneArray::length(src_arr) };
                        for i in 0..src_len {
                            let elem = unsafe { RuneArray::get_element(src_arr, i as usize) };
                            unsafe {
                                tgt_arr = RuneArray::push(gc, tgt_arr, elem);
                            }
                        }
                        self.push(Value::from_heap_ptr(tgt_arr as *mut u8));
                    } else {
                        self.push(make_error_object(
                            gc,
                            "TypeError",
                            "ArrayExtend on non-array",
                        ));
                        return Exit::Throw(self.pop());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ArraySlice => {
                    let start_idx = self.pop();
                    let arr_val = self.pop();
                    let start = start_idx.as_smi().unwrap_or(0) as usize;
                    if let Some(heap) = arr_val.heap_ptr() {
                        let tag = unsafe { (*(heap as *const GcHeader)).tag() };
                        if tag == TAG_ARRAY {
                            let arr_ptr = heap as *mut RuneArray;
                            let len = unsafe { RuneArray::length(arr_ptr) } as usize;
                            let slice_len = len.saturating_sub(start);
                            let mut elems: Vec<Value> = Vec::with_capacity(slice_len);
                            for i in start..len {
                                let v = unsafe { RuneArray::get_element(arr_ptr, i) };
                                elems.push(v);
                            }
                            let new_arr = RuneArray::allocate(gc, &elems);
                            // Set shape and prototype
                            unsafe {
                                let ptr = new_arr as *mut u8;
                                let shape_ptr = ptr.add(8) as *mut *const Shape;
                                *shape_ptr = *DENSE_ARRAY_SHAPE as *const Shape;
                                let proto_ptr = ptr.add(24) as *mut *mut u8;
                                if self.array_prototype.is_heap_object()
                                    && let Some(proto) = self.array_prototype.heap_ptr()
                                {
                                    *proto_ptr = proto;
                                }
                            }
                            self.push(Value::from_heap_ptr(new_arr as *mut u8));
                        } else {
                            self.push(Value::from_heap_ptr(heap));
                        }
                    } else {
                        self.push(Value::undefined());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ToString => {
                    let val = self.pop();
                    let s = value_to_js_string(val);
                    let ptr = HeapString::allocate(gc, &s);
                    self.push(Value::from_heap_ptr(ptr as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StringConcat => {
                    let rhs = self.pop();
                    let lhs = self.pop();
                    let lhs_s = value_to_js_string(lhs);
                    let rhs_s = value_to_js_string(rhs);
                    let combined = lhs_s + &rhs_s;
                    let ptr = HeapString::allocate(gc, &combined);
                    self.push(Value::from_heap_ptr(ptr as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ForInInit => {
                    let obj = self.pop();
                    if obj.is_null() || obj.is_undefined() {
                        self.push(Value::smi(0));
                    } else {
                        self.push(obj);
                        self.push(Value::smi(0));
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ForInNext => {
                    let end_target = instr.operands[0] as usize;
                    let index_val = self.pop();
                    let index = index_val.as_smi().unwrap_or(0) as usize;
                    let obj = self.peek();
                    let done = if let Some(ptr) = obj.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        match tag {
                            TAG_ARRAY => {
                                let len =
                                    unsafe { RuneArray::length(ptr as *mut RuneArray) } as usize;
                                if index < len {
                                    let key_str = index.to_string();
                                    let key = HeapString::allocate(gc, &key_str);
                                    self.push(Value::smi((index + 1) as i32));
                                    self.push(Value::from_heap_ptr(key as *mut u8));
                                    false
                                } else {
                                    true
                                }
                            }
                            TAG_OBJECT => {
                                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                if index < shape.property_count {
                                    let key_name = shape.key_name_at(index).unwrap_or("");
                                    let key = HeapString::allocate(gc, key_name);
                                    self.push(Value::smi((index + 1) as i32));
                                    self.push(Value::from_heap_ptr(key as *mut u8));
                                    false
                                } else {
                                    true
                                }
                            }
                            _ => true,
                        }
                    } else {
                        true
                    };
                    if done {
                        self.pop(); // pop obj
                        self.frames[fi].pc = end_target;
                    } else {
                        self.frames[fi].pc = pc + 1;
                    }
                }
                Opcode::LoadProperty => {
                    let raw_key = self.pop();
                    let obj = self.pop();
                    let result = if obj.is_heap_object() {
                        let tag = {
                            let ptr = obj.heap_ptr().unwrap();
                            unsafe { (*(ptr as *const GcHeader)).tag() }
                        };
                        if tag == TAG_STRING {
                            // String property access
                            if let Some(index) = value_to_array_index(raw_key) {
                                // Numeric index: return character at index
                                let s = unsafe {
                                    HeapString::to_string(obj.heap_ptr().unwrap() as *mut HeapString)
                                };
                                let ch = s.chars().nth(index);
                                match ch {
                                    Some(c) => {
                                        let result_s = HeapString::allocate(gc, &c.to_string());
                                        Value::from_heap_ptr(result_s as *mut u8)
                                    }
                                    None => Value::undefined(),
                                }
                            } else if let Some(ptr) = raw_key.heap_ptr() {
                                let key_tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                                if key_tag == TAG_STRING {
                                    let key_str =
                                        unsafe { HeapString::to_string(ptr as *mut HeapString) };
                                    if key_str == "length" {
                                        // String length
                                        let s = unsafe {
                                            HeapString::to_string(
                                                obj.heap_ptr().unwrap() as *mut HeapString
                                            )
                                        };
                                        let len = s.encode_utf16().count();
                                        Value::smi(len as i32)
                                    } else if self.string_prototype.is_heap_object() {
                                        // Look up from String.prototype
                                        if let Some(proto_ptr) = self.string_prototype.heap_ptr() {
                                            let proto_key = PropertyKey::from_string(&key_str);
                                            let shape = unsafe {
                                                JSObject::shape_ptr(proto_ptr as *mut JSObject)
                                            };
                                            if let Some(slot) = shape.lookup(&proto_key) {
                                                unsafe {
                                                    JSObject::get_slot(
                                                        proto_ptr as *mut JSObject,
                                                        slot,
                                                    )
                                                }
                                            } else {
                                                Value::undefined()
                                            }
                                        } else {
                                            Value::undefined()
                                        }
                                    } else {
                                        Value::undefined()
                                    }
                                } else {
                                    Value::undefined()
                                }
                            } else {
                                Value::undefined()
                            }
                        } else {
                            // IC fast path: check inline cache before full walk
                            if instr.ic_index >= 0 {
                                let ic_idx = instr.ic_index as usize;
                                self.ic_stats.lookups += 1;
                                if ic_idx < self.ics.len()
                                    && let Some(ptr) = obj.heap_ptr()
                                {
                                    if tag == TAG_OBJECT {
                                        let shape =
                                            unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                        let (shape_id, key_hash) = ic_cache_key(shape.id, raw_key);
                                        if let Some(entry) =
                                            self.ics[ic_idx].get(shape_id, key_hash)
                                        {
                                            self.ic_stats.hits += 1;
                                            // Record shape_id for trace analysis
                                            if let Some(target) = self.recording_trace
                                                && let Some(trace) =
                                                    self.loop_traces.get_mut(&target)
                                            {
                                                if !trace.shape_ids.contains(&shape.id) {
                                                    trace.shape_ids.push(shape.id);
                                                }
                                                if let Some(last) = trace.ops.last_mut() {
                                                    last.shape_id = shape.id;
                                                }
                                            }
                                            // Hot-path specialization: after 8 hits, patch
                                            // LoadProperty → LoadPropertyIC for shape-guarded access.
                                            if ic_idx < self.ic_hit_counts.len()
                                                && self.ic_hit_counts[ic_idx] < 8
                                            {
                                                self.ic_hit_counts[ic_idx] += 1;
                                                if self.ic_hit_counts[ic_idx] == 8 {
                                                    let instr_mut = unsafe {
                                                        let instrs_ptr =
                                                            (*prog_ptr).instructions.as_ptr()
                                                                as *mut Instruction;
                                                        &mut *instrs_ptr.add(pc)
                                                    };
                                                    instr_mut.opcode = Opcode::LoadPropertyIC;
                                                    instr_mut.operands.clear();
                                                    instr_mut.operands.extend_from_slice(&[
                                                        shape.id as i64,
                                                        entry.offset as i64,
                                                        entry.proto_depth as i64,
                                                    ]);
                                                    self.ic_entries[ic_idx] = entry;
                                                }
                                            }
                                            let val = if entry.is_own {
                                                unsafe {
                                                    JSObject::get_slot(
                                                        ptr as *mut JSObject,
                                                        entry.offset,
                                                    )
                                                }
                                            } else {
                                                let mut p = ptr;
                                                for _ in 0..entry.proto_depth {
                                                    let next = unsafe {
                                                        JSObject::prototype(p as *mut JSObject)
                                                    };
                                                    if next.is_null() {
                                                        break;
                                                    }
                                                    p = next;
                                                }
                                                unsafe {
                                                    JSObject::get_slot(
                                                        p as *mut JSObject,
                                                        entry.offset,
                                                    )
                                                }
                                            };
                                            self.push(val);
                                            self.frames[fi].pc = pc + 1;
                                            continue;
                                        }
                                    } else if tag == TAG_ARRAY {
                                        let (shape_id, key_hash) =
                                            ic_cache_key(DENSE_ARRAY_SHAPE.id, raw_key);
                                        if let Some(entry) =
                                            self.ics[ic_idx].get(shape_id, key_hash)
                                        {
                                            self.ic_stats.hits += 1;
                                            let len =
                                                unsafe { RuneArray::length(ptr as *mut RuneArray) };
                                            let val = if entry.is_own {
                                                if entry.offset < len as usize {
                                                    unsafe {
                                                        RuneArray::get_element(
                                                            ptr as *mut RuneArray,
                                                            entry.offset,
                                                        )
                                                    }
                                                } else {
                                                    Value::undefined()
                                                }
                                            } else {
                                                // Inherited from Array.prototype
                                                let mut p = ptr;
                                                for _ in 0..entry.proto_depth {
                                                    let next = unsafe {
                                                        JSObject::prototype(p as *mut JSObject)
                                                    };
                                                    if next.is_null() {
                                                        break;
                                                    }
                                                    p = next;
                                                }
                                                unsafe {
                                                    JSObject::get_slot(
                                                        p as *mut JSObject,
                                                        entry.offset,
                                                    )
                                                }
                                            };
                                            self.push(val);
                                            self.frames[fi].pc = pc + 1;
                                            continue;
                                        }
                                    }
                                }
                                self.ic_stats.misses += 1;
                                // Full lookup with IC population

                                load_property_recursive_ic(
                                    gc,
                                    &mut self.ics,
                                    &mut self.ic_entries,
                                    &mut self.ic_hit_counts,
                                    &mut self.ic_stats,
                                    &instr,
                                    obj,
                                    raw_key,
                                )
                            } else {
                                // No IC attached — fall back to full lookup
                                load_property_recursive(obj, raw_key)
                            }
                        }
                    } else {
                        Value::undefined()
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadPropertyIC => {
                    // Shape-guarded fast path. Operands: [cached_shape_id, offset, proto_depth]
                    let raw_key = self.pop();
                    let obj = self.pop();
                    let ic_idx = instr.ic_index as usize;
                    let cached_shape_id = instr.operands.first().copied().unwrap_or(0) as u64;
                    let offset = instr.operands.get(1).copied().unwrap_or(0) as usize;
                    let proto_depth = instr.operands.get(2).copied().unwrap_or(0) as u8;

                    if ic_idx < self.ic_entries.len()
                        && let Some(ptr) = obj.heap_ptr()
                    {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_OBJECT {
                            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                            if shape.id == cached_shape_id {
                                // Record shape_id for trace analysis (fast path)
                                if let Some(target) = self.recording_trace
                                    && let Some(trace) = self.loop_traces.get_mut(&target)
                                {
                                    if !trace.shape_ids.contains(&cached_shape_id) {
                                        trace.shape_ids.push(cached_shape_id);
                                    }
                                    if let Some(last) = trace.ops.last_mut() {
                                        last.shape_id = cached_shape_id;
                                    }
                                }
                                // Shape guard passes — direct slot access
                                let val = if proto_depth == 0 {
                                    unsafe { JSObject::get_slot(ptr as *mut JSObject, offset) }
                                } else {
                                    let mut p = ptr;
                                    for _ in 0..proto_depth {
                                        let next =
                                            unsafe { JSObject::prototype(p as *mut JSObject) };
                                        if next.is_null() {
                                            break;
                                        }
                                        p = next;
                                    }
                                    unsafe { JSObject::get_slot(p as *mut JSObject, offset) }
                                };
                                self.push(val);
                                self.frames[fi].pc = pc + 1;
                                continue;
                            }
                        }
                    }
                    // Shape guard failed — fall back to generic LoadProperty
                    let result = load_property_recursive_ic(
                        gc,
                        &mut self.ics,
                        &mut self.ic_entries,
                        &mut self.ic_hit_counts,
                        &mut self.ic_stats,
                        &instr,
                        obj,
                        raw_key,
                    );
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StoreProperty => {
                    let value = self.pop();
                    let raw_key = self.pop();
                    let obj = self.pop();
                    // IC hit counting: track successful own-property writes for patching
                    if let Some(ptr) = obj.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_OBJECT
                            && !is_proto_key(raw_key)
                            && let Some(key) = value_to_prop_key(raw_key)
                        {
                            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                            if let Some(slot) = shape.lookup(&key) {
                                let ic_idx = instr.ic_index as usize;
                                if ic_idx < self.ic_hit_counts.len()
                                    && self.ic_hit_counts[ic_idx] < 8
                                {
                                    self.ic_hit_counts[ic_idx] += 1;
                                    if self.ic_hit_counts[ic_idx] == 8 {
                                        let instr_mut = unsafe {
                                            let instrs_ptr = (*prog_ptr).instructions.as_ptr()
                                                as *mut Instruction;
                                            &mut *instrs_ptr.add(pc)
                                        };
                                        instr_mut.opcode = Opcode::StorePropertyIC;
                                        instr_mut.operands.clear();
                                        instr_mut.operands.extend_from_slice(&[
                                            shape.id as i64,
                                            slot as i64,
                                            0,
                                        ]);
                                    }
                                }
                            }
                        }
                    }
                    do_store_property(obj, raw_key, value);
                    self.push(value);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StorePropertyIC => {
                    let value = self.pop();
                    let raw_key = self.pop();
                    let obj = self.pop();
                    let cached_shape_id = instr.operands.first().copied().unwrap_or(0) as u64;
                    let offset = instr.operands.get(1).copied().unwrap_or(0) as usize;
                    if let Some(ptr) = obj.heap_ptr()
                        && unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_OBJECT
                        && unsafe { JSObject::shape_ptr(ptr as *mut JSObject) }.id == cached_shape_id
                    {
                        unsafe { JSObject::set_slot(ptr as *mut JSObject, offset, value) };
                    } else {
                        do_store_property(obj, raw_key, value);
                    }
                    self.push(value);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::DeleteProperty => {
                    let raw_key = self.pop();
                    let obj = self.pop();
                    let result = if let Some(ptr) = obj.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_OBJECT
                            && let Some(key) = value_to_prop_key(raw_key)
                        {
                            unsafe { JSObject::remove_property(ptr as *mut JSObject, &key) };
                        }
                        Value::boolean(true)
                    } else {
                        Value::boolean(true)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::DefineProperty => {
                    let value = self.pop();
                    let obj = self.pop();
                    let key_idx = instr.operands[0] as usize;
                    if let Some(key_str) = self.frames[fi].prog_str(key_idx)
                        && let Some(ptr) = obj.heap_ptr()
                    {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_OBJECT {
                            let key = PropertyKey::from_string(&key_str);
                            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                            if let Some(slot) = shape.lookup(&key) {
                                unsafe { JSObject::set_slot(ptr as *mut JSObject, slot, value) };
                            } else {
                                unsafe {
                                    JSObject::add_property(
                                        ptr as *mut JSObject,
                                        key,
                                        key_str.to_string(),
                                        value,
                                    )
                                };
                            }
                        }
                    }
                    self.push(obj);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::SpreadIntoObject => {
                    let source = self.pop();
                    let tgt = self.pop();
                    // §13.2.6.5 step 4: null/undefined → no-op
                    if !source.is_null()
                        && !source.is_undefined()
                        && let (Some(src_ptr), Some(tgt_ptr)) = (source.heap_ptr(), tgt.heap_ptr())
                    {
                        let tag = unsafe { (*(src_ptr as *const GcHeader)).tag() };
                        if tag == TAG_OBJECT {
                            let src_shape =
                                unsafe { JSObject::shape_ptr(src_ptr as *mut JSObject) };
                            let count = src_shape.entries.len();
                            for i in 0..count {
                                let key = src_shape.entries[i].0;
                                let key_name = src_shape.key_names[i].clone();
                                let val =
                                    unsafe { JSObject::get_slot(src_ptr as *mut JSObject, i) };
                                let tgt_shape =
                                    unsafe { JSObject::shape_ptr(tgt_ptr as *mut JSObject) };
                                if let Some(slot) = tgt_shape.lookup(&key) {
                                    unsafe {
                                        JSObject::set_slot(tgt_ptr as *mut JSObject, slot, val)
                                    };
                                } else {
                                    unsafe {
                                        JSObject::add_property(
                                            tgt_ptr as *mut JSObject,
                                            key,
                                            key_name,
                                            val,
                                        )
                                    };
                                }
                            }
                        } else if tag == TAG_ARRAY {
                            let src_len = unsafe { RuneArray::length(src_ptr as *mut RuneArray) };
                            for i in 0..src_len as usize {
                                let elem =
                                    unsafe { RuneArray::get_element(src_ptr as *mut RuneArray, i) };
                                let key_str = i.to_string();
                                let key = PropertyKey::from_string(&key_str);
                                let tgt_shape =
                                    unsafe { JSObject::shape_ptr(tgt_ptr as *mut JSObject) };
                                if let Some(slot) = tgt_shape.lookup(&key) {
                                    unsafe {
                                        JSObject::set_slot(tgt_ptr as *mut JSObject, slot, elem)
                                    };
                                } else {
                                    unsafe {
                                        JSObject::add_property(
                                            tgt_ptr as *mut JSObject,
                                            key,
                                            key_str,
                                            elem,
                                        )
                                    };
                                }
                            }
                            let len_str = "length".to_string();
                            let len_key = PropertyKey::from_string(&len_str);
                            let tgt_shape =
                                unsafe { JSObject::shape_ptr(tgt_ptr as *mut JSObject) };
                            if let Some(slot) = tgt_shape.lookup(&len_key) {
                                unsafe {
                                    JSObject::set_slot(
                                        tgt_ptr as *mut JSObject,
                                        slot,
                                        Value::smi(src_len as i32),
                                    )
                                };
                            } else {
                                unsafe {
                                    JSObject::add_property(
                                        tgt_ptr as *mut JSObject,
                                        len_key,
                                        len_str,
                                        Value::smi(src_len as i32),
                                    )
                                };
                            }
                        }
                    }
                    self.push(tgt);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadGlobal => {
                    let name_idx = instr.operands[0] as usize;
                    if let Some(name) = self.frames[fi].prog_str(name_idx) {
                        let val = self
                            .globals
                            .get(&name)
                            .copied()
                            .or_else(|| self.builtin_wrappers.get(&name).copied())
                            .or_else(|| self.get_builtin(&name))
                            .unwrap_or(Value::undefined());
                        self.push(val);
                    } else {
                        self.push(Value::undefined());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StoreGlobal => {
                    let name_idx = instr.operands[0] as usize;
                    let value = self.pop();
                    if let Some(name) = self.frames[fi].prog_str(name_idx) {
                        self.globals.insert(name, value);
                    }
                    self.push(value);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::IncLocal => {
                    let idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    let old_val = if idx < self.frames[fi].locals.len() {
                        self.frames[fi].locals[idx]
                    } else {
                        Value::undefined()
                    };
                    let n = to_number(old_val) + 1.0;
                    let new_val = number_result(gc, n);
                    if idx >= self.frames[fi].locals.len() {
                        self.frames[fi].locals.resize(idx + 1, Value::undefined());
                    }
                    self.frames[fi].locals[idx] = new_val;
                    self.push(if is_prefix { new_val } else { old_val });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::DecLocal => {
                    let idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    let old_val = if idx < self.frames[fi].locals.len() {
                        self.frames[fi].locals[idx]
                    } else {
                        Value::undefined()
                    };
                    let n = to_number(old_val) - 1.0;
                    let new_val = number_result(gc, n);
                    if idx >= self.frames[fi].locals.len() {
                        self.frames[fi].locals.resize(idx + 1, Value::undefined());
                    }
                    self.frames[fi].locals[idx] = new_val;
                    self.push(if is_prefix { new_val } else { old_val });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::IncGlobal => {
                    let name_idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    if let Some(name) = self.frames[fi].prog_str(name_idx) {
                        let old_val = self
                            .globals
                            .get(&name)
                            .copied()
                            .or_else(|| self.builtin_wrappers.get(&name).copied())
                            .or_else(|| self.get_builtin(&name))
                            .unwrap_or(Value::undefined());
                        let n = to_number(old_val) + 1.0;
                        let new_val = number_result(gc, n);
                        self.globals.insert(name, new_val);
                        self.push(if is_prefix { new_val } else { old_val });
                    } else {
                        self.push(Value::undefined());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::DecGlobal => {
                    let name_idx = instr.operands[0] as usize;
                    let is_prefix = instr.operands[1] != 0;
                    if let Some(name) = self.frames[fi].prog_str(name_idx) {
                        let old_val = self
                            .globals
                            .get(&name)
                            .copied()
                            .or_else(|| self.builtin_wrappers.get(&name).copied())
                            .or_else(|| self.get_builtin(&name))
                            .unwrap_or(Value::undefined());
                        let n = to_number(old_val) - 1.0;
                        let new_val = number_result(gc, n);
                        self.globals.insert(name, new_val);
                        self.push(if is_prefix { new_val } else { old_val });
                    } else {
                        self.push(Value::undefined());
                    }
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Lexical scoping (let/const/TDZ) ----
                Opcode::BlockEnter => {
                    let count = instr.operands[0] as usize;
                    let fi = self.frames.len() - 1;
                    let f = &mut self.frames[fi];
                    f.scope_boundaries.push(f.lexical_slots.len());
                    f.lexical_slots
                        .extend(std::iter::repeat_n(Value::undefined(), count));
                    f.lexical_tdz.extend(std::iter::repeat_n(true, count));
                    f.lexical_const.extend(std::iter::repeat_n(false, count));
                    f.pc = pc + 1;
                }
                Opcode::BlockLeave => {
                    let fi = self.frames.len() - 1;
                    let f = &mut self.frames[fi];
                    if let Some(boundary) = f.scope_boundaries.pop() {
                        f.lexical_slots.truncate(boundary);
                        f.lexical_tdz.truncate(boundary);
                        f.lexical_const.truncate(boundary);
                    }
                    f.pc = pc + 1;
                }
                Opcode::DeclareLet => {
                    let slot = instr.operands[0] as usize;
                    let fi = self.frames.len() - 1;
                    let val = self.pop();
                    let f = &mut self.frames[fi];
                    if slot < f.lexical_slots.len() {
                        f.lexical_slots[slot] = val;
                        f.lexical_tdz[slot] = false;
                    }
                    f.pc = pc + 1;
                }
                Opcode::DeclareConst => {
                    let slot = instr.operands[0] as usize;
                    let fi = self.frames.len() - 1;
                    let val = self.pop();
                    let f = &mut self.frames[fi];
                    if slot < f.lexical_slots.len() {
                        f.lexical_slots[slot] = val;
                        f.lexical_tdz[slot] = false;
                        f.lexical_const[slot] = true;
                    }
                    f.pc = pc + 1;
                }
                Opcode::LoadLexical => {
                    let slot = instr.operands[0] as usize;
                    let fi = self.frames.len() - 1;
                    let f = &self.frames[fi];
                    if slot < f.lexical_slots.len() {
                        if f.lexical_tdz[slot] {
                            return self.throw_reference_error(
                                gc,
                                &format!("Cannot access '{}' before initialization", slot),
                            );
                        }
                        self.push(f.lexical_slots[slot]);
                    } else {
                        self.push(Value::undefined());
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StoreLexical => {
                    let slot = instr.operands[0] as usize;
                    let fi = self.frames.len() - 1;
                    let val = self.pop();
                    // Check TDZ before store (per spec §8.1.1.4.4, SetMutableBinding
                    // throws ReferenceError if binding is uninitialized)
                    if slot < self.frames[fi].lexical_slots.len() {
                        if self.frames[fi].lexical_tdz[slot] {
                            return self.throw_reference_error(
                                gc,
                                &format!("Cannot access '{}' before initialization", slot),
                            );
                        }
                        if self.frames[fi].lexical_const[slot] {
                            return self.throw_type_error(gc, "Assignment to constant variable");
                        }
                        self.frames[fi].lexical_slots[slot] = val;
                    }
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Unary ----
                Opcode::TypeOf => {
                    let val = self.pop();
                    let s = if val.is_undefined() {
                        "undefined"
                    } else if val.is_null() {
                        "object"
                    } else if val.is_boolean() {
                        "boolean"
                    } else if val.is_smi() {
                        "number"
                    } else {
                        let ptr = val.raw() as *mut GcHeader;
                        let tag = unsafe { (*ptr).tag() };
                        match tag {
                            TAG_STRING => "string",
                            TAG_FUNC => "function",
                            TAG_FLOAT64 => "number",
                            _ => "object",
                        }
                    };
                    let str = HeapString::allocate(gc, s);
                    self.push(Value::from_heap_ptr(str as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Control flow ----
                Opcode::Jump => {
                    let target = instr.operands[0] as usize;
                    if target < pc {
                        // Back-edge: loop iteration
                        let entry = self.loop_counts.entry(target).or_insert(0);
                        *entry += 1;
                        // Start recording a trace at threshold
                        if *entry == 50 {
                            self.recording_trace = Some(target);
                            self.loop_traces.insert(
                                target,
                                LoopTrace {
                                    target_pc: target,
                                    ops: Vec::new(),
                                    total_iterations: *entry,
                                    shape_ids: Vec::new(),
                                    compiled_entry: std::ptr::null(),
                                    exit_pc: 0,
                                },
                            );
                        }
                        // After trace recorded (monomorphic), patch loop body
                        if *entry > 60
                            && self
                                .loop_traces
                                .get(&target)
                                .is_some_and(|t| t.is_monomorphic())
                        {
                            unsafe {
                                self.patch_loop_body(prog_ptr, target, pc);
                            }
                        }
                        // Execute compiled trace natively, bypassing interpreter
                        let compiled = self
                            .loop_traces
                            .get(&target)
                            .map(|t| t.compiled_entry)
                            .unwrap_or(std::ptr::null());
                        if !compiled.is_null() {
                            // Execute compiled trace natively.  The trace runs the
                            // entire loop body (condition + body + branch); when the
                            // condition becomes false it exits.  Works for all Smi
                            // values; results above i31 range display as wrapped i32
                            // due to as_smi() truncation, but the underlying u64 is
                            // correct.
                            unsafe {
                                let gc_ptr = gc as *mut SemiSpace as *mut u8;
                                let _ = self.execute_trace(fi, compiled, gc_ptr);
                            }
                            self.frames[fi].pc = self
                                .loop_traces
                                .get(&target)
                                .map(|t| t.exit_pc)
                                .unwrap_or(pc + 1);
                            continue;
                        }
                    }
                    self.frames[fi].pc = target;
                }
                Opcode::JumpIfTrue => {
                    let val = self.pop();
                    let target = instr.operands[0] as usize;
                    if val.to_bool() {
                        self.frames[fi].pc = target
                    } else {
                        self.frames[fi].pc = pc + 1
                    }
                }
                Opcode::JumpIfFalse => {
                    let val = self.pop();
                    let target = instr.operands[0] as usize;
                    if !val.to_bool() {
                        self.frames[fi].pc = target
                    } else {
                        self.frames[fi].pc = pc + 1
                    }
                }
                Opcode::Throw => {
                    let val = self.pop();
                    // Find in-frame handler
                    let handler_idx = self
                        .try_stack
                        .iter()
                        .rposition(|tf| tf.frame_depth == self.frames.len());
                    if let Some(idx) = handler_idx {
                        let (catch_pc, finally_pc, stack_depth, in_catch) = {
                            let tf = &self.try_stack[idx];
                            (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                        };
                        if in_catch && finally_pc != 0 {
                            self.try_stack[idx].saved_exception = Some(val);
                            self.stack.truncate(stack_depth);
                            self.frames[fi].pc = finally_pc;
                            continue;
                        }
                        if catch_pc != 0 && !in_catch {
                            if finally_pc != 0 {
                                // Keep TryFrame for redirecting catch-body exceptions to finally
                                self.try_stack[idx].in_catch = true;
                            } else {
                                // No finally — pop TryFrame, exception is now handled
                                self.try_stack.remove(idx);
                            }
                            self.stack.truncate(stack_depth);
                            self.push(val);
                            self.frames[fi].pc = catch_pc;
                            continue;
                        }
                        if finally_pc != 0 {
                            self.try_stack[idx].saved_exception = Some(val);
                            self.stack.truncate(stack_depth);
                            self.frames[fi].pc = finally_pc;
                            continue;
                        }
                    }
                    // No handler — pop frame and check caller
                    let callee_base = self.frames.last().unwrap().stack_base;
                    let popped_frame = self.frames.len() - 1;
                    self.last_locals = self.frames[popped_frame].locals.clone();
                    self.frames.pop();
                    self.try_stack.retain(|tf| tf.frame_depth != popped_frame);
                    if self.frames.is_empty() {
                        self.stack.clear();
                        return Exit::Throw(val);
                    }
                    // Check for try-catch-finally in the caller frame
                    let new_fi = self.frames.len() - 1;
                    let caller_idx = self
                        .try_stack
                        .iter()
                        .rposition(|tf| tf.frame_depth == self.frames.len());
                    if let Some(idx) = caller_idx {
                        let (catch_pc, finally_pc, stack_depth, in_catch) = {
                            let tf = &self.try_stack[idx];
                            (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                        };
                        if in_catch && finally_pc != 0 {
                            self.try_stack[idx].saved_exception = Some(val);
                            self.stack.truncate(stack_depth);
                            self.frames[new_fi].pc = finally_pc;
                            continue;
                        }
                        if catch_pc != 0 && !in_catch {
                            if finally_pc != 0 {
                                self.try_stack[idx].in_catch = true;
                            } else {
                                self.try_stack.remove(idx);
                            }
                            self.stack.truncate(stack_depth);
                            self.push(val);
                            self.frames[new_fi].pc = catch_pc;
                            continue;
                        }
                        if finally_pc != 0 {
                            self.try_stack[idx].saved_exception = Some(val);
                            self.stack.truncate(stack_depth);
                            self.frames[new_fi].pc = finally_pc;
                            continue;
                        }
                    }
                    self.stack.truncate(callee_base);
                    self.push(val);
                    self.frames[new_fi].pc += 1;
                    return Exit::Throw(val);
                }
                Opcode::ThrowIfNullish => {
                    let val = self.peek();
                    if val.is_null() || val.is_undefined() {
                        self.pop();
                        self.register_roots(gc);
                        let exc = make_error_object(
                            gc,
                            "TypeError",
                            "Cannot destructure null or undefined",
                        );
                        // Now behave like Opcode::Throw
                        let handler_idx = self
                            .try_stack
                            .iter()
                            .rposition(|tf| tf.frame_depth == self.frames.len());
                        if let Some(idx) = handler_idx {
                            let (catch_pc, finally_pc, stack_depth, in_catch) = {
                                let tf = &self.try_stack[idx];
                                (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                            };
                            if in_catch && finally_pc != 0 {
                                self.try_stack[idx].saved_exception = Some(exc);
                                self.stack.truncate(stack_depth);
                                self.frames[fi].pc = finally_pc;
                                continue;
                            }
                            if catch_pc != 0 && !in_catch {
                                if finally_pc != 0 {
                                    self.try_stack[idx].in_catch = true;
                                } else {
                                    self.try_stack.remove(idx);
                                }
                                self.stack.truncate(stack_depth);
                                self.push(exc);
                                self.frames[fi].pc = catch_pc;
                                continue;
                            }
                            if finally_pc != 0 {
                                self.try_stack[idx].saved_exception = Some(exc);
                                self.stack.truncate(stack_depth);
                                self.frames[fi].pc = finally_pc;
                                continue;
                            }
                        }
                        // No handler — pop frame and check caller
                        let callee_base = self.frames.last().unwrap().stack_base;
                        let popped_frame = self.frames.len() - 1;
                        self.last_locals = self.frames[popped_frame].locals.clone();
                        self.frames.pop();
                        self.try_stack.retain(|tf| tf.frame_depth != popped_frame);
                        if self.frames.is_empty() {
                            self.stack.clear();
                            return Exit::Throw(exc);
                        }
                        let new_fi = self.frames.len() - 1;
                        let caller_idx = self
                            .try_stack
                            .iter()
                            .rposition(|tf| tf.frame_depth == self.frames.len());
                        if let Some(idx) = caller_idx {
                            let (catch_pc, finally_pc, stack_depth, in_catch) = {
                                let tf = &self.try_stack[idx];
                                (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                            };
                            if in_catch && finally_pc != 0 {
                                self.try_stack[idx].saved_exception = Some(exc);
                                self.stack.truncate(stack_depth);
                                self.frames[new_fi].pc = finally_pc;
                                continue;
                            }
                            if catch_pc != 0 && !in_catch {
                                if finally_pc != 0 {
                                    self.try_stack[idx].in_catch = true;
                                } else {
                                    self.try_stack.remove(idx);
                                }
                                self.stack.truncate(stack_depth);
                                self.push(exc);
                                self.frames[new_fi].pc = catch_pc;
                                continue;
                            }
                            if finally_pc != 0 {
                                self.try_stack[idx].saved_exception = Some(exc);
                                self.stack.truncate(stack_depth);
                                self.frames[new_fi].pc = finally_pc;
                                continue;
                            }
                        }
                        self.stack.truncate(callee_base);
                        self.push(exc);
                        self.frames[new_fi].pc += 1;
                        return Exit::Throw(exc);
                    }
                    self.frames[fi].pc += 1;
                }
                Opcode::TryBegin => {
                    let catch_pc = instr.operands[0] as usize;
                    let finally_pc = instr.operands[1] as usize;
                    self.try_stack.push(TryFrame {
                        catch_pc,
                        finally_pc,
                        stack_depth: self.stack.len(),
                        frame_depth: self.frames.len(),
                        saved_exception: None,
                        in_catch: false,
                    });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::TryEnd => {
                    self.try_stack.pop();
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::FinallyDone => {
                    let rethrow_pc = instr.operands[0] as usize;
                    let tf = self.try_stack.pop().expect("FinallyDone without TryFrame");
                    if let Some(ex) = tf.saved_exception {
                        self.push(ex);
                        self.frames[fi].pc = rethrow_pc;
                    } else {
                        self.frames[fi].pc = pc + 1;
                    }
                }

                // ---- Functions ----
                Opcode::MakeRestArray => {
                    let regular_count = instr.operands[0] as usize;
                    let named_offset = if unsafe { (*self.frames[fi].prog).named_function } {
                        1
                    } else {
                        0
                    };
                    let rest_start = named_offset + regular_count;
                    let rest_end = self.frames[fi].locals.len();
                    let mut elems: Vec<Value> = Vec::new();
                    for i in rest_start..rest_end {
                        elems.push(self.frames[fi].locals[i]);
                    }
                    let arr = RuneArray::allocate(gc, &elems);
                    unsafe {
                        let ptr = arr as *mut u8;
                        let shape_ptr = ptr.add(8) as *mut *const Shape;
                        *shape_ptr = *DENSE_ARRAY_SHAPE as *const Shape;
                        let proto_ptr = ptr.add(24) as *mut *mut u8;
                        if self.array_prototype.is_heap_object()
                            && let Some(proto) = self.array_prototype.heap_ptr()
                        {
                            *proto_ptr = proto;
                        }
                    }
                    self.push(Value::from_heap_ptr(arr as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::MakeArgumentsArray => {
                    let named_offset = if unsafe { (*self.frames[fi].prog).named_function } {
                        1
                    } else {
                        0
                    };
                    let argc = self.frames[fi].passed_argc;
                    let mut elems: Vec<Value> = Vec::with_capacity(argc);
                    for i in 0..argc {
                        elems.push(self.frames[fi].locals[named_offset + i]);
                    }
                    let arr = RuneArray::allocate(gc, &elems);
                    unsafe {
                        let ptr = arr as *mut u8;
                        let shape_ptr = ptr.add(8) as *mut *const Shape;
                        *shape_ptr = *DENSE_ARRAY_SHAPE as *const Shape;
                        let proto_ptr = ptr.add(24) as *mut *mut u8;
                        if self.array_prototype.is_heap_object()
                            && let Some(proto) = self.array_prototype.heap_ptr()
                        {
                            *proto_ptr = proto;
                        }
                    }
                    self.push(Value::from_heap_ptr(arr as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::CopyLexical => {
                    let src_slot = instr.operands[0] as usize;
                    let dst_slot = instr.operands[1] as usize;
                    let f = &self.frames[fi];
                    let val = if src_slot < f.lexical_slots.len() {
                        f.lexical_slots[src_slot]
                    } else {
                        Value::undefined()
                    };
                    let f = &mut self.frames[fi];
                    if dst_slot >= f.lexical_slots.len() {
                        f.lexical_slots.resize(dst_slot + 1, Value::undefined());
                        f.lexical_tdz.resize(dst_slot + 1, false);
                        f.lexical_const.resize(dst_slot + 1, false);
                    }
                    f.lexical_slots[dst_slot] = val;
                    f.lexical_tdz[dst_slot] = false;
                    f.pc = pc + 1;
                }
                Opcode::MakeFunction => {
                    let func_idx = instr.operands[0] as u64;
                    let is_arrow = instr.operands.get(1).copied().unwrap_or(0) != 0;
                    let prog_ptr = prog as *const BytecodeProgram as *const u8;
                    // Allocate the default `.prototype` FIRST so that if GC triggers
                    // during Func::allocate, we can resolve the forwarding address.
                    let default_proto = if !is_arrow {
                        JSObject::allocate(gc, Shape::empty(), &[])
                    } else {
                        std::ptr::null_mut()
                    };
                    let ptr = Func::allocate(gc, func_idx, prog_ptr, is_arrow, self.frames[fi].env);
                    // Both default_proto and ptr may be stale after GC-triggered
                    // collection during either allocate. Resolve via forwarding.
                    unsafe {
                        let resolved_ptr = if (*(ptr as *const GcHeader)).is_forwarded() {
                            (*(ptr as *const GcHeader)).forwarding_addr() as *mut Func
                        } else {
                            ptr
                        };
                        // AFPC: install a cached native entry point if one exists.
                        if let Some(&entry) = self.cached_jit_entries.get(&(func_idx as usize)) {
                            Func::set_jit_entry(resolved_ptr, entry);
                        }
                        Func::set_env_ptr(resolved_ptr, self.frames[fi].env);
                        if !is_arrow {
                            let resolved_proto = if !default_proto.is_null()
                                && (*(default_proto as *const GcHeader)).is_forwarded()
                            {
                                (*(default_proto as *const GcHeader)).forwarding_addr()
                            } else {
                                default_proto as *mut u8
                            };
                            Func::set_prototype(resolved_ptr, resolved_proto);
                        }
                        self.push(Value::from_heap_ptr(resolved_ptr as *mut u8));
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::MakeEnv => {
                    let count = instr.operands[0] as usize;
                    let new_env =
                        EnvObject::allocate(gc, count, self.frames[fi].env as *mut EnvObject);
                    // new_env and parent may be stale after GC-triggered collection;
                    // resolve forwarding and re-read from the (updated) root.
                    unsafe {
                        let resolved = if (*(new_env as *const GcHeader)).is_forwarded() {
                            (*(new_env as *const GcHeader)).forwarding_addr() as *mut EnvObject
                        } else {
                            new_env
                        };
                        EnvObject::set_parent(resolved, self.frames[fi].env as *mut EnvObject);
                        self.frames[fi].env = resolved as *mut u8;
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::RestoreEnv => {
                    let env = self.frames[fi].env as *mut EnvObject;
                    if !env.is_null() {
                        let parent = unsafe { EnvObject::parent(env) };
                        self.frames[fi].env = parent as *mut u8;
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadCaptured => {
                    let depth = instr.operands[0] as usize;
                    let slot = instr.operands[1] as usize;
                    let env = self.frames[fi].env as *mut EnvObject;
                    let target = unsafe { EnvObject::ancestor(env, depth) };
                    let val = unsafe { EnvObject::get_slot(target, slot) };
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::StoreCaptured => {
                    let depth = instr.operands[0] as usize;
                    let slot = instr.operands[1] as usize;
                    let val = self.pop();
                    let env = self.frames[fi].env as *mut EnvObject;
                    let target = unsafe { EnvObject::ancestor(env, depth) };
                    unsafe { EnvObject::set_slot(target, slot, val) };
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::New => {
                    let argc = instr.operands[0] as usize;
                    let mut args: Vec<Value> = (0..argc).map(|_| self.pop()).collect();
                    args.reverse();
                    let constructor = self.pop();
                    // Create a new empty object
                    let shape = Shape::empty();
                    let obj = JSObject::allocate(gc, shape, &[]);
                    let obj_val = Value::from_heap_ptr(obj as *mut u8);
                    // If constructor is a builtin, call it with the new object as `this`
                    if let Some(smi_val) = constructor.as_smi()
                        && smi_val < 0
                    {
                        let id = ((-smi_val) as usize) - 1;
                        if id < self.builtins.len() {
                            let result = (self.builtins[id].func)(gc, obj_val, &args, &mut *self);
                            if let Some(exc) = self.pending_exception.take() {
                                self.push(exc);
                                return Exit::Throw(exc);
                            }
                            if result.is_heap_object() {
                                self.push(result);
                            } else {
                                self.push(obj_val);
                            }
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }
                    // Set prototype from constructor.prototype
                    // §11.2.2 [[Construct]]: new object's [[Prototype]] = constructor.prototype
                    // Use interned PROTOTYPE_KEY to avoid HeapString allocation.
                    if constructor.is_heap_object()
                        && let Some(ptr) = constructor.heap_ptr()
                    {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_OBJECT {
                            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                            if let Some(slot) = shape.lookup(&PROTOTYPE_KEY) {
                                let proto_val =
                                    unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                                if proto_val.is_heap_object()
                                    && let Some(proto_ptr) = proto_val.heap_ptr()
                                {
                                    unsafe {
                                        JSObject::set_prototype(obj, proto_ptr);
                                    }
                                }
                            }
                        } else if tag == TAG_FUNC {
                            // User-defined function: read prototype from Func struct
                            let proto_ptr = unsafe { Func::prototype(ptr as *mut Func) };
                            if !proto_ptr.is_null() {
                                unsafe {
                                    JSObject::set_prototype(obj, proto_ptr);
                                }
                            }
                        }
                    }
                    // If constructor is a user-defined function, call its body with this = new object
                    if let Some(ptr) = constructor.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_FUNC {
                            // §16.2.1.1.1: Arrow functions have [[Construct]]: undefined
                            if unsafe { Func::is_arrow(ptr as *mut Func) } {
                                let msg = HeapString::allocate(
                                    gc,
                                    "TypeError: Arrow function is not a constructor",
                                );
                                self.push(Value::from_heap_ptr(msg as *mut u8));
                                let val = self.pop();
                                // Manually unwind through try_stack like Opcode::Throw does
                                let handler_idx = self
                                    .try_stack
                                    .iter()
                                    .rposition(|tf| tf.frame_depth == self.frames.len());
                                if let Some(idx) = handler_idx {
                                    let (catch_pc, finally_pc, stack_depth, in_catch) = {
                                        let tf = &self.try_stack[idx];
                                        (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                                    };
                                    if in_catch && finally_pc != 0 {
                                        self.try_stack[idx].saved_exception = Some(val);
                                        self.stack.truncate(stack_depth);
                                        self.frames[fi].pc = finally_pc;
                                        continue;
                                    }
                                    if catch_pc != 0 && !in_catch {
                                        if finally_pc != 0 {
                                            self.try_stack[idx].in_catch = true;
                                        } else {
                                            self.try_stack.remove(idx);
                                        }
                                        self.stack.truncate(stack_depth);
                                        self.push(val);
                                        self.frames[fi].pc = catch_pc;
                                        continue;
                                    }
                                    if finally_pc != 0 {
                                        self.try_stack[idx].saved_exception = Some(val);
                                        self.stack.truncate(stack_depth);
                                        self.frames[fi].pc = finally_pc;
                                        continue;
                                    }
                                }
                                // No handler — pop frame and check caller
                                let popped_frame = self.frames.len() - 1;
                                self.last_locals = self.frames[popped_frame].locals.clone();
                                self.frames.pop();
                                self.try_stack.retain(|tf| tf.frame_depth != popped_frame);
                                if self.frames.is_empty() {
                                    self.stack.clear();
                                    return Exit::Throw(val);
                                }
                                let new_fi = self.frames.len() - 1;
                                let caller_idx = self
                                    .try_stack
                                    .iter()
                                    .rposition(|tf| tf.frame_depth == self.frames.len());
                                if let Some(idx) = caller_idx {
                                    let (catch_pc, finally_pc, stack_depth, in_catch) = {
                                        let tf = &self.try_stack[idx];
                                        (tf.catch_pc, tf.finally_pc, tf.stack_depth, tf.in_catch)
                                    };
                                    if in_catch && finally_pc != 0 {
                                        self.try_stack[idx].saved_exception = Some(val);
                                        self.stack.truncate(stack_depth);
                                        self.frames[new_fi].pc = finally_pc;
                                        continue;
                                    }
                                    if catch_pc != 0 && !in_catch {
                                        if finally_pc != 0 {
                                            self.try_stack[idx].in_catch = true;
                                        } else {
                                            self.try_stack.remove(idx);
                                        }
                                        self.stack.truncate(stack_depth);
                                        self.push(val);
                                        self.frames[new_fi].pc = catch_pc;
                                        continue;
                                    }
                                    if finally_pc != 0 {
                                        self.try_stack[idx].saved_exception = Some(val);
                                        self.stack.truncate(stack_depth);
                                        self.frames[new_fi].pc = finally_pc;
                                        continue;
                                    }
                                }
                                self.stack.clear();
                                return Exit::Throw(val);
                            }
                            let func_idx = unsafe { Func::func_index(ptr as *mut Func) } as usize;
                            let creator_prog = unsafe {
                                &*(Func::prog_ptr(ptr as *mut Func) as *const BytecodeProgram)
                            };
                            if func_idx < creator_prog.functions.len() {
                                let func_prog = &creator_prog.functions[func_idx];
                                let mut locals: Vec<Value> = if func_prog.named_function {
                                    vec![constructor]
                                } else {
                                    vec![]
                                };
                                let passed_argc = args.len();
                                locals.extend(args);
                                let func_ptr = ptr as *mut Func;
                                let func_env = unsafe { Func::env_ptr(func_ptr) };
                                self.frames.push(Frame {
                                    locals,
                                    lexical_slots: Vec::new(),
                                    lexical_tdz: Vec::new(),
                                    lexical_const: Vec::new(),
                                    scope_boundaries: Vec::new(),
                                    passed_argc,
                                    pc: 0,
                                    stack_base: self.stack.len(),
                                    prog: func_prog as *const BytecodeProgram,
                                    generator_id: None,
                                    this: obj_val,
                                    is_constructor_call: true,
                                    constructed_object: obj_val,
                                    env: func_env,
                                });
                                continue;
                            }
                        }
                    }
                    self.push(obj_val);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Call => {
                    let argc = instr.operands[0] as usize;
                    let mut args: Vec<Value> = (0..argc).map(|_| self.pop()).collect();
                    args.reverse();
                    let callee = self.pop();
                    let this = self.pop();

                    // Builtin dispatch: negative Smi handles
                    if let Some(smi_val) = callee.as_smi() {
                        if smi_val < 0 {
                            let id = ((-smi_val) as usize) - 1;
                            if id < self.builtins.len() {
                                let result = (self.builtins[id].func)(gc, this, &args, &mut *self);
                                if let Some(exc) = self.pending_exception.take() {
                                    self.push(exc);
                                    return Exit::Throw(exc);
                                }
                                self.push(result);
                                self.frames[fi].pc = pc + 1;
                                continue;
                            }
                        } else {
                            // Positive Smi: generator handle — push undefined
                            self.push(Value::undefined());
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }

                    if let Some(ptr) = callee.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_FUNC {
                            let func_idx = unsafe { Func::func_index(ptr as *mut Func) } as usize;
                            let creator_prog = unsafe {
                                &*(Func::prog_ptr(ptr as *mut Func) as *const BytecodeProgram)
                            };
                            if func_idx < creator_prog.functions.len() {
                                let func_prog = &creator_prog.functions[func_idx];
                                if func_prog.is_generator {
                                    let g =
                                        Generator::new(args, func_prog as *const BytecodeProgram);
                                    let gen_id = self.generators.len();
                                    self.generators.push(g);
                                    self.push(Value::smi(gen_id as i32));
                                    self.frames[fi].pc = pc + 1;
                                    continue;
                                }
                                // --- JIT tier-up (if enabled) ---
                                #[cfg(feature = "jit")]
                                {
                                    unsafe { Func::increment_call_count(ptr as *mut Func) };
                                    let count = unsafe { Func::call_count(ptr as *mut Func) };
                                    const JIT_THRESHOLD: u32 = 50;
                                    const MIN_JIT_FUNCTION_SIZE: usize = 3;

                                    // Only JIT-compile functions large enough to amortize
                                    // prologue/epilogue overhead. Tiny leaf functions like
                                    // `add(a,b){return a+b;}` are faster in the interpreter.
                                    let large_enough = func_prog.instructions.len() >= MIN_JIT_FUNCTION_SIZE;

                                    if unsafe { Func::jit_entry(ptr as *mut Func) }.is_null()
                                        && count == JIT_THRESHOLD
                                        && large_enough
                                        && rune_jit_baseline::is_jit_compatible(func_prog)
                                    {
                                        #[cfg(target_arch = "x86_64")]
                                        let compiled = {
                                            let codegen = CodeGen::new(func_prog.instructions.len());
                                            codegen.compile(func_prog)
                                        };
                                        #[cfg(target_arch = "aarch64")]
                                        let compiled = {
                                            let codegen = Aarch64CodeGen::new(func_prog.instructions.len());
                                            codegen.compile(func_prog)
                                        };
                                        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
                                        let compiled = {
                                            let _ = func_prog;
                                            unreachable!("JIT not supported on this architecture")
                                        };
                                        compiled.mem.make_executable();
                                        let entry = compiled.mem.code_ptr();
                                        unsafe {
                                            Func::set_jit_entry(ptr as *mut Func, entry);
                                        }
                                        self.bailout_tables
                                            .insert(entry as usize, Box::new(compiled.bailout_table));
                                        std::mem::forget(compiled.mem);
                                    }

                                    let jit_entry = unsafe { Func::jit_entry(ptr as *mut Func) };
                                    if !jit_entry.is_null() && large_enough {
                                        let mut jit_locals: Vec<Value> = if func_prog.named_function
                                        {
                                            vec![callee]
                                        } else {
                                            vec![]
                                        };
                                        jit_locals.extend(args.iter().copied());
                                        let local_count = func_prog.local_names.len();
                                        while jit_locals.len() < local_count {
                                            jit_locals.push(Value::undefined());
                                        }
                                        // Phase D: JIT accepts any argument types. Input Smi guards
                                        // on every value-consuming opcode handle non-Smi values by
                                        // bailing to the interpreter.
                                            self.jit_entry_count += 1;
                                            let func: JitEntryFn =
                                                unsafe { std::mem::transmute(jit_entry) };
                                            let vm_ptr = self as *mut Vm as *mut u8;
                                            let gc_ptr = gc as *mut SemiSpace as *mut u8;
                                            self.jit_helpers.lexical_helper =
                                                rune_jit_lexical_helper as *const () as usize;
                                            self.jit_helpers.bailout_helper =
                                                rune_jit_bailout_helper as *const () as usize;
                                            self.jit_helpers.typeof_helper =
                                                rune_jit_typeof_helper as *const () as usize;
                                            self.jit_helpers.string_helper =
                                                rune_jit_string_helper as *const () as usize;
                                            self.jit_helpers.global_helper =
                                                rune_jit_global_helper as *const () as usize;
                                            // Clear pending flag before entering JIT; the bailout
                                            // helper sets it if a bailout occurs (cannot use
                                            // bc_pc != 0 as sentinel — MakeArgumentsArray at PC 0
                                            // would collide).
                                            self.jit_bailout.pending = false;
                                            let result_raw = unsafe {
                                                func(
                                                    vm_ptr,
                                                    gc_ptr,
                                                    jit_locals.as_mut_ptr() as *mut u64,
                                                )
                                            };
                                            if self.jit_bailout.pending {
                                                // Bailout occurred — materialise interpreter state.
                                                // Push a new Frame for the callee per §6.2: the
                                                // bc_pc is inside the callee's bytecode, not the
                                                // caller's frame at fi.
                                                let bailout_bc_pc = self.jit_bailout.bc_pc;
                                                self.jit_bailout.pending = false;
                                                self.jit_bailout.bc_pc = 0;
                                                // Clone locals for the frame (bailout is rare).
                                                let mut bailout_locals = jit_locals.clone();
                                                while bailout_locals.len() < local_count {
                                                    bailout_locals.push(Value::undefined());
                                                }
                                                let func_env =
                                                    unsafe { Func::env_ptr(ptr as *mut Func) };
                                                self.frames.push(Frame {
                                                    locals: bailout_locals,
                                                    lexical_slots: Vec::new(),
                                                    lexical_tdz: Vec::new(),
                                                    lexical_const: Vec::new(),
                                                    scope_boundaries: Vec::new(),
                                                    passed_argc: args.len(),
                                                    pc: bailout_bc_pc,
                                                    stack_base: self.stack.len(),
                                                    prog: func_prog as *const BytecodeProgram,
                                                    generator_id: None,
                                                    this,
                                                    is_constructor_call: false,
                                                    constructed_object: Value::undefined(),
                                                    env: func_env,
                                                });
                                                let snapshot = std::mem::take(
                                                    &mut self.jit_bailout.stack_snapshot,
                                                );
                                                for val in snapshot {
                                                    self.push(Value::from_raw(val));
                                                }
                                                continue;
                                            }
                                            self.last_locals = jit_locals;
                                            self.push(Value::from_raw(result_raw));
                                            self.frames[fi].pc = pc + 1;
                                            continue;
                                    }
                                }
                                // --- End JIT tier-up ---
                                let func_ptr = ptr as *mut Func;
                                let func_env = unsafe { Func::env_ptr(func_ptr) };
                                let mut locals: Vec<Value> = if func_prog.named_function {
                                    vec![callee]
                                } else {
                                    vec![]
                                };
                                let passed_argc = args.len();
                                locals.extend(args);
                                self.frames.push(Frame {
                                    locals,
                                    lexical_slots: Vec::new(),
                                    lexical_tdz: Vec::new(),
                                    lexical_const: Vec::new(),
                                    scope_boundaries: Vec::new(),
                                    passed_argc,
                                    pc: 0,
                                    stack_base: self.stack.len(),
                                    prog: func_prog as *const BytecodeProgram,
                                    generator_id: None,
                                    this,
                                    is_constructor_call: false,
                                    constructed_object: Value::undefined(),
                                    env: func_env,
                                });
                                continue;
                            }
                        }
                    }
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::CallFromArray => {
                    let args_arr = self.pop();
                    let callee = self.pop();
                    let this = self.pop();
                    let argc = if let Some(ptr) = args_arr.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_ARRAY {
                            unsafe { RuneArray::length(ptr as *mut RuneArray) as usize }
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    let mut args: Vec<Value> = Vec::with_capacity(argc);
                    if let Some(ptr) = args_arr.heap_ptr() {
                        let arr_ptr = ptr as *mut RuneArray;
                        for i in 0..argc {
                            let v = unsafe { RuneArray::get_element(arr_ptr, i) };
                            args.push(v);
                        }
                    }

                    // Builtin dispatch: negative Smi handles
                    if let Some(smi_val) = callee.as_smi() {
                        if smi_val < 0 {
                            let id = ((-smi_val) as usize) - 1;
                            if id < self.builtins.len() {
                                let result = (self.builtins[id].func)(gc, this, &args, &mut *self);
                                if let Some(exc) = self.pending_exception.take() {
                                    self.push(exc);
                                    return Exit::Throw(exc);
                                }
                                self.push(result);
                                self.frames[fi].pc = pc + 1;
                                continue;
                            }
                        } else {
                            self.push(Value::undefined());
                            self.frames[fi].pc = pc + 1;
                            continue;
                        }
                    }

                    if let Some(ptr) = callee.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_FUNC {
                            let func_idx = unsafe { Func::func_index(ptr as *mut Func) } as usize;
                            let creator_prog = unsafe {
                                &*(Func::prog_ptr(ptr as *mut Func) as *const BytecodeProgram)
                            };
                            if func_idx < creator_prog.functions.len() {
                                let func_prog = &creator_prog.functions[func_idx];
                                if func_prog.is_generator {
                                    let g =
                                        Generator::new(args, func_prog as *const BytecodeProgram);
                                    let gen_id = self.generators.len();
                                    self.generators.push(g);
                                    self.push(Value::smi(gen_id as i32));
                                    self.frames[fi].pc = pc + 1;
                                    continue;
                                }
                                let func_ptr = ptr as *mut Func;
                                let func_env = unsafe { Func::env_ptr(func_ptr) };
                                let mut locals: Vec<Value> = if func_prog.named_function {
                                    vec![callee]
                                } else {
                                    vec![]
                                };
                                let passed_argc = args.len();
                                locals.extend(args);
                                self.frames.push(Frame {
                                    locals,
                                    lexical_slots: Vec::new(),
                                    lexical_tdz: Vec::new(),
                                    lexical_const: Vec::new(),
                                    scope_boundaries: Vec::new(),
                                    passed_argc,
                                    pc: 0,
                                    stack_base: self.stack.len(),
                                    prog: func_prog as *const BytecodeProgram,
                                    generator_id: None,
                                    this,
                                    is_constructor_call: false,
                                    constructed_object: Value::undefined(),
                                    env: func_env,
                                });
                                continue;
                            }
                        }
                    }
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Return => {
                    debug_assert!(
                        self.stack.len() > self.frames.last().unwrap().stack_base,
                        "Return: stack underflow (len={}, base={})",
                        self.stack.len(),
                        self.frames.last().unwrap().stack_base,
                    );
                    debug_assert!(
                        self.stack.len() <= self.frames.last().unwrap().stack_base + 2,
                        "Return: stack too deep (len={}, base={})",
                        self.stack.len(),
                        self.frames.last().unwrap().stack_base,
                    );
                    let result = self.pop();
                    let callee_base = self.frames.last().unwrap().stack_base;
                    let gen_id = self.frames.last().unwrap().generator_id;
                    if let Some(id) = gen_id {
                        self.generators[id].done = true;
                    }
                    let popped_frame = self.frames.len() - 1;
                    let is_constructor = self.frames[popped_frame].is_constructor_call;
                    let constructed_obj = self.frames[popped_frame].constructed_object;
                    self.last_locals = self.frames[popped_frame].locals.clone();
                    self.frames.pop();
                    self.try_stack.retain(|tf| tf.frame_depth != popped_frame);
                    if self.frames.is_empty() {
                        self.stack.clear();
                        return Exit::Return(result);
                    }
                    let new_fi = self.frames.len() - 1;
                    self.stack.truncate(callee_base);
                    // §11.2.2 [[Construct]]: if constructor returns a heap object, use it;
                    // otherwise use the originally constructed object.
                    if is_constructor {
                        if result.is_heap_object() {
                            self.push(result);
                        } else {
                            self.push(constructed_obj);
                        }
                    } else {
                        self.push(result);
                    }
                    self.frames[new_fi].pc += 1;
                }

                // ---- Generators ----
                Opcode::InitGenerator => {
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Yield => {
                    let val = self.pop();
                    if let Some(gen_id) = self.frames[fi].generator_id {
                        let g = &mut self.generators[gen_id];
                        g.locals = self.frames[fi].locals.clone();
                        g.lexical_slots = self.frames[fi].lexical_slots.clone();
                        g.lexical_tdz = self.frames[fi].lexical_tdz.clone();
                        g.lexical_const = self.frames[fi].lexical_const.clone();
                        g.scope_boundaries = self.frames[fi].scope_boundaries.clone();
                        g.pc = pc + 1;
                        g.prog = self.frames[fi].prog;
                        g.started = true;
                    }
                    let callee_base = self.frames.last().unwrap().stack_base;
                    let popped_frame = self.frames.len() - 1;
                    self.last_locals = self.frames[popped_frame].locals.clone();
                    self.frames.pop();
                    self.try_stack.retain(|tf| tf.frame_depth != popped_frame);
                    if self.frames.is_empty() {
                        self.stack.clear();
                        return Exit::Yield(val);
                    }
                    let new_fi = self.frames.len() - 1;
                    self.stack.truncate(callee_base);
                    self.push(val);
                    self.frames[new_fi].pc += 1;
                    return Exit::Yield(val);
                }
                Opcode::YieldStar => {
                    // Stub: return undefined (delegate yield not yet implemented)
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Resume => {
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
            }
        }

        let result = self.stack.pop().unwrap_or(Value::undefined());
        let saved_locals = self
            .frames
            .first()
            .map(|f| f.locals.clone())
            .unwrap_or_default();
        self.frames.clear();
        self.stack.clear();
        // Save locals for sync by execute()
        self.last_locals = saved_locals;
        Exit::Return(result)
    }

    pub fn push(&mut self, val: Value) {
        self.stack.push(val);
    }

    pub fn pop(&mut self) -> Value {
        self.stack.pop().unwrap_or(Value::undefined())
    }

    pub fn peek(&self) -> Value {
        self.stack.last().copied().unwrap_or(Value::undefined())
    }

    /// Update all root references from `old_ptr` to `new_ptr` after a heap object
    /// has been relocated (e.g., array grow reallocation).
    /// Scans stack, all frame locals, and globals for matching heap pointers.
    pub fn update_heap_reference(&mut self, old_ptr: *mut u8, new_ptr: *mut u8) {
        for v in &mut self.stack {
            if let Some(p) = v.heap_ptr()
                && p == old_ptr
            {
                *v = Value::from_heap_ptr(new_ptr);
            }
        }
        for frame in &mut self.frames {
            for v in &mut frame.locals {
                if let Some(p) = v.heap_ptr()
                    && p == old_ptr
                {
                    *v = Value::from_heap_ptr(new_ptr);
                }
            }
            // Also update env object slots (the GC-managed EnvObject)
            if !frame.env.is_null() {
                let env_ptr = frame.env;
                unsafe {
                    let slot_count = *(env_ptr.add(8) as *const u32) as usize;
                    let slots = env_ptr.add(24) as *mut Value;
                    for i in 0..slot_count {
                        let slot = &mut *slots.add(i);
                        if let Some(p) = slot.heap_ptr()
                            && p == old_ptr
                        {
                            *slot = Value::from_heap_ptr(new_ptr);
                        }
                    }
                }
            }
        }
        for v in self.globals.values_mut() {
            if let Some(p) = v.heap_ptr()
                && p == old_ptr
            {
                *v = Value::from_heap_ptr(new_ptr);
            }
        }
    }
}

impl RootProvider for Vm {
    fn register_roots(&mut self, gc: &mut SemiSpace) {
        gc.clear_roots();
        self.register_roots(gc);
    }
}

impl Vm {
    /// Return a summary of IC hit/miss statistics.
    pub fn dump_ic_stats(&self) -> String {
        let total = self.ic_stats.hits + self.ic_stats.misses;
        let hit_pct = if total > 0 {
            (self.ic_stats.hits as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        format!(
            "IC stats: {} lookups, {} hits, {} misses ({:.1}% hit rate)",
            self.ic_stats.lookups, self.ic_stats.hits, self.ic_stats.misses, hit_pct
        )
    }

    /// Return a summary of loop hotness and recorded traces (for --trace-stats).
    pub fn dump_trace_stats(&self) -> String {
        if self.loop_counts.is_empty() {
            return "Trace stats: no loops detected.".to_string();
        }
        let mut lines = vec![format!(
            "Trace stats: {} loop(s) detected",
            self.loop_counts.len()
        )];
        for (target, count) in self.loop_counts.iter() {
            let label = if *count >= 50 { "HOT" } else { "warm" };
            lines.push(format!(
                "  pc={} → {} iterations ({})",
                target, count, label
            ));
            if let Some(trace) = self.loop_traces.get(target) {
                let mono = if trace.is_monomorphic() {
                    "MONO (1 shape)"
                } else {
                    "POLY"
                };
                let icost = trace.estimated_interpreter_cost();
                let ncost = trace.estimated_native_cost();
                let speedup = if ncost > 0 {
                    (icost as f64 / ncost as f64) as u32
                } else {
                    0
                };
                lines.push(format!(
                    "    trace: {} ops, {} shapes ({})",
                    trace.ops.len(),
                    trace.shape_ids.len(),
                    mono
                ));
                lines.push(format!(
                    "    estimated speedup: {}→{} instrs ≈ {}×",
                    icost,
                    ncost,
                    speedup.max(1)
                ));
            }
        }
        lines.join("\n")
    }

    /// Patch LoadProperty instructions in a hot monomorphic loop to
    /// LoadPropertyIC with cached IC values, eliminating IC lookup overhead.
    /// Compile a recorded loop trace to native AArch64 code.
    /// The trace is compiled as a self-contained loop: the back-edge Jump is
    /// remapped to loop back to the first instruction, and JumpIfFalse is
    /// remapped to exit the trace.  The interpreter never enters the compiled
    /// code — it runs until the loop condition is false, then returns.
    #[cfg(target_arch = "aarch64")]
    fn compile_trace_native(&mut self, target_pc: usize) {
        use rune_jit_baseline::Aarch64CodeGen;
        use rune_bytecode::opcode::{BytecodeProgram, Instruction};

        let trace = match self.loop_traces.get_mut(&target_pc) {
            Some(t) => t,
            None => return,
        };
        let mut instrs: Vec<Instruction> = Vec::with_capacity(trace.ops.len() + 2);
        let mut exit_pc: usize = 0;
        // The last recorded op is the first instruction of the iteration
        // that triggered the recording stop — it's a duplicate of op 0.
        let ops_slice = if trace.ops.len() > 1
            && trace.ops.first().map(|t| t.opcode) == trace.ops.last().map(|t| t.opcode)
        {
            &trace.ops[..trace.ops.len() - 1]
        } else {
            &trace.ops[..]
        };
        for t in ops_slice {
            let opcode: Opcode = unsafe { std::mem::transmute(t.opcode) };
            let mut operands = t.operands.clone();
            // Remap branch targets from original bytecode indices to in-trace
            // indices.
            match opcode {
                Opcode::Jump | Opcode::JumpIfTrue | Opcode::JumpIfFalse => {
                    let orig_target = operands.first().copied().unwrap_or(0) as usize;
                    if opcode == Opcode::JumpIfFalse && exit_pc == 0 {
                        exit_pc = orig_target;
                    }
                    if orig_target == target_pc {
                        // Back-edge → branch to trace start (loop)
                        operands[0] = 0;
                    } else if orig_target > target_pc {
                        // Forward branch → target is past the end of our trace
                        // (exit path). Point to a trailing Return instruction.
                        operands[0] = -1; // will be replaced with actual return index
                    } else {
                        // Other backward branch (unlikely in a simple loop).
                        // Keep as-is; will be within the trace body.
                    }
                    // Store the position that needs exit-target patching
                }
                _ => {}
            }
            instrs.push(Instruction::new(opcode, operands));
        }

        if instrs.is_empty() {
            return;
        }

        // Patch forward-branch targets to point past the last instruction.
        // Also add a Return at the end so the trace exits cleanly.
        let return_index = instrs.len();
        for instr in &mut instrs {
            if matches!(instr.opcode, Opcode::Jump | Opcode::JumpIfTrue | Opcode::JumpIfFalse)
                && instr.operands.first().copied() == Some(-1)
            {
                instr.operands[0] = return_index as i64;
            }
        }
        instrs.push(Instruction::new(Opcode::LoadUndefined, vec![]));
        instrs.push(Instruction::new(Opcode::Return, vec![]));

        let prog = BytecodeProgram::new(instrs, vec![], vec![]);
        if !rune_jit_baseline::is_jit_compatible(&prog) {
            return; // trace contains unsupported opcodes (strings, objects, etc.)
        }
        // Trace compiler doesn't support global opcodes yet.
        for instr in &prog.instructions {
            match instr.opcode {
                Opcode::LoadGlobal | Opcode::StoreGlobal | Opcode::IncGlobal | Opcode::DecGlobal => {
                    return;
                }
                _ => {}
            }
        }

        let codegen = Aarch64CodeGen::new(prog.instructions.len());
        let compiled = codegen.compile(&prog);
        compiled.mem.make_executable();
        let entry = compiled.mem.code_ptr();
        trace.compiled_entry = entry;
        trace.exit_pc = exit_pc;
        self._compiled_trace_mem.push(compiled.mem);
        eprintln!("Trace: compiled loop pc={} with {} ops → native", target_pc, prog.instructions.len());
    }

    /// Call a compiled loop trace. Returns the raw u64 result (unused for
    /// loop traces — the locals are updated in-place by the trace).
    unsafe fn execute_trace(&mut self, fi: usize, entry: *const u8, gc_ptr: *mut u8) -> u64 {
        let func: rune_jit_baseline::JitEntryFn = unsafe { std::mem::transmute(entry) };
        let locals = self.frames[fi].locals.as_mut_ptr() as *mut u64;
        unsafe {
            func(
                self as *mut Vm as *mut u8,
                gc_ptr,
                locals,
            )
        }
    }

    unsafe fn patch_loop_body(
        &mut self,
        prog_ptr: *const BytecodeProgram,
        target_pc: usize,
        back_edge_pc: usize,
    ) {
        if self.loop_patched.contains(&target_pc) {
            return;
        }
        let trace = match self.loop_traces.get(&target_pc) {
            Some(t) if t.is_monomorphic() => t,
            _ => return,
        };
        let shape_id = trace.shape_ids.first().copied().unwrap_or(0);
        if shape_id == 0 {
            return;
        }

        let mut patched = 0u32;
        for pc in target_pc..=back_edge_pc {
            let instr_ptr = unsafe {
                let instrs = (*prog_ptr).instructions.as_ptr() as *mut Instruction;
                &mut *instrs.add(pc)
            };
            if instr_ptr.opcode == Opcode::LoadProperty && instr_ptr.ic_index >= 0 {
                let ic_idx = instr_ptr.ic_index as usize;
                if ic_idx < self.ics.len() {
                    // Find the IC entry matching the trace's monomorphic shape_id
                    for (key, entry) in &self.ics[ic_idx].entries {
                        if key.shape_id == shape_id {
                            instr_ptr.opcode = Opcode::LoadPropertyIC;
                            instr_ptr.operands.clear();
                            instr_ptr.operands.extend_from_slice(&[
                                shape_id as i64,
                                entry.offset as i64,
                                entry.proto_depth as i64,
                            ]);
                            patched += 1;
                            break;
                        }
                    }
                }
            }
        }

        if patched > 0 {
            eprintln!(
                "Trace: patched {} LoadProperty → LoadPropertyIC in loop pc={}..{} (shape={})",
                patched, target_pc, back_edge_pc, shape_id
            );
        } else {
            eprintln!(
                "Trace: loop pc={}..{} already LoadPropertyIC (shape={})",
                target_pc, back_edge_pc, shape_id
            );
        }
        self.loop_patched.insert(target_pc);
    }
}

/// Allocate a GC-managed string and return it as a raw pointer (for builtins).
pub fn heap_string(gc: &mut SemiSpace, s: &str) -> *mut u8 {
    HeapString::allocate(gc, s) as *mut u8
}

impl Frame {
    fn prog_str(&self, idx: usize) -> Option<String> {
        let prog = unsafe { &*self.prog };
        prog.string_pool.get(idx).cloned()
    }
}

/// Per §7.2.14 IsStrictlyEqual.
/// §7.2.13 Abstract Equality Comparison.
/// Returns true if `a == b` per the spec.
fn values_loosely_equal(a: Value, b: Value) -> bool {
    // Same type or same-heap-tag → strict equality
    if a.is_boolean() && b.is_boolean() {
        return a == b;
    }
    if (a.is_smi() || a.is_float64()) && (b.is_smi() || b.is_float64()) {
        return values_strictly_equal(a, b);
    }
    // null == undefined → true (and vice versa)
    if (a.is_null() && b.is_undefined()) || (a.is_undefined() && b.is_null()) {
        return true;
    }
    // §7.2.13 step 6-7: Boolean → ToNumber(b), then compare
    if a.is_boolean() {
        return values_loosely_equal(
            if a.to_boolean() == Some(true) {
                Value::smi(1)
            } else {
                Value::smi(0)
            },
            b,
        );
    }
    if b.is_boolean() {
        return values_loosely_equal(
            a,
            if b.to_boolean() == Some(true) {
                Value::smi(1)
            } else {
                Value::smi(0)
            },
        );
    }
    // §7.2.13 step 8-9: Number vs String → compare ToNumber(string) with number
    let (num_val, str_val) = if (a.is_smi() || a.is_float64()) && values_is_string(b) {
        (a, b)
    } else if (b.is_smi() || b.is_float64()) && values_is_string(a) {
        (b, a)
    } else {
        // §7.2.13 step 10: Object vs String/Number/Symbol → ToPrimitive (deferred)
        // Fall back to strict equality for now.
        return values_strictly_equal(a, b);
    };
    let na = value_to_f64(num_val);
    let nb = to_number(str_val);
    if na.is_nan() || nb.is_nan() {
        return false;
    }
    if na == 0.0 && nb == 0.0 {
        // +0 === -0 per loose equality too
        return true;
    }
    na == nb
}

fn values_is_string(v: Value) -> bool {
    if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        tag == TAG_STRING
    } else {
        false
    }
}

/// Extract f64 from a value known to be numeric (Smi or Float64).
fn value_to_f64(v: Value) -> f64 {
    if let Some(n) = v.as_smi() {
        n as f64
    } else {
        v.as_float64().unwrap_or(f64::NAN)
    }
}

fn values_strictly_equal(a: Value, b: Value) -> bool {
    // Both are Number type (Smi or Float64)
    if a.is_smi() || b.is_smi() || a.is_float64() || b.is_float64() {
        let na = if a.is_smi() {
            a.as_smi().map(|s| s as f64)
        } else {
            a.as_float64()
        };
        let nb = if b.is_smi() {
            b.as_smi().map(|s| s as f64)
        } else {
            b.as_float64()
        };
        if let (Some(av), Some(bv)) = (na, nb) {
            if av.is_nan() || bv.is_nan() {
                return false;
            }
            return av == bv;
        }
        return false;
    }
    // Same raw value (same Smi or same heap pointer)
    if a == b {
        return true;
    }
    // String content comparison
    if let (Some(pa), Some(pb)) = (a.heap_ptr(), b.heap_ptr()) {
        let ta = unsafe { (*(pa as *const GcHeader)).tag() };
        let tb = unsafe { (*(pb as *const GcHeader)).tag() };
        if ta == TAG_STRING && tb == TAG_STRING {
            let la = unsafe { HeapString::len(pa as *mut HeapString) };
            let lb = unsafe { HeapString::len(pb as *mut HeapString) };
            if la != lb {
                return false;
            }
            let da = unsafe { HeapString::data(pa as *mut HeapString) };
            let db = unsafe { HeapString::data(pb as *mut HeapString) };
            for i in 0..la {
                if unsafe { *da.add(i) != *db.add(i) } {
                    return false;
                }
            }
            return true;
        }
    }
    false
}

/// Compare two values as strings for IsLessThan semantics.
/// Returns None if either value is not a string.
fn compare_strings_lt(a: Value, b: Value) -> Option<bool> {
    if let (Some(pa), Some(pb)) = (a.heap_ptr(), b.heap_ptr()) {
        let ta = unsafe { (*(pa as *const GcHeader)).tag() };
        let tb = unsafe { (*(pb as *const GcHeader)).tag() };
        if ta == TAG_STRING && tb == TAG_STRING {
            let la = unsafe { HeapString::len(pa as *mut HeapString) };
            let lb = unsafe { HeapString::len(pb as *mut HeapString) };
            let da = unsafe { HeapString::data(pa as *mut HeapString) };
            let db = unsafe { HeapString::data(pb as *mut HeapString) };
            let min_len = la.min(lb);
            for i in 0..min_len {
                let ca = unsafe { *da.add(i) };
                let cb = unsafe { *db.add(i) };
                if ca < cb {
                    return Some(true);
                }
                if ca > cb {
                    return Some(false);
                }
            }
            return Some(la < lb);
        }
    }
    None
}

fn value_to_debug_string(val: Value) -> String {
    if val.is_undefined() {
        "undefined".to_string()
    } else if val.is_null() {
        "null".to_string()
    } else if let Some(v) = val.as_smi() {
        v.to_string()
    } else if let Some(v) = val.as_float64() {
        v.to_string()
    } else if let Some(ptr) = val.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            unsafe { HeapString::to_string(ptr as *mut HeapString) }
        } else {
            "[object Object]".to_string()
        }
    } else {
        "undefined".to_string()
    }
}

fn value_to_prop_key(val: Value) -> Option<PropertyKey> {
    if let Some(ptr) = val.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            return Some(PropertyKey::from_string(&s));
        }
    }
    if let Some(v) = val.as_smi() {
        return Some(PropertyKey::from_string(&v.to_string()));
    }
    None
}

/// Check if a Value is the string `"__proto__"` (the special prototype setter key).
fn is_proto_key(val: Value) -> bool {
    if let Some(ptr) = val.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            return s == "__proto__";
        }
    }
    false
}

/// Maximum depth to walk the prototype chain before giving up (cycle guard).
const MAX_PROTOTYPE_DEPTH: usize = 256;

/// Walk the prototype chain to resolve a property.
/// Implements OrdinaryGet (§10.1.8.1): check own property, then recurse on [[Prototype]].
/// For dense arrays: numeric keys access elements directly; non-numeric walks to prototype.
/// Returns undefined if the chain exceeds MAX_PROTOTYPE_DEPTH (prevents infinite loops on cycles).
fn load_property_recursive(obj: Value, raw_key: Value) -> Value {
    let mut current = obj;
    let mut depth = 0;
    loop {
        if depth >= MAX_PROTOTYPE_DEPTH {
            return Value::undefined();
        }
        depth += 1;
        if let Some(ptr) = current.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_OBJECT {
                if let Some(key) = value_to_prop_key(raw_key) {
                    let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                    if let Some(slot) = shape.lookup(&key) {
                        return unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                    }
                    // Not found — walk to prototype
                    let proto = unsafe { JSObject::prototype(ptr as *mut JSObject) };
                    if proto.is_null() {
                        return Value::undefined();
                    }
                    current = Value::from_heap_ptr(proto);
                    continue;
                } else {
                    return Value::undefined();
                }
            } else if tag == TAG_ARRAY {
                // Dense array: numeric key → direct element access
                if let Some(index) = value_to_array_index(raw_key) {
                    let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
                    if index < len as usize {
                        return unsafe { RuneArray::get_element(ptr as *mut RuneArray, index) };
                    }
                    return Value::undefined(); // out of bounds
                }
                // "length" property → return stored length
                if let Some(key_ptr) = raw_key.heap_ptr() {
                    let key_tag = unsafe { (*(key_ptr as *const GcHeader)).tag() };
                    if key_tag == TAG_STRING {
                        let key_str = unsafe { HeapString::to_string(key_ptr as *mut HeapString) };
                        if key_str == "length" {
                            let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
                            return Value::smi(len as i32);
                        }
                    }
                }
                // Non-numeric key → walk to prototype
                let proto = unsafe { JSObject::prototype(ptr as *mut JSObject) };
                if proto.is_null() {
                    return Value::undefined();
                }
                current = Value::from_heap_ptr(proto);
                continue;
            } else if tag == TAG_FUNC {
                if let Some(key) = value_to_prop_key(raw_key)
                    && key == *PROTOTYPE_KEY
                {
                    let proto_ptr = unsafe { Func::prototype(ptr as *mut Func) };
                    if !proto_ptr.is_null() {
                        return Value::from_heap_ptr(proto_ptr);
                    }
                }
                return Value::undefined();
            }
        }
        return Value::undefined();
    }
}

/// Full property lookup that populates the inline cache on miss.
#[allow(clippy::too_many_arguments)] // several distinct mutable VM subsystems are required
fn load_property_recursive_ic(
    _gc: &mut SemiSpace,
    ics: &mut Vec<InlineCache>,
    ic_entries: &mut Vec<IcEntry>,
    ic_hit_counts: &mut Vec<u32>,
    ic_stats: &mut IcStats,
    instr: &Instruction,
    obj: Value,
    raw_key: Value,
) -> Value {
    // Check IC first before doing full lookup
    if instr.ic_index >= 0
        && let Some(ptr) = obj.heap_ptr()
    {
        let ic_idx = instr.ic_index as usize;
        if ic_idx < ics.len() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_OBJECT {
                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                let (shape_id, key_hash) = ic_cache_key(shape.id, raw_key);
                if let Some(entry) = ics[ic_idx].get(shape_id, key_hash) {
                    ic_stats.hits += 1;
                    if entry.is_own {
                        unsafe {
                            return JSObject::get_slot(ptr as *mut JSObject, entry.offset);
                        }
                    } else {
                        let mut p = ptr;
                        for _ in 0..entry.proto_depth {
                            let next = unsafe { JSObject::prototype(p as *mut JSObject) };
                            if next.is_null() {
                                break;
                            }
                            p = next;
                        }
                        unsafe {
                            return JSObject::get_slot(p as *mut JSObject, entry.offset);
                        }
                    }
                }
            }
        }
    }

    let result = load_property_recursive(obj, raw_key);
    // Populate IC for all result types
    if instr.ic_index >= 0
        && let Some(ptr) = obj.heap_ptr()
    {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        let ic_idx = instr.ic_index as usize;
        while ics.len() <= ic_idx {
            ics.push(InlineCache::new());
            ic_entries.push(IcEntry::default());
            ic_hit_counts.push(0);
        }
        if tag == TAG_OBJECT {
            if let Some(key) = value_to_prop_key(raw_key) {
                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                let (shape_id, key_hash) = ic_cache_key(shape.id, raw_key);
                if let Some(offset) = shape.lookup(&key) {
                    // Own property
                    ics[ic_idx].insert(
                        shape_id,
                        key_hash,
                        IcEntry {
                            offset,
                            is_own: true,
                            proto_depth: 0,
                        },
                    );
                } else {
                    // Inherited — walk prototype chain to find offset and depth
                    let mut depth: u8 = 0;
                    let mut p = ptr;
                    loop {
                        let next = unsafe { JSObject::prototype(p as *mut JSObject) };
                        if next.is_null() {
                            break;
                        }
                        depth += 1;
                        if depth >= MAX_PROTOTYPE_DEPTH as u8 {
                            break;
                        }
                        let next_shape = unsafe { JSObject::shape_ptr(next as *mut JSObject) };
                        if let Some(offset) = next_shape.lookup(&key) {
                            ics[ic_idx].insert(
                                shape_id,
                                key_hash,
                                IcEntry {
                                    offset,
                                    is_own: false,
                                    proto_depth: depth,
                                },
                            );
                            break;
                        }
                        p = next;
                    }
                }
            }
        } else if tag == TAG_ARRAY {
            // Dense array IC: numeric keys cache element index directly
            if let Some(index) = value_to_array_index(raw_key) {
                let (shape_id, key_hash) = ic_cache_key(DENSE_ARRAY_SHAPE.id, raw_key);
                ics[ic_idx].insert(
                    shape_id,
                    key_hash,
                    IcEntry {
                        offset: index,
                        is_own: true,
                        proto_depth: 0,
                    },
                );
            } else if let Some(key) = value_to_prop_key(raw_key) {
                // Non-numeric key — inherited from Array.prototype
                let (shape_id, key_hash) = ic_cache_key(DENSE_ARRAY_SHAPE.id, raw_key);
                let mut depth: u8 = 0;
                let mut p = ptr;
                loop {
                    let next = unsafe { JSObject::prototype(p as *mut JSObject) };
                    if next.is_null() {
                        break;
                    }
                    depth += 1;
                    if depth >= MAX_PROTOTYPE_DEPTH as u8 {
                        break;
                    }
                    let next_shape = unsafe { JSObject::shape_ptr(next as *mut JSObject) };
                    if let Some(offset) = next_shape.lookup(&key) {
                        ics[ic_idx].insert(
                            shape_id,
                            key_hash,
                            IcEntry {
                                offset,
                                is_own: false,
                                proto_depth: depth,
                            },
                        );
                        break;
                    }
                    p = next;
                }
            }
        }
    }
    result
}

/// Perform the full store-property logic (modelled after StoreProperty handler body).
fn do_store_property(obj: Value, raw_key: Value, value: Value) {
    if let Some(ptr) = obj.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_OBJECT {
            if is_proto_key(raw_key) {
                if let Some(val_ptr) = value.heap_ptr() {
                    unsafe { JSObject::set_prototype(ptr as *mut JSObject, val_ptr) };
                } else {
                    unsafe { JSObject::set_prototype(ptr as *mut JSObject, std::ptr::null_mut()) };
                }
            } else if let Some(key) = value_to_prop_key(raw_key) {
                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                if let Some(slot) = shape.lookup(&key) {
                    unsafe { JSObject::set_slot(ptr as *mut JSObject, slot, value) };
                } else {
                    let key_name = value_to_debug_string(raw_key);
                    unsafe { JSObject::add_property(ptr as *mut JSObject, key, key_name, value) };
                }
            }
        } else if tag == TAG_ARRAY {
            if let Some(index) = value_to_array_index(raw_key) {
                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
                if index < len as usize {
                    unsafe { RuneArray::set_element(ptr as *mut RuneArray, index, value) };
                }
            }
        } else if tag == TAG_FUNC
            && let Some(key) = value_to_prop_key(raw_key)
            && key == *PROTOTYPE_KEY
            && let Some(val_ptr) = value.heap_ptr()
        {
            unsafe { Func::set_prototype(ptr as *mut Func, val_ptr); }
        }
    }
}

/// Convert a Value to an f64 for numeric operations.
/// Returns NaN for non-numeric types (undefined, null, objects, strings).
fn to_number(v: Value) -> f64 {
    if let Some(n) = v.as_smi() {
        n as f64
    } else if let Some(n) = v.as_float64() {
        n
    } else if v.is_null() {
        0.0
    } else if v.is_undefined() {
        f64::NAN
    } else if v.is_boolean() {
        // §7.1.4: ToNumber(Boolean) — true → 1, false → 0
        if v.to_boolean() == Some(true) {
            1.0
        } else {
            0.0
        }
    } else if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return 0.0;
            }
            if let Ok(n) = trimmed.parse::<f64>() {
                return n;
            }
            // Hex literals like "0x1F"
            let upper = trimmed.to_uppercase();
            if upper.starts_with("0X")
                && let Ok(n) = u64::from_str_radix(&upper[2..], 16)
            {
                return n as f64;
            }
            // Infinity
            if trimmed.eq_ignore_ascii_case("infinity")
                || trimmed == "+Infinity"
                || trimmed == "-Infinity"
            {
                return if trimmed.starts_with('-') {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                };
            }
            f64::NAN
        } else {
            f64::NAN
        }
    } else {
        f64::NAN
    }
}

/// §7.1.6 ToInt32: Convert a Value to a signed 32-bit integer.
fn to_int32(v: Value) -> i32 {
    let n = to_number(v);
    if n.is_nan() || n.is_infinite() {
        return 0;
    }
    // Truncate toward zero
    let int = n.trunc();
    // Mod 2^32 (positive)
    let int32bit = int.rem_euclid(4294967296.0);
    // If ≥ 2^31, wrap to negative
    if int32bit >= 2147483648.0 {
        (int32bit - 4294967296.0) as i32
    } else {
        int32bit as i32
    }
}

/// Wrap an f64 result back into a Value, trying to use Smi for small integers.
fn number_result(gc: &mut SemiSpace, val: f64) -> Value {
    if val.is_nan() || val.is_infinite() {
        let ptr = HeapFloat64::allocate(gc, val);
        return Value::from_float64_ptr(ptr as *mut u8);
    }
    if val.fract() == 0.0 {
        // Preserve -0.0 as HeapFloat64; Smi would lose the sign bit
        if val == 0.0 && val.is_sign_negative() {
            let ptr = HeapFloat64::allocate(gc, val);
            return Value::from_float64_ptr(ptr as *mut u8);
        }
        let i = val as i64;
        if i >= -(1 << 30) as i64 && i < (1 << 30) as i64 {
            return Value::smi(val as i32);
        }
    }
    // TODO Phase 5: Replace HeapFloat64 with NaN-boxing for zero-allocation arithmetic
    let ptr = HeapFloat64::allocate(gc, val);
    Value::from_float64_ptr(ptr as *mut u8)
}

/// Compute the IC cache key combining shape.id with the property key,
/// so that different keys on the same shape produce distinct cache entries.
fn ic_cache_key(shape_id: u64, raw_key: Value) -> (u64, u64) {
    if let Some(idx) = value_to_array_index(raw_key) {
        (shape_id, idx as u64)
    } else if let Some(key) = value_to_prop_key(raw_key) {
        (shape_id, key.as_u64())
    } else {
        (shape_id, 0)
    }
}

/// Check if an object has a property (for the `in` operator).
/// Returns false for non-object values (primitives are not objects).
fn has_property(obj: Value, raw_key: Value) -> bool {
    if let Some(ptr) = obj.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_OBJECT {
            if let Some(key) = value_to_prop_key(raw_key) {
                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                if shape.lookup(&key).is_some() {
                    return true;
                }
                // Walk prototype chain
                let mut current = obj;
                let mut depth = 0;
                loop {
                    if depth >= 5_000 {
                        return false;
                    }
                    depth += 1;
                    let cur_ptr = current.heap_ptr().unwrap();
                    let cur_tag = unsafe { (*(cur_ptr as *const GcHeader)).tag() };
                    if cur_tag == TAG_OBJECT {
                        let proto = unsafe { JSObject::prototype(cur_ptr as *mut JSObject) };
                        if proto.is_null() {
                            return false;
                        }
                        current = Value::from_heap_ptr(proto);
                        if let Some(proto_ptr) = current.heap_ptr() {
                            let proto_tag = unsafe { (*(proto_ptr as *const GcHeader)).tag() };
                            if proto_tag == TAG_OBJECT {
                                let proto_shape =
                                    unsafe { JSObject::shape_ptr(proto_ptr as *mut JSObject) };
                                if proto_shape.lookup(&key).is_some() {
                                    return true;
                                }
                            } else {
                                return false;
                            }
                        } else {
                            return false;
                        }
                    } else {
                        return false;
                    }
                }
            }
            false
        } else if tag == TAG_ARRAY {
            if let Some(index) = value_to_array_index(raw_key) {
                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
                return index < len as usize;
            }
            if let Some(key_ptr) = raw_key.heap_ptr() {
                let key_tag = unsafe { (*(key_ptr as *const GcHeader)).tag() };
                if key_tag == TAG_STRING {
                    let key_str = unsafe { HeapString::to_string(key_ptr as *mut HeapString) };
                    if key_str == "length" {
                        return true;
                    }
                }
            }
            // Walk prototype chain for non-numeric keys on arrays
            has_property(
                unsafe {
                    let proto = JSObject::prototype(ptr as *mut JSObject);
                    if proto.is_null() {
                        return false;
                    }
                    Value::from_heap_ptr(proto)
                },
                raw_key,
            )
        } else if tag == TAG_FUNC {
            if let Some(key) = value_to_prop_key(raw_key)
                && key == *PROTOTYPE_KEY
            {
                return true;
            }
            false
        } else if tag == TAG_STRING {
            if let Some(index) = value_to_array_index(raw_key) {
                let len = unsafe { HeapString::len(ptr as *mut HeapString) };
                return index < len;
            }
            // Walk String.prototype for non-numeric keys
            false
        } else {
            false
        }
    } else {
        false
    }
}

/// OrdinaryHasInstance per §13.10.2: walk lhs prototype chain looking for rhs_proto.
fn ordinary_has_instance(lhs: Value, rhs_proto_ptr: *mut u8) -> bool {
    let mut current = lhs;
    let mut depth = 0;
    loop {
        if depth >= MAX_PROTOTYPE_DEPTH {
            return false;
        }
        depth += 1;
        if let Some(ptr) = current.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            let proto = if tag == TAG_OBJECT || tag == TAG_ARRAY {
                unsafe { JSObject::prototype(ptr as *mut JSObject) }
            } else {
                return false;
            };
            if proto.is_null() {
                return false;
            }
            if proto == rhs_proto_ptr {
                return true;
            }
            current = Value::from_heap_ptr(proto);
        } else {
            return false;
        }
    }
}

/// Check if a Value is a GC-allocated string.
fn value_is_string(v: Value) -> bool {
    if let Some(ptr) = v.heap_ptr() {
        unsafe { (*(ptr as *const GcHeader)).tag() == TAG_STRING }
    } else {
        false
    }
}

/// Convert a Value to an array index if it is a non-negative Smi.
fn value_to_array_index(v: Value) -> Option<usize> {
    if let Some(n) = v.as_smi() {
        if n >= 0 { Some(n as usize) } else { None }
    } else if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            // Only parse canonical numeric strings to avoid surprises
            s.parse::<usize>().ok()
        } else {
            None
        }
    } else {
        None
    }
}

/// Lexical operation codes for the JIT callout helper.
const LEX_BLOCK_ENTER: u64 = 0;
const LEX_BLOCK_LEAVE: u64 = 1;
const LEX_DECLARE_LET: u64 = 2;
const LEX_DECLARE_CONST: u64 = 3;
const LEX_LOAD: u64 = 4;
const LEX_STORE: u64 = 5;
const LEX_LOAD_THIS: u64 = 6;

/// JIT callout for all lexical-scope operations.
/// Called from JIT-compiled code via the `lexical_helper` function pointer
/// stored in `Vm::jit_helpers`.
/// Returns 0 for most ops; returns the loaded Value for LEX_LOAD.
#[unsafe(no_mangle)]
pub extern "C" fn rune_jit_lexical_helper(
    vm_ptr: *mut u8,
    op: u64,
    arg1: u64,
    arg2: u64,
) -> u64 {
    let vm = unsafe { &mut *(vm_ptr as *mut Vm) };
    let fi = vm.frames.len() - 1;
    let f = &mut vm.frames[fi];
    match op {
        LEX_BLOCK_ENTER => {
            let count = arg1 as usize;
            for _ in 0..count {
                f.lexical_slots.push(Value::undefined());
            }
            f.lexical_tdz.extend(std::iter::repeat_n(true, count));
            f.lexical_const.extend(std::iter::repeat_n(false, count));
            f.scope_boundaries.push(f.lexical_slots.len());
            0
        }
        LEX_BLOCK_LEAVE => {
            let boundary = f.scope_boundaries.pop().unwrap_or(0);
            f.lexical_slots.truncate(boundary);
            f.lexical_tdz.truncate(boundary);
            f.lexical_const.truncate(boundary);
            0
        }
        LEX_DECLARE_LET => {
            let slot = arg1 as usize;
            if slot < f.lexical_tdz.len() {
                f.lexical_tdz[slot] = false;
            }
            0
        }
        LEX_DECLARE_CONST => {
            let slot = arg1 as usize;
            if slot < f.lexical_tdz.len() {
                f.lexical_tdz[slot] = false;
                f.lexical_const[slot] = true;
            }
            0
        }
        LEX_LOAD => {
            let slot = arg1 as usize;
            if slot < f.lexical_slots.len() {
                if f.lexical_tdz[slot] {
                    return Value::undefined().raw();
                }
                return f.lexical_slots[slot].raw();
            }
            Value::undefined().raw()
        }
        LEX_STORE => {
            let slot = arg1 as usize;
            let val = Value::from_raw(arg2);
            if slot < f.lexical_slots.len() && !f.lexical_const[slot] {
                f.lexical_slots[slot] = val;
            }
            val.raw()
        }
        LEX_LOAD_THIS => {
            f.this.raw()
        }
        _ => 0,
    }
}

/// Bailout helper called from JIT code when a guard fails.
///
/// Snapshots the JIT value stack and records the bailout PC so the
/// `vm.rs` call site can materialise interpreter state after the JIT
/// function returns.
///
/// # Safety
///
/// `vm_ptr` must be a valid pointer to a `Vm`. `jit_sp` must point into
/// the JIT value stack (between `vm.jit_stack_base` and the current top).
pub extern "C" fn rune_jit_bailout_helper(
    vm_ptr: *mut u8,
    bc_pc: usize,
    jit_sp: *mut u64,
) -> u64 {
    let vm = unsafe { &mut *(vm_ptr as *mut Vm) };
    vm.jit_bailout_count += 1;
    let base = vm.jit_stack_base as usize;
    let current = jit_sp as usize;
    let count = if current >= base {
        (current - base) / 8
    } else {
        0
    };
    let base_ptr = base as *const u64;
    let mut snapshot = Vec::with_capacity(count);
    for i in 0..count {
        snapshot.push(unsafe { *base_ptr.add(i) });
    }
    vm.jit_bailout = JitBailoutState {
        bc_pc,
        pending: true,
        stack_snapshot: snapshot,
        reason: rune_jit_baseline::BailoutReason::BailOnEntry,
    };
    0
}

/// Indices into Vm::typeof_strings for each typeof result.
const TYPEOF_NUMBER: usize = 0;
const TYPEOF_STRING: usize = 1;
const TYPEOF_BOOLEAN: usize = 2;
const TYPEOF_UNDEFINED: usize = 3;
const TYPEOF_OBJECT: usize = 4;
const TYPEOF_FUNCTION: usize = 5;

/// JIT callout for `typeof` operator.
///
/// Takes a raw Value, returns the pre-allocated string Value corresponding
/// to the ECMAScript `typeof` result. Reads from `Vm::typeof_strings`.
///
/// # Safety
///
/// `vm_ptr` must be a valid pointer to a `Vm`. `value_raw` is a raw Value u64.
pub extern "C" fn rune_jit_typeof_helper(vm_ptr: *mut u8, value_raw: u64) -> u64 {
    let vm = unsafe { &*(vm_ptr as *mut Vm) };
    let val = Value::from_raw(value_raw);
    let idx = if val.is_undefined() {
        TYPEOF_UNDEFINED
    } else if val.is_null() {
        TYPEOF_OBJECT
    } else if val.is_boolean() {
        TYPEOF_BOOLEAN
    } else if val.is_smi() {
        TYPEOF_NUMBER
    } else {
        let ptr = val.raw() as *const GcHeader;
        let tag = unsafe { (*ptr).tag() };
        match tag {
            TAG_STRING => TYPEOF_STRING,
            TAG_FUNC => TYPEOF_FUNCTION,
            TAG_FLOAT64 => TYPEOF_NUMBER,
            _ => TYPEOF_OBJECT,
        }
    };
    vm.typeof_strings[idx].raw()
}

/// JIT callout for `LoadStringConst`.
///
/// Looks up the pre-allocated string handle from `Vm::string_cache[prog_ptr][idx]`.
/// If the cache entry is cold (interpreter hasn't seen this string yet), allocates
/// it via the GC and caches it.
///
/// # Safety
///
/// `vm_ptr` must be a valid pointer to a `Vm`. `gc_ptr` must be a valid pointer
/// to a `SemiSpace`. `prog_ptr` must point to a live `BytecodeProgram`.
pub extern "C" fn rune_jit_string_helper(
    vm_ptr: *mut u8,
    gc_ptr: *mut u8,
    prog_ptr: *const u8,
    string_idx: usize,
) -> u64 {
    let vm = unsafe { &mut *(vm_ptr as *mut Vm) };
    let gc = unsafe { &mut *(gc_ptr as *mut SemiSpace) };
    let cache_key = prog_ptr as usize;
    let handles = vm.string_cache.entry(cache_key).or_insert_with(|| {
        Vec::new()
    });
    if string_idx >= handles.len() {
        handles.resize(string_idx + 1, Value::undefined());
    }
    let val = &mut handles[string_idx];
    if val.is_undefined() {
        let prog = unsafe { &*(prog_ptr as *const rune_bytecode::opcode::BytecodeProgram) };
        let s = prog.string_pool.get(string_idx).map(|s| s.as_str()).unwrap_or("");
        let ptr = rune_core::string::HeapString::allocate(gc, s);
        *val = Value::from_heap_ptr(ptr as *mut u8);
    }
    val.raw()
}

/// JIT callout for LoadGlobal, StoreGlobal, IncGlobal, DecGlobal.
///
/// # Safety
///
/// `vm_ptr` must be a valid Vm pointer. `gc_ptr` must be a valid SemiSpace.
/// `prog_ptr` must point to a live BytecodeProgram.
pub extern "C" fn rune_jit_global_helper(
    vm_ptr: *mut u8,
    gc_ptr: *mut u8,
    prog_ptr: *const u8,
    op: u64,
    name_idx: u64,
    value_raw: u64,
) -> u64 {
    let vm = unsafe { &mut *(vm_ptr as *mut Vm) };
    let gc = unsafe { &mut *(gc_ptr as *mut SemiSpace) };
    let prog = unsafe { &*(prog_ptr as *const rune_bytecode::opcode::BytecodeProgram) };
    let name = prog.string_pool.get(name_idx as usize).map(|s| s.as_str()).unwrap_or("");

    match op {
        0 => {
            // LoadGlobal
            let val = vm.globals.get(name).copied()
                .or_else(|| vm.builtin_wrappers.get(name).copied())
                .or_else(|| vm.get_builtin(name))
                .unwrap_or(Value::undefined());
            val.raw()
        }
        1 => {
            // StoreGlobal
            let val = Value::from_raw(value_raw);
            vm.globals.insert(name.to_string(), val);
            val.raw()
        }
        2 | 3 => {
            // IncGlobal (2) or DecGlobal (3)
            let old_val = vm.globals.get(name).copied()
                .or_else(|| vm.builtin_wrappers.get(name).copied())
                .or_else(|| vm.get_builtin(name))
                .unwrap_or(Value::undefined());
            let is_prefix = value_raw != 0;
            let n = if op == 2 {
                to_number(old_val) + 1.0
            } else {
                to_number(old_val) - 1.0
            };
            let new_val = number_result(gc, n);
            vm.globals.insert(name.to_string(), new_val);
            let result = if is_prefix { new_val } else { old_val };
            result.raw()
        }
        _ => Value::undefined().raw(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_bytecode::opcode::{BytecodeProgram, Instruction};

    fn run(prog: &BytecodeProgram) -> Result<Value, Value> {
        let mut gc = SemiSpace::new();
        let mut vm = Vm::new();
        vm.execute(&mut gc, prog)
    }

    fn run_ok(prog: &BytecodeProgram) -> Value {
        run(prog).unwrap()
    }

    macro_rules! prog {
        ($($op:expr),* $(,)?) => {
            BytecodeProgram::new(
                vec![$(Instruction::new($op, vec![])),*],
                vec![],
                vec![],
            )
        };
    }

    #[test]
    fn test_load_smi() {
        let p = BytecodeProgram::new(
            vec![Instruction::new(Opcode::LoadSmi, vec![42])],
            vec![],
            vec![],
        );
        let v = run_ok(&p);
        assert_eq!(v.as_smi(), Some(42));
    }

    #[test]
    fn test_load_undefined() {
        let p = prog![Opcode::LoadUndefined];
        assert!(run_ok(&p).is_undefined());
    }

    #[test]
    fn test_load_null() {
        let p = prog![Opcode::LoadNull];
        assert!(run_ok(&p).is_null());
    }

    #[test]
    fn test_load_boolean_true() {
        let p = BytecodeProgram::new(
            vec![Instruction::new(Opcode::LoadBoolean, vec![1])],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_load_boolean_false() {
        let p = BytecodeProgram::new(
            vec![Instruction::new(Opcode::LoadBoolean, vec![0])],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(false));
    }

    #[test]
    fn test_add_smi() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![10]),
                Instruction::new(Opcode::LoadSmi, vec![20]),
                Instruction::new(Opcode::Add, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(30));
    }

    #[test]
    fn test_sub() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![20]),
                Instruction::new(Opcode::LoadSmi, vec![5]),
                Instruction::new(Opcode::Sub, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(15));
    }

    #[test]
    fn test_mul() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![6]),
                Instruction::new(Opcode::LoadSmi, vec![7]),
                Instruction::new(Opcode::Mul, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(42));
    }

    #[test]
    fn test_div() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![10]),
                Instruction::new(Opcode::LoadSmi, vec![3]),
                Instruction::new(Opcode::Div, vec![]),
            ],
            vec![],
            vec![],
        );
        let v = run_ok(&p);
        assert!(v.is_float64(), "10/3 should be a float");
        assert!((v.as_float64().unwrap() - 3.3333333333333335).abs() < 1e-10);
    }

    #[test]
    fn test_mod() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![10]),
                Instruction::new(Opcode::LoadSmi, vec![3]),
                Instruction::new(Opcode::Mod, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_neg() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::Neg, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(-42));
    }

    #[test]
    fn test_not() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0]),
                Instruction::new(Opcode::Not, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_bitnot() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::BitNot, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(!42));
    }

    #[test]
    fn test_void() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![99]),
                Instruction::new(Opcode::Void, vec![]),
            ],
            vec![],
            vec![],
        );
        assert!(run_ok(&p).is_undefined());
    }

    #[test]
    fn test_jump() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::Jump, vec![2]),    // skip to instr 2
                Instruction::new(Opcode::LoadSmi, vec![0]), // skipped
                Instruction::new(Opcode::LoadSmi, vec![1]), // target
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_jump_if_false_taken() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![0]), // false
                Instruction::new(Opcode::JumpIfFalse, vec![3]),
                Instruction::new(Opcode::LoadSmi, vec![0]), // skipped
                Instruction::new(Opcode::LoadSmi, vec![1]), // target
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_jump_if_true_taken() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![1]), // true
                Instruction::new(Opcode::JumpIfTrue, vec![3]),
                Instruction::new(Opcode::LoadSmi, vec![0]), // skipped
                Instruction::new(Opcode::LoadSmi, vec![1]), // target
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_dup_pop() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::Dup, vec![]),
                Instruction::new(Opcode::Pop, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(42));
    }

    #[test]
    fn test_eq() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::Eq, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_neq() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::LoadSmi, vec![2]),
                Instruction::new(Opcode::Ne, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_lt() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::LoadSmi, vec![2]),
                Instruction::new(Opcode::Lt, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_bitwise() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0xFF]),
                Instruction::new(Opcode::LoadSmi, vec![0x0F]),
                Instruction::new(Opcode::BitAnd, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(0x0F));
    }

    #[test]
    fn test_shift() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![8]),
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::Shl, vec![]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(16));
    }

    #[test]
    fn test_logical_and_short_circuit() {
        // false && ... → false (short circuit, RHS not evaluated)
        // lhs, Dup, JumpIfFalse→end, Pop, rhs, end:
        // JumpIfFalse POPS and jumps if falsy; Dup preserves lhs copy for result.
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![0]),
                Instruction::new(Opcode::Dup, vec![]),
                Instruction::new(Opcode::JumpIfFalse, vec![5]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadBoolean, vec![1]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(false));
    }

    #[test]
    fn test_logical_or_short_circuit() {
        // true || ... → true (short circuit, RHS not evaluated)
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![1]),
                Instruction::new(Opcode::Dup, vec![]),
                Instruction::new(Opcode::JumpIfTrue, vec![5]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadBoolean, vec![0]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_logical_and_non_short_circuit() {
        // true && false → false (no short circuit, both evaluated)
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![1]),
                Instruction::new(Opcode::Dup, vec![]),
                Instruction::new(Opcode::JumpIfFalse, vec![5]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadBoolean, vec![0]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(false));
    }

    #[test]
    fn test_logical_or_non_short_circuit() {
        // false || true → true (no short circuit, both evaluated)
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![0]),
                Instruction::new(Opcode::Dup, vec![]),
                Instruction::new(Opcode::JumpIfTrue, vec![5]),
                Instruction::new(Opcode::Pop, vec![]),
                Instruction::new(Opcode::LoadBoolean, vec![1]),
            ],
            vec![],
            vec![],
        );
        assert_eq!(run_ok(&p).to_boolean(), Some(true));
    }

    #[test]
    fn test_typeof_smi() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::TypeOf, vec![]),
            ],
            vec![],
            vec![],
        );
        let v = run_ok(&p);
        assert!(v.is_heap_object(), "typeof smi should return heap string");
    }

    #[test]
    fn test_throw_returns_error() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![99]),
                Instruction::new(Opcode::Throw, vec![]),
            ],
            vec![],
            vec![],
        );
        let result = run(&p);
        assert!(result.is_err(), "throw should return Err");
        assert_eq!(result.unwrap_err().as_smi(), Some(99));
    }
}
