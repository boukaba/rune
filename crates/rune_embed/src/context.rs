use std::pin::Pin;
use rune_bytecode::opcode::BytecodeProgram;
use rune_core::gc::SemiSpace;
use rune_core::value::Value;
use rune_interpreter::vm::Vm;

/// Stable embedding API for Rune.
/// Each `Context` owns a GC heap and can evaluate source code.
pub struct Context {
    gc: SemiSpace,
    vm: Vm,
    programs: Vec<Pin<Box<BytecodeProgram>>>,
}

impl Context {
    pub fn new() -> Self {
        let mut ctx = Context {
            gc: SemiSpace::new(),
            vm: Vm::new(),
            programs: Vec::new(),
        };
        // Register default builtins
        for b in rune_interpreter::builtins::default_builtins() {
            ctx.vm.register_builtin(b.name, b.func);
        }
        ctx
    }

    /// Parse, compile, and execute JavaScript source code.
    /// Returns the top-of-stack Value after execution.
    pub fn eval(&mut self, source: &str) -> Result<Value, String> {
        // Parse
        let mut parser = rune_parser::Parser::new(source);
        let program = parser.parse();
        if !parser.errors.is_empty() {
            return Err(format!("Parse errors: {:?}", parser.errors));
        }

        // Emit bytecode
        let mut emitter = rune_parser::emitter::Emitter::new();
        emitter.emit_program(&program);
        let bytecode = emitter.into_bytecode();

        // Execute — keep bytecode alive for dangling prog_ptr refs from Func
        let pinned = Box::pin(bytecode);
        self.programs.push(pinned);
        let prog_ref: &BytecodeProgram = &*self.programs.last().unwrap().as_ref();

        self.vm.execute(&mut self.gc, prog_ref).map_err(|v| format!("Uncaught: {v:?}"))
    }

    /// Evaluate raw bytecode instructions and return the top-of-stack Value.
    pub fn eval_bytecode(&mut self, bytecode: &BytecodeProgram) -> Result<Value, Value> {
        self.vm.execute(&mut self.gc, bytecode)
    }

    /// Resume a suspended generator with an argument.
    /// `gen_id` is the Smi handle returned from the generator's constructor call.
    /// Returns the next yielded (or returned) value.
    pub fn resume(&mut self, gen_id: usize, arg: Value) -> Result<Value, Value> {
        self.vm.resume_generator(&mut self.gc, gen_id, arg)
    }

    /// Allocate a string in the GC heap.
    pub fn allocate_string(&mut self, s: &str) -> Value {
        let ptr = rune_core::string::HeapString::allocate(&mut self.gc, s);
        Value::from_heap_ptr(ptr as *mut u8)
    }

    /// Allocate an object with the given slot values.
    pub fn allocate_object(&mut self, slot_values: &[Value]) -> Value {
        let shape = rune_core::shape::Shape::empty();
        let ptr = rune_core::object::JSObject::allocate(&mut self.gc, shape, slot_values);
        Value::from_heap_ptr(ptr as *mut u8)
    }

    pub fn gc(&mut self) -> &mut SemiSpace {
        &mut self.gc
    }

    pub fn vm(&mut self) -> &mut Vm {
        &mut self.vm
    }
}

/// Opaque context handle for C FFI.
#[allow(dead_code)]
pub struct ContextHandle(*mut Context);
