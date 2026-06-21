use rune_bytecode::opcode::{BytecodeProgram, Instruction, Opcode};
use rune_core::float::HeapFloat64;
use rune_core::function::Func;
use rune_core::gc::{GcHeader, SemiSpace, TAG_FUNC, TAG_STRING, TAG_OBJECT, TAG_FLOAT64};
use rune_core::object::JSObject;
use rune_core::shape::{PropertyKey, Shape, PROTOTYPE_KEY};
use rune_core::string::HeapString;
use rune_core::value::Value;
use crate::builtins::{Builtin, BuiltinFn};
use crate::generator::Generator;
use crate::ic::{IcEntry, IcStats, InlineCache};
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
        }
    }

    /// Build pre-wired constructor objects (Object, etc.) in the GC heap.
    /// Must be called after all builtins are registered.
    pub fn init_builtin_wrappers(&mut self, gc: &mut SemiSpace) {
        // Find the Object_create builtin index
        let create_id = self.builtins.iter().position(|b| b.name == "Object_create");
        if let Some(create_idx) = create_id {
            let create_handle = Value::smi(-(create_idx as i32) - 1);
            // Build Object wrapper: an object with {create: <builtin handle>}
            let key = rune_core::shape::PropertyKey::from_string("create");
            let shape = rune_core::shape::Shape::intern(vec![(key, 0usize)]);
            let obj_ptr = JSObject::allocate(gc, shape, &[create_handle]);
            let obj_val = Value::from_heap_ptr(obj_ptr as *mut u8);
            self.builtin_wrappers.insert("Object".to_string(), obj_val);
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
                        } else {
                            Value::smi(v.wrapping_neg())
                        }
                    } else if let Some(v) = a.as_float64() {
                        let ptr = HeapFloat64::allocate(gc, -v);
                        Value::from_float64_ptr(ptr as *mut u8)
                    } else if a.is_undefined() || a.is_null() {
                        let ptr = HeapFloat64::allocate(gc, f64::NAN);
                        Value::from_float64_ptr(ptr as *mut u8)
                    } else {
                        self.push(a);
                        self.frames[fi].pc = pc + 1;
                        continue;
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
                Opcode::LogicalAnd => {
                    let a = self.pop();
                    if !a.to_bool() {
                        self.push(a);
                    } else {
                        let b = self.pop();
                        self.push(b);
                    }
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LogicalOr => {
                    let a = self.pop();
                    if a.to_bool() {
                        self.push(a);
                    } else {
                        let b = self.pop();
                        self.push(b);
                    }
                    self.frames[fi].pc = pc + 1;
                }

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
                        _ => compare_strings_lt(a, b).map(|v| Value::smi(if v { 1 } else { 0 })).unwrap_or(Value::undefined()),
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::Gt => {
                    let b = self.pop();
                    let a = self.pop();
                    let result = match (a.as_smi(), b.as_smi()) {
                        (Some(av), Some(bv)) => Value::smi(if av > bv { 1 } else { 0 }),
                        _ => compare_strings_lt(b, a).map(|v| Value::smi(if v { 1 } else { 0 })).unwrap_or(Value::undefined()),
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
                            let gt = compare_strings_lt(b, a).unwrap_or(true);
                            Value::smi(if !gt { 1 } else { 0 })
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
                            let lt = compare_strings_lt(a, b).unwrap_or(true);
                            Value::smi(if !lt { 1 } else { 0 })
                        }
                    };
                    self.push(result);
                    self.frames[fi].pc = pc + 1;
                }

                // ---- Objects ----
                Opcode::NewObject => {
                    let count = instr.operands[0] as usize;
                    let mut values: Vec<Value> = (0..count).map(|_| self.pop()).collect();
                    values.reverse();
                    let mut entries: Vec<(PropertyKey, usize)> = Vec::with_capacity(count);
                    for i in 0..count {
                        let key_idx = instr.operands[1 + i] as usize;
                        let key_str = self.frames[fi].prog_str(key_idx).unwrap_or_default();
                        entries.push((PropertyKey::from_string(&key_str), i));
                    }
                    let shape = Shape::intern(entries);
                    let obj = JSObject::allocate(gc, shape, &values);
                    self.push(Value::from_heap_ptr(obj as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::NewArray => {
                    let elem_count = instr.operands[0] as usize;
                    let mut elems: Vec<Value> = (0..elem_count).map(|_| self.pop()).collect();
                    elems.reverse();
                    let shape = Shape::empty();
                    let obj = JSObject::allocate(gc, shape, &elems);
                    self.push(Value::from_heap_ptr(obj as *mut u8));
                    self.frames[fi].pc = pc + 1;
                }
                Opcode::LoadProperty => {
                    let raw_key = self.pop();
                    let obj = self.pop();
                    let result = if obj.is_heap_object() {
                        // IC fast path: check inline cache before full walk
                        if instr.ic_index >= 0 {
                            let ic_idx = instr.ic_index as usize;
                            self.ic_stats.lookups += 1;
                            if ic_idx < self.ics.len() {
                                if let Some(ptr) = obj.heap_ptr() {
                                    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                                    if tag == TAG_OBJECT {
                                        let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                                        if let Some(entry) = self.ics[ic_idx].entries.get(&shape.id) {
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
                                    }
                                }
                            }
                            self.ic_stats.misses += 1;
                            // Full lookup with IC population
                            load_property_recursive_ic(gc, &mut self.ics, &instr, obj, raw_key)
                        } else {
                            // No IC attached — fall back to full lookup
                            load_property_recursive(obj, raw_key)
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
                                    // Property not found — add it with shape transition
                                    unsafe { JSObject::add_property(ptr as *mut JSObject, key, value) };
                                }
                            }
                        }
                    }
                    self.push(value);
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
                    // If constructor is a builtin, call it with the new object
                    if let Some(smi_val) = constructor.as_smi() {
                        if smi_val < 0 {
                            let id = ((-smi_val) as usize) - 1;
                            if id < self.builtins.len() {
                                let result = (self.builtins[id].func)(gc, &args, &*self);
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

                    // Builtin dispatch: negative Smi handles
                    if let Some(smi_val) = callee.as_smi() {
                        if smi_val < 0 {
                            let id = ((-smi_val) as usize) - 1;
                            if id < self.builtins.len() {
                                let result = (self.builtins[id].func)(gc, &args, &*self);
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
                                let mut locals: Vec<Value> = if func_prog.named_function { vec![callee] } else { vec![] };
                                locals.extend(args);
                                self.frames.push(Frame {
                                    locals,
                                    pc: 0,
                                    stack_base: self.stack.len(),
                                    prog: func_prog as *const BytecodeProgram,
                                    generator_id: None,
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
                    self.last_locals = self.frames[popped_frame].locals.clone();
                    self.frames.pop();
                    self.try_stack.retain(|tf| tf.frame_depth != popped_frame);
                    if self.frames.is_empty() {
                        self.stack.clear();
                        return Exit::Return(result);
                    }
                    let new_fi = self.frames.len() - 1;
                    self.stack.truncate(callee_base);
                    self.push(result);
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
}

impl Frame {
    fn prog_str(&self, idx: usize) -> Option<String> {
        let prog = unsafe { &*self.prog };
        prog.string_pool.get(idx).cloned()
    }
}

/// Convert a Value to its string representation for concatenation.
fn values_strictly_equal(a: Value, b: Value) -> bool {
    if a == b {
        return true;
    }
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
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![0]),
                Instruction::new(Opcode::LogicalAnd, vec![]),
            ],
            vec![], vec![],
        );
        assert_eq!(run_ok(&p).as_smi(), Some(0));
    }

    #[test]
    fn test_logical_or_short_circuit() {
        let p = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadBoolean, vec![1]),
                Instruction::new(Opcode::LogicalOr, vec![]),
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
    // Populate IC if applicable
    if instr.ic_index >= 0 {
        if result.is_heap_object() {
            if let Some(ptr) = obj.heap_ptr() {
                let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                if tag == TAG_OBJECT {
                    if let Some(key) = value_to_prop_key(raw_key) {
                        let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                        let ic_idx = instr.ic_index as usize;
                        while ics.len() <= ic_idx {
                            ics.push(InlineCache::new());
                        }
                        if let Some(offset) = shape.lookup(&key) {
                            // Own property
                            ics[ic_idx].entries.insert(shape.id, IcEntry {
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
                                    ics[ic_idx].entries.insert(shape.id, IcEntry {
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

/// Check if a Value is a GC-allocated string.
fn value_is_string(v: Value) -> bool {
    if let Some(ptr) = v.heap_ptr() {
        unsafe { (*(ptr as *const GcHeader)).tag() == TAG_STRING }
    } else {
        false
    }
}
