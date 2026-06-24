use rune_bytecode::opcode::BytecodeProgram;
use rune_core::gc::SemiSpace;
use rune_core::value::Value;
use rune_interpreter::vm::Vm;
use std::pin::Pin;

/// Stable embedding API for Rune.
/// Each `Context` owns a GC heap and can evaluate source code.
pub struct Context {
    gc: SemiSpace,
    vm: Vm,
    programs: Vec<Pin<Box<BytecodeProgram>>>,
    /// Opaque keep-alive objects (e.g. mmap'd native code) that must outlive
    /// any references held by the VM.
    _keep_alive: Vec<Box<dyn std::any::Any>>,
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

impl Context {
    pub fn new() -> Self {
        Self::new_with_semispace(16 * 1024 * 1024) // 16 MiB default
    }

    /// Like `new()` but with a small semispace (1 MiB) for tests that
    /// run many contexts in parallel. Large live sets (>60K objects)
    /// may OOM with this — use the 16 MiB `new()` for those.
    pub fn new_small() -> Self {
        Self::new_with_semispace(1024 * 1024) // 1 MiB for parallel tests
    }

    fn new_with_semispace(size: usize) -> Self {
        let mut ctx = Context {
            gc: SemiSpace::with_size(size),
            vm: Vm::new(),
            programs: Vec::new(),
            _keep_alive: Vec::new(),
        };
        // Register default builtins
        for b in rune_interpreter::builtins::default_builtins() {
            ctx.vm.register_builtin(b.name, b.func);
        }
        // Build constructor wrappers (Object.create, etc.)
        ctx.vm.init_builtin_wrappers(&mut ctx.gc);
        ctx
    }

    /// Parse and emit JavaScript source code into a `BytecodeProgram`.
    pub fn compile(&self, source: &str) -> Result<BytecodeProgram, String> {
        // Parse
        let mut parser = rune_parser::Parser::new(source);
        let program = parser.parse();
        if !parser.errors.is_empty() {
            return Err(format!("Parse errors: {:?}", parser.errors));
        }

        // Emit bytecode
        let mut emitter = rune_parser::emitter::Emitter::new();
        emitter.emit_program(&program);
        Ok(emitter.into_bytecode())
    }

    /// Parse, compile, and execute JavaScript source code.
    /// Returns the top-of-stack Value after execution.
    pub fn eval(&mut self, source: &str) -> Result<Value, String> {
        let bytecode = self.compile(source)?;

        // Execute — keep bytecode alive for dangling prog_ptr refs from Func
        let pinned = Box::pin(bytecode);
        self.programs.push(pinned);
        let prog_ref: &BytecodeProgram = &self.programs.last().unwrap().as_ref();

        self.vm
            .execute(&mut self.gc, prog_ref)
            .map_err(|v| format!("Uncaught: {v:?}"))
    }

    /// Execute a bytecode program and keep it alive in this context.
    pub fn eval_bytecode_owned(&mut self, bytecode: BytecodeProgram) -> Result<Value, String> {
        let pinned = Box::pin(bytecode);
        self.programs.push(pinned);
        let prog_ref: &BytecodeProgram = &self.programs.last().unwrap().as_ref();
        self.vm
            .execute(&mut self.gc, prog_ref)
            .map_err(|v| format!("Uncaught: {v:?}"))
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

    /// Return a clone of the VM's inline-cache (SIDT) table.
    /// Useful for AFPC cache serialization after a warm-up run.
    pub fn ics(&self) -> Vec<rune_interpreter::ic::InlineCache> {
        self.vm.ics.clone()
    }

    /// Install a previously-captured inline-cache table into the VM.
    /// Called during AFPC cache load before executing cached bytecode.
    pub fn set_ics(&mut self, ics: Vec<rune_interpreter::ic::InlineCache>) {
        self.vm.ics = ics;
    }

    /// Install cached native code entry points into the VM.
    /// The `InstalledNativeCode` object must be kept alive; storing it in the
    /// context guarantees the executable mapping outlives any calls.
    pub fn install_native_code(&mut self, mut native: crate::afpc::InstalledNativeCode) {
        self.vm.cached_jit_entries = native.take_entries();
        // Keep the mapping alive by storing it alongside the pinned programs.
        // `InstalledNativeCode` is not Send/Sync, so we box it as an opaque object.
        let boxed: Box<dyn std::any::Any> = Box::new(native);
        self._keep_alive.push(boxed);
    }
}

/// Opaque context handle for C FFI.
#[allow(dead_code)]
pub struct ContextHandle(*mut Context);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_afpc_cache_roundtrip_and_install() {
        let source = "function f(n) { let s = 0; for (let i = 0; i < n; i++) s += i; return s; } f(100);";
        let tmp = std::env::temp_dir().join("rune_embed_afpc_roundtrip_test.cache");
        let _ = std::fs::remove_file(&tmp);

        // First run: compile, AOT compile, execute, save cache.
        {
            let mut ctx = Context::new_small();
            let bytecode = ctx.compile(source).expect("compile failed");
            let compiled_funcs = crate::afpc::aot_compile_functions(&bytecode);
            let result = ctx.eval_bytecode_owned(bytecode.clone()).expect("execute failed");
            assert_eq!(result.as_smi(), Some(4950));
            let ics = ctx.ics();
            let mut cache = crate::afpc::AfpcCache::from_runtime(bytecode, ics);
            cache.compiled_funcs = compiled_funcs;
            crate::afpc::save_afpc_cache(&tmp, &cache).expect("save failed");
        }

        // Second run: load cache, install native code, execute from bytecode.
        {
            let cache = crate::afpc::load_afpc_cache(&tmp).expect("load failed");
            let mut ctx = Context::new_small();
            cache.restore_shapes();
            if !cache.compiled_funcs.is_empty() {
                let native = crate::afpc::InstalledNativeCode::from_cache(&cache);
                ctx.install_native_code(native);
            }
            ctx.set_ics(cache.ic_table);
            let result = ctx.eval_bytecode_owned(cache.bytecode).expect("execute failed");
            assert_eq!(result.as_smi(), Some(4950));
        }

        let _ = std::fs::remove_file(&tmp);
    }
}
