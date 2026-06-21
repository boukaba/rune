use rune_bytecode::opcode::{BytecodeProgram, Instruction, Opcode};
use rune_core::float::HeapFloat64;
use rune_core::function::Func;
use rune_core::gc::{GcHeader, SemiSpace, TAG_FUNC, TAG_STRING, TAG_OBJECT, TAG_ARRAY, TAG_FLOAT64};
use rune_core::object::JSObject;
use rune_core::array::RuneArray;
use rune_core::shape::{PropertyKey, Shape, PROTOTYPE_KEY, DENSE_ARRAY_SHAPE};
use rune_core::string::HeapString;
use rune_core::value::Value;
use crate::builtins::{Builtin, BuiltinFn};
use crate::generator::Generator;
use crate::ic::{IcEntry, IcStats, InlineCache};
#[cfg(all(feature = "jit", target_arch = "x86_64"))]
use rune_jit_baseline::{CodeGen, JitEntryFn};
use std::cell::UnsafeCell;
use std::collections::HashMap;

/// Callback for the `eval` builtin: parses and executes JS source, returns result.
pub type EvalFn = Box<dyn FnMut(&mut SemiSpace, &str) -> Result<Value, String>>;



struct Frame {
    locals: Vec<Value>,
    pc: usize,
    stack_base: usize,
    prog: *const BytecodeProgram,
    generator_id: Option<usize>,
    this: Value,
    is_constructor_call: bool,
    constructed_object: Value,
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

/// Stack-based bytecode interpreter with call frame support.
pub struct Vm {
    pub stack: Vec<Value>,
    frames: Vec<Frame>,
    try_stack: Vec<TryFrame>,
    pub generators: Vec<Generator>,
    pub builtins: Vec<Builtin>,
    pub globals: HashMap<String, Value>,
    /// Shape-Indexed Dispatch Tables for property access caching.
    pub ics: Vec<InlineCache>,
    /// Aggregate IC statistics.
    pub ic_stats: IcStats,
    /// Pre-built constructor objects (like `Object`) that expose methods via property access.
    builtin_wrappers: HashMap<String, Value>,
    last_locals: Vec<Value>,
    pub eval_fn: UnsafeCell<Option<EvalFn>>,
    /// Reference to Array.prototype for setting on newly created arrays.
    pub array_prototype: Value,
    /// Reference to String.prototype for string property access.
    pub string_prototype: Value,
    /// Reference to Object.prototype for setting on newly created objects.
    pub object_prototype: Value,
}

impl Vm {
    pub fn new() -> Self {
        Vm {
            stack: Vec::new(),
            frames: Vec::new(),
            try_stack: Vec::new(),
            generators: Vec::new(),
            builtins: Vec::new(),
            globals: HashMap::new(),
            ics: Vec::new(),
            ic_stats: IcStats::default(),
            builtin_wrappers: HashMap::new(),
            last_locals: Vec::new(),
            eval_fn: UnsafeCell::new(None),
            array_prototype: Value::undefined(),
            string_prototype: Value::undefined(),
            object_prototype: Value::undefined(),
        }
    }

    /// Build pre-wired constructor objects (Object, etc.) in the GC heap.
    /// Must be called after all builtins are registered.
    pub fn init_builtin_wrappers(&mut self, gc: &mut SemiSpace) {
        fn find_handle(builtins: &[Builtin], name: &str) -> Option<Value> {
            builtins.iter().position(|b| b.name == name)
                .map(|id| Value::smi(-(id as i32) - 1))
        }
        fn make_object(gc: &mut SemiSpace, pairs: &[(&str, Value)]) -> Value {
            let keys: Vec<(PropertyKey, usize)> = pairs.iter().enumerate()
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
            self.builtin_wrappers.insert("Array.prototype".to_string(), arr_proto);
            self.array_prototype = arr_proto;
        }

        // String.prototype with charAt/slice methods
        let char_at_handle = find_handle(&self.builtins, "String_prototype_charAt");
        let slice_handle = find_handle(&self.builtins, "String_prototype_slice");
        if let (Some(char_at), Some(slice)) = (char_at_handle, slice_handle) {
            let str_proto = make_object(gc, &[("charAt", char_at), ("slice", slice)]);
            self.builtin_wrappers.insert("String.prototype".to_string(), str_proto);
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
        ].iter().filter_map(|(name, val)| {
            val.map(|v| (*name, v))
        }).collect();
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
        self.globals.insert("undefined".to_string(), Value::undefined());
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
    fn all_smi(values: &[Value]) -> bool {
        values.iter().all(|v| v.is_smi())
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
        }
        for val in self.globals.values() {
            gc.push_root(val as *const Value as *mut u64);
        }
    }

    /// Execute a bytecode program and return its result.
    pub fn execute(&mut self, gc: &mut SemiSpace, program: &BytecodeProgram) -> Result<Value, Value> {
        self.frames.clear();
        self.stack.clear();
        self.try_stack.clear();

        // Initialize top-level locals from persisted globals
        let locals: Vec<Value> = program.local_names.iter()
            .map(|name| self.globals.get(name).copied().unwrap_or(Value::undefined()))
            .collect();

        self.frames.push(Frame {
            locals,
            pc: 0,
            stack_base: 0,
            prog: program as *const BytecodeProgram,
            generator_id: None,
            this: Value::undefined(),
            is_constructor_call: false,
            constructed_object: Value::undefined(),
        });

        self.register_roots(gc);

        let result = match self.run_loop(gc) {
            Exit::Return(v) => Ok(v),
            Exit::Yield(_) => Ok(Value::undefined()),
            Exit::Throw(v) => Err(v),
        };

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
    pub fn resume_generator(&mut self, gc: &mut SemiSpace, gen_id: usize, arg: Value) -> Result<Value, Value> {
        if self.generators[gen_id].done {
            return Ok(Value::undefined());
        }
        self.try_stack.clear();

        let (locals, pc, prog, started) = {
            let g = &self.generators[gen_id];
            (g.locals.clone(), g.pc, g.prog, g.started)
        };

        self.frames.push(Frame {
            locals,
            pc,
            stack_base: self.stack.len(),
            prog,
            generator_id: Some(gen_id),
            this: Value::undefined(),
            is_constructor_call: false,
            constructed_object: Value::undefined(),
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
                    self.push(if val { Value::smi(1) } else { Value::smi(0) });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadString => {
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadStringConst => {
                    let idx = instr.operands[0] as usize;
                    let s = prog.string_pool.get(idx).map(|s| s.as_str()).unwrap_or("");
                    let ptr = HeapString::allocate(gc, s);
                    self.push(Value::from_heap_ptr(ptr as *mut u8));
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
                    self.pop();
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Dup => {
                    let val = self.peek();
                    self.push(val);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Unary ----
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
                    self.push(if a.to_bool() { Value::smi(0) } else { Value::smi(1) });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitNot => {
                    let a = self.pop();
                    let result = if let Some(v) = a.as_smi() {
                        Value::smi(!v)
                    } else {
                        Value::undefined()
                    };
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
                        if bv == 0 { number_result(gc, f64::NAN) } else { Value::smi(av % bv) }
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
                        if bv < 0 { number_result(gc, (av as f64).powf(bv as f64)) } else { Value::smi(av.wrapping_pow(bv as u32)) }
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
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        Value::smi(av.wrapping_shl(bv as u32))
                    } else { Value::undefined() };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Shr => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        Value::smi(av.wrapping_shr(bv as u32))
                    } else { Value::undefined() };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::ShrU => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        let shifted = (av as u32).wrapping_shr(bv as u32);
                        Value::smi(shifted as i32)
                    } else { Value::undefined() };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitOr => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        Value::smi(av | bv)
                    } else { Value::undefined() };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitXor => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        Value::smi(av ^ bv)
                    } else { Value::undefined() };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::BitAnd => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
                        Value::smi(av & bv)
                    } else { Value::undefined() };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Logical ----
                // ---- Comparisons ----
                Opcode::Eq | Opcode::StrictEq => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(if values_strictly_equal(a, b) { Value::smi(1) } else { Value::smi(0) });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Ne | Opcode::StrictNe => {
                    let b = self.pop();
                    let a = self.pop();
                    self.push(if !values_strictly_equal(a, b) { Value::smi(1) } else { Value::smi(0) });
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Lt => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = match (a.as_smi(), b.as_smi()) {
                        (Some(av), Some(bv)) => Value::smi(if av < bv { 1 } else { 0 }),
                        _ => {
                            if let Some(v) = compare_strings_lt(a, b) {
                                Value::smi(if v { 1 } else { 0 })
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::undefined()
                                } else {
                                    Value::smi(if av < bv { 1 } else { 0 })
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
                        (Some(av), Some(bv)) => Value::smi(if av > bv { 1 } else { 0 }),
                        _ => {
                            if let Some(v) = compare_strings_lt(b, a) {
                                Value::smi(if v { 1 } else { 0 })
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::undefined()
                                } else {
                                    Value::smi(if av > bv { 1 } else { 0 })
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
                        (Some(av), Some(bv)) => Value::smi(if av <= bv { 1 } else { 0 }),
                        _ => {
                            if let Some(v) = compare_strings_lt(a, b) {
                                Value::smi(if v { 1 } else { 0 })
                            } else if let Some(v) = compare_strings_lt(b, a) {
                                // Both are strings: if b < a then a <= b is false, else equal → true
                                Value::smi(if v { 0 } else { 1 })
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::smi(0)
                                } else {
                                    Value::smi(if av <= bv { 1 } else { 0 })
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
                        (Some(av), Some(bv)) => Value::smi(if av >= bv { 1 } else { 0 }),
                        _ => {
                            if let Some(v) = compare_strings_lt(b, a) {
                                Value::smi(if v { 1 } else { 0 })
                            } else if let Some(v) = compare_strings_lt(a, b) {
                                Value::smi(if v { 0 } else { 1 })
                            } else {
                                let av = to_number(a);
                                let bv = to_number(b);
                                if av.is_nan() || bv.is_nan() {
                                    Value::smi(0)
                                } else {
                                    Value::smi(if av >= bv { 1 } else { 0 })
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
                    self.push(if found { Value::smi(1) } else { Value::smi(0) });
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
                    if self.object_prototype.is_heap_object() {
                        if let Some(proto_ptr) = self.object_prototype.heap_ptr() {
                            unsafe { JSObject::set_prototype(obj, proto_ptr); }
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
                        if self.array_prototype.is_heap_object() {
                            if let Some(proto) = self.array_prototype.heap_ptr() {
                                *proto_ptr = proto;
                            }
                        }
                    }
                    self.push(Value::from_heap_ptr(arr as *mut u8));
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
                                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) } as usize;
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
                                let s = unsafe { HeapString::to_string(obj.heap_ptr().unwrap() as *mut HeapString) };
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
                                    let key_str = unsafe { HeapString::to_string(ptr as *mut HeapString) };
                                    if key_str == "length" {
                                        // String length
                                        let s = unsafe { HeapString::to_string(obj.heap_ptr().unwrap() as *mut HeapString) };
                                        let len = s.encode_utf16().count();
                                        Value::smi(len as i32)
                                    } else if self.string_prototype.is_heap_object() {
                                        // Look up from String.prototype
                                        if let Some(proto_ptr) = self.string_prototype.heap_ptr() {
                                            let proto_key = PropertyKey::from_string(&key_str);
                                            let shape = unsafe { JSObject::shape_ptr(proto_ptr as *mut JSObject) };
                                            if let Some(slot) = shape.lookup(&proto_key) {
                                                unsafe { JSObject::get_slot(proto_ptr as *mut JSObject, slot) }
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
                                if ic_idx < self.ics.len() {
                                    if let Some(ptr) = obj.heap_ptr() {
                                        if tag == TAG_OBJECT {
                                            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                            let ck = ic_cache_key(shape.id, raw_key);
                                            if let Some(entry) = self.ics[ic_idx].entries.get(&ck) {
                                                self.ic_stats.hits += 1;
                                                let val = if entry.is_own {
                                                    unsafe { JSObject::get_slot(ptr as *mut JSObject, entry.offset) }
                                                } else {
                                                    let mut p = ptr as *mut u8;
                                                    for _ in 0..entry.proto_depth {
                                                        let next = unsafe { JSObject::prototype(p as *mut JSObject) };
                                                        if next.is_null() { break; }
                                                        p = next;
                                                    }
                                                    unsafe { JSObject::get_slot(p as *mut JSObject, entry.offset) }
                                                };
                                                self.push(val);
                                                self.frames[fi].pc = pc + 1;
                                                continue;
                                            }
                                        } else if tag == TAG_ARRAY {
                                            // Array IC hit: offset is element index
                                            let ck = ic_cache_key((*DENSE_ARRAY_SHAPE).id, raw_key);
                                            if let Some(entry) = self.ics[ic_idx].entries.get(&ck) {
                                                self.ic_stats.hits += 1;
                                                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
                                                let val = if entry.is_own {
                                                    if entry.offset < len as usize {
                                                        unsafe { RuneArray::get_element(ptr as *mut RuneArray, entry.offset) }
                                                    } else {
                                                        Value::undefined()
                                                    }
                                                } else {
                                                    // Inherited from Array.prototype
                                                    let mut p = ptr as *mut u8;
                                                    for _ in 0..entry.proto_depth {
                                                        let next = unsafe { JSObject::prototype(p as *mut JSObject) };
                                                        if next.is_null() { break; }
                                                        p = next;
                                                    }
                                                    unsafe { JSObject::get_slot(p as *mut JSObject, entry.offset) }
                                                };
                                                self.push(val);
                                                self.frames[fi].pc = pc + 1;
                                                continue;
                                            }
                                        }
                                    }
                                }
                                self.ic_stats.misses += 1;
                                // Full lookup with IC population
                                let result = load_property_recursive_ic(gc, &mut self.ics, &instr, obj, raw_key);
                                result
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
                Opcode::StoreProperty => {
                    let value = self.pop();
                    let raw_key = self.pop();
                    let obj = self.pop();
                    if let Some(ptr) = obj.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_OBJECT {
                            if let Some(key) = value_to_prop_key(raw_key) {
                                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                if let Some(slot) = shape.lookup(&key) {
                                    unsafe { JSObject::set_slot(ptr as *mut JSObject, slot, value) };
                                } else {
                                    let key_name = value_to_debug_string(raw_key);
                                    unsafe { JSObject::add_property(ptr as *mut JSObject, key, key_name, value) };
                                }
                            }
                        } else if tag == TAG_ARRAY {
                            // Dense array: numeric key → element set
                            if let Some(index) = value_to_array_index(raw_key) {
                                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
                                if index < len as usize {
                                    unsafe { RuneArray::set_element(ptr as *mut RuneArray, index, value) };
                                }
                            }
                        } else if tag == TAG_FUNC {
                            // Function.prototype = value
                            if let Some(key) = value_to_prop_key(raw_key) {
                                if key == *PROTOTYPE_KEY {
                                    if let Some(val_ptr) = value.heap_ptr() {
                                        unsafe { Func::set_prototype(ptr as *mut Func, val_ptr); }
                                    }
                                }
                            }
                        }
                    }
                    self.push(value);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::DeleteProperty => {
                    let raw_key = self.pop();
                    let obj = self.pop();
                    let result = if let Some(ptr) = obj.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_OBJECT {
                            if let Some(key) = value_to_prop_key(raw_key) {
                                unsafe { JSObject::remove_property(ptr as *mut JSObject, &key) };
                            }
                        }
                        Value::smi(1)
                    } else {
                        Value::smi(1)
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::DefineProperty => {
                    let _value = self.pop();
                    let _raw_key = self.pop();
                    let _obj = self.pop();
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadGlobal => {
                    let name_idx = instr.operands[0] as usize;
                    if let Some(name) = self.frames[fi].prog_str(name_idx) {
                        let val = self.globals.get(&name).copied()
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
                        let old_val = self.globals.get(&name).copied()
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
                        let old_val = self.globals.get(&name).copied()
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

                // ---- Unary ----
                Opcode::TypeOf => {
                    let val = self.pop();
                    let s = if val.is_undefined() { "undefined" }
                    else if val.is_null() { "object" }
                    else if val.is_smi() { "number" }
                    else {
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
                    self.frames[fi].pc = target;
                }
                Opcode::JumpIfTrue => {
                    let val = self.pop();
                    let target = instr.operands[0] as usize;
                    if val.to_bool() { self.frames[fi].pc = target } else { self.frames[fi].pc = pc + 1 }
                }
                Opcode::JumpIfFalse => {
                    let val = self.pop();
                    let target = instr.operands[0] as usize;
                    if !val.to_bool() { self.frames[fi].pc = target } else { self.frames[fi].pc = pc + 1 }
                }
                Opcode::Throw => {
                    let val = self.pop();
                    // Find in-frame handler
                    let handler_idx = self.try_stack.iter().rposition(|tf| tf.frame_depth == self.frames.len());
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
                    let caller_idx = self.try_stack.iter().rposition(|tf| tf.frame_depth == self.frames.len());
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
                Opcode::MakeFunction => {
                    let func_idx = instr.operands[0] as u64;
                    let prog_ptr = prog as *const BytecodeProgram as *const u8;
                    let ptr = Func::allocate(gc, func_idx, prog_ptr);
                    // Create default `.prototype` object (§11.2.2)
                    let default_proto = JSObject::allocate(gc, Shape::empty(), &[]);
                    unsafe { Func::set_prototype(ptr, default_proto as *mut u8); }
                    self.push(Value::from_heap_ptr(ptr as *mut u8));
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
                    if let Some(smi_val) = constructor.as_smi() {
                        if smi_val < 0 {
                            let id = ((-smi_val) as usize) - 1;
                            if id < self.builtins.len() {
                                let result = (self.builtins[id].func)(gc, obj_val, &args, &mut *self);
                                if result.is_heap_object() {
                                    self.push(result);
                                } else {
                                    self.push(obj_val);
                                }
                                self.frames[fi].pc = pc + 1;
                                continue;
                            }
                        }
                    }
                    // Set prototype from constructor.prototype
                    // §11.2.2 [[Construct]]: new object's [[Prototype]] = constructor.prototype
                    // Use interned PROTOTYPE_KEY to avoid HeapString allocation.
                    if constructor.is_heap_object() {
                        if let Some(ptr) = constructor.heap_ptr() {
                            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                            if tag == TAG_OBJECT {
                                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                if let Some(slot) = shape.lookup(&*PROTOTYPE_KEY) {
                                    let proto_val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                                    if proto_val.is_heap_object() {
                                        if let Some(proto_ptr) = proto_val.heap_ptr() {
                                            unsafe {
                                                JSObject::set_prototype(obj, proto_ptr);
                                            }
                                        }
                                    }
                                }
                            } else if tag == TAG_FUNC {
                                // User-defined function: read prototype from Func struct
                                let proto_ptr = unsafe { Func::prototype(ptr as *mut Func) };
                                if !proto_ptr.is_null() {
                                    unsafe { JSObject::set_prototype(obj, proto_ptr); }
                                }
                            }
                        }
                    }
                    // If constructor is a user-defined function, call its body with this = new object
                    if let Some(ptr) = constructor.heap_ptr() {
                        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                        if tag == TAG_FUNC {
                            let func_idx = unsafe { Func::func_index(ptr as *mut Func) } as usize;
                            let creator_prog = unsafe { &*(Func::prog_ptr(ptr as *mut Func) as *const BytecodeProgram) };
                            if func_idx < creator_prog.functions.len() {
                                let func_prog = &creator_prog.functions[func_idx];
                                let mut locals: Vec<Value> = if func_prog.named_function { vec![constructor] } else { vec![] };
                                locals.extend(args);
                                self.frames.push(Frame {
                                    locals,
                                    pc: 0,
                                    stack_base: self.stack.len(),
                                    prog: func_prog as *const BytecodeProgram,
                                    generator_id: None,
                                    this: obj_val,
                                    is_constructor_call: true,
                                    constructed_object: obj_val,
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
                            let creator_prog = unsafe { &*(Func::prog_ptr(ptr as *mut Func) as *const BytecodeProgram) };
                            if func_idx < creator_prog.functions.len() {
                                let func_prog = &creator_prog.functions[func_idx];
                                if func_prog.is_generator {
                                    let g = Generator::new(args, func_prog as *const BytecodeProgram);
                                    let gen_id = self.generators.len();
                                    self.generators.push(g);
                                    self.push(Value::smi(gen_id as i32));
                                    self.frames[fi].pc = pc + 1;
                                    continue;
                                }
                                // --- JIT tier-up (if enabled on x86-64) ---
                                #[cfg(all(feature = "jit", target_arch = "x86_64"))]
                                {
                                    unsafe { Func::increment_call_count(ptr as *mut Func) };
                                    let count = unsafe { Func::call_count(ptr as *mut Func) };
                                    const JIT_THRESHOLD: u32 = 50;

                                    if unsafe { Func::jit_entry(ptr as *mut Func) }.is_null() {
                                        if count == JIT_THRESHOLD && rune_jit_baseline::is_jit_compatible(func_prog) {
                                            let codegen = CodeGen::new(func_prog.instructions.len());
                                            let mem = codegen.compile(func_prog);
                                            mem.make_executable();
                                            unsafe { Func::set_jit_entry(ptr as *mut Func, mem.code_ptr()) };
                                            std::mem::forget(mem);
                                        }
                                    }

                                    let jit_entry = unsafe { Func::jit_entry(ptr as *mut Func) };
                                    if !jit_entry.is_null() {
                                        let mut jit_locals: Vec<Value> = if func_prog.named_function { vec![callee] } else { vec![] };
                                        jit_locals.extend(args);
                                        let local_count = func_prog.local_names.len();
                                        while jit_locals.len() < local_count {
                                            jit_locals.push(Value::undefined());
                                        }
                                        // Safety check: only call JIT if all inputs are Smi
                                        if Self::all_smi(&jit_locals) {
                                            let func: JitEntryFn = unsafe { std::mem::transmute(jit_entry) };
                                            let vm_ptr = self as *mut Vm as *mut u8;
                                            let gc_ptr = gc as *mut SemiSpace as *mut u8;
                                            let result_raw = unsafe { func(vm_ptr, gc_ptr, jit_locals.as_mut_ptr() as *mut u64) };
                                            self.last_locals = jit_locals;
                                            self.push(Value::from_raw(result_raw));
                                            self.frames[fi].pc = pc + 1;
                                            continue;
                                        }
                                        // Non-Smi value present — fall through to interpreter
                                    }
                                }
                                // --- End JIT tier-up ---
                                let mut locals: Vec<Value> = if func_prog.named_function { vec![callee] } else { vec![] };
                                locals.extend(args);
                                self.frames.push(Frame {
                                    locals,
                                    pc: 0,
                                    stack_base: self.stack.len(),
                                    prog: func_prog as *const BytecodeProgram,
                                    generator_id: None,
                                    this,
                                    is_constructor_call: false,
                                    constructed_object: Value::undefined(),
                                });
                                continue;
                            }
                        }
                    }
                    self.push(Value::undefined());
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Return => {
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
        let saved_locals = self.frames.first().map(|f| f.locals.clone()).unwrap_or_default();
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
            if let Some(p) = v.heap_ptr() {
                if p == old_ptr {
                    *v = Value::from_heap_ptr(new_ptr);
                }
            }
        }
        for frame in &mut self.frames {
            for v in &mut frame.locals {
                if let Some(p) = v.heap_ptr() {
                    if p == old_ptr {
                        *v = Value::from_heap_ptr(new_ptr);
                    }
                }
            }
        }
        for v in self.globals.values_mut() {
            if let Some(p) = v.heap_ptr() {
                if p == old_ptr {
                    *v = Value::from_heap_ptr(new_ptr);
                }
            }
        }
    }
}

impl Frame {
    fn prog_str(&self, idx: usize) -> Option<String> {
        let prog = unsafe { &*self.prog };
        prog.string_pool.get(idx).cloned()
    }
}

/// Per §7.2.14 IsStrictlyEqual.
fn values_strictly_equal(a: Value, b: Value) -> bool {
    // Both are Number type (Smi or Float64)
    if a.is_smi() || b.is_smi() || a.is_float64() || b.is_float64() {
        let na = if a.is_smi() { a.as_smi().map(|s| s as f64) } else { a.as_float64() };
        let nb = if b.is_smi() { b.as_smi().map(|s| s as f64) } else { b.as_float64() };
        if let (Some(av), Some(bv)) = (na, nb) {
            if av.is_nan() || bv.is_nan() { return false; }
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
                if ca < cb { return Some(true); }
                if ca > cb { return Some(false); }
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
            vec![], vec![],
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
            vec![], vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_load_boolean_false() {
        let p = BytecodeProgram::new(
            vec![Instruction::new(Opcode::LoadBoolean, vec![0])],
            vec![], vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(0));
    }

    #[test]
    fn test_add_smi() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![10]),
                Instruction::new(Opcode::LoadSmi, vec![20]),
                Instruction::new(Opcode::Add, vec![]),
            ],
            vec![], vec![],
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
            vec![], vec![],
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
            vec![], vec![],
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
            vec![], vec![],
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
            vec![], vec![],
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
            vec![], vec![],
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
            vec![], vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_bitnot() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::BitNot, vec![]),
            ],
            vec![], vec![],
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
            vec![], vec![],
        );
        assert!(run_ok(&p).is_undefined());
    }

    #[test]
    fn test_jump() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::Jump, vec![2]),   // skip to instr 2
                Instruction::new(Opcode::LoadSmi, vec![0]), // skipped
                Instruction::new(Opcode::LoadSmi, vec![1]), // target
            ],
            vec![], vec![],
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
            vec![], vec![],
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
            vec![], vec![],
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
            vec![], vec![],
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
            vec![], vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_neq() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::LoadSmi, vec![2]),
                Instruction::new(Opcode::Ne, vec![]),
            ],
            vec![], vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_lt() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::LoadSmi, vec![2]),
                Instruction::new(Opcode::Lt, vec![]),
            ],
            vec![], vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_bitwise() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![0xFF]),
                Instruction::new(Opcode::LoadSmi, vec![0x0F]),
                Instruction::new(Opcode::BitAnd, vec![]),
            ],
            vec![], vec![],
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
            vec![], vec![],
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
            vec![], vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(0));
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
            vec![], vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
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
            vec![], vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(0));
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
            vec![], vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(1));
    }

    #[test]
    fn test_typeof_smi() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::TypeOf, vec![]),
            ],
            vec![], vec![],
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
            vec![], vec![],
        );
        let result = run(&p);
        assert!(result.is_err(), "throw should return Err");
        assert_eq!(result.unwrap_err().as_smi(), Some(99));
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
                if let Some(key) = value_to_prop_key(raw_key) {
                    if key == *PROTOTYPE_KEY {
                        let proto_ptr = unsafe { Func::prototype(ptr as *mut Func) };
                        if !proto_ptr.is_null() {
                            return Value::from_heap_ptr(proto_ptr);
                        }
                    }
                }
                return Value::undefined();
            }
        }
        return Value::undefined();
    }
}

/// Full property lookup that populates the inline cache on miss.
fn load_property_recursive_ic(
    _gc: &mut SemiSpace,
    ics: &mut Vec<InlineCache>,
    instr: &Instruction,
    obj: Value,
    raw_key: Value,
) -> Value {
    let result = load_property_recursive(obj, raw_key);
    // Populate IC for all result types — Smi, Float64, heap, undefined
    if instr.ic_index >= 0 {
        if let Some(ptr) = obj.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            let ic_idx = instr.ic_index as usize;
            while ics.len() <= ic_idx {
                ics.push(InlineCache::new());
            }
            if tag == TAG_OBJECT {
                if let Some(key) = value_to_prop_key(raw_key) {
                    let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                    let ck = ic_cache_key(shape.id, raw_key);
                    if let Some(offset) = shape.lookup(&key) {
                        // Own property
                        ics[ic_idx].entries.insert(ck, IcEntry {
                            offset,
                            is_own: true,
                            proto_depth: 0,
                        });
                    } else {
                        // Inherited — walk prototype chain to find offset and depth
                        let mut depth: u8 = 0;
                        let mut p = ptr as *mut u8;
                        loop {
                            let next = unsafe { JSObject::prototype(p as *mut JSObject) };
                            if next.is_null() { break; }
                            depth += 1;
                            if depth >= MAX_PROTOTYPE_DEPTH as u8 { break; }
                            let next_shape = unsafe { JSObject::shape_ptr(next as *mut JSObject) };
                            if let Some(offset) = next_shape.lookup(&key) {
                                ics[ic_idx].entries.insert(ck, IcEntry {
                                    offset,
                                    is_own: false,
                                    proto_depth: depth,
                                });
                                break;
                            }
                            p = next;
                        }
                    }
                }
            } else if tag == TAG_ARRAY {
                // Dense array IC: numeric keys cache element index directly
                if let Some(index) = value_to_array_index(raw_key) {
                    let ck = ic_cache_key((*DENSE_ARRAY_SHAPE).id, raw_key);
                    ics[ic_idx].entries.insert(ck, IcEntry {
                        offset: index,
                        is_own: true,
                        proto_depth: 0,
                    });
                } else if let Some(key) = value_to_prop_key(raw_key) {
                    // Non-numeric key — inherited from Array.prototype
                    let ck = ic_cache_key((*DENSE_ARRAY_SHAPE).id, raw_key);
                    let mut depth: u8 = 0;
                    let mut p = ptr as *mut u8;
                    loop {
                        let next = unsafe { JSObject::prototype(p as *mut JSObject) };
                        if next.is_null() { break; }
                        depth += 1;
                        if depth >= MAX_PROTOTYPE_DEPTH as u8 { break; }
                        let next_shape = unsafe { JSObject::shape_ptr(next as *mut JSObject) };
                        if let Some(offset) = next_shape.lookup(&key) {
                            ics[ic_idx].entries.insert(ck, IcEntry {
                                offset,
                                is_own: false,
                                proto_depth: depth,
                            });
                            break;
                        }
                        p = next;
                    }
                }
            }
        }
    }
    result
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
            if upper.starts_with("0X") {
                if let Ok(n) = u64::from_str_radix(&upper[2..], 16) {
                    return n as f64;
                }
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
                                let proto_shape = unsafe { JSObject::shape_ptr(proto_ptr as *mut JSObject) };
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
            has_property(unsafe {
                let proto = JSObject::prototype(ptr as *mut JSObject);
                if proto.is_null() { return false; }
                Value::from_heap_ptr(proto)
            }, raw_key)
        } else if tag == TAG_FUNC {
            if let Some(key) = value_to_prop_key(raw_key) {
                if key == *PROTOTYPE_KEY {
                    return true;
                }
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
