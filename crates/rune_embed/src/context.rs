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
    /// Enable JIT-to-JIT inlining for hot callees (Phase F).
    /// Default: false during F-0/F-1; flipped to true when F-2 lands.
    pub enable_inlining: bool,
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
            enable_inlining: false,
        };
        // Register default builtins
        for b in rune_interpreter::builtins::default_builtins() {
            ctx.vm.register_builtin(b.name, b.func);
        }
        // Build constructor wrappers (Object.create, etc.)
        ctx.vm.init_builtin_wrappers(&mut ctx.gc);
        // Pre-allocate typeof result strings for JIT typeof helper.
        ctx.vm.typeof_strings = [
            ctx.allocate_string("number"),
            ctx.allocate_string("string"),
            ctx.allocate_string("boolean"),
            ctx.allocate_string("undefined"),
            ctx.allocate_string("object"),
            ctx.allocate_string("function"),
        ];
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

    /// Benchmark: parse+emit vs cache load for a realistic 128-line program.
    /// Run with: cargo test -p rune_embed bench_real_cache --release -- --nocapture
    #[test]
    fn bench_real_cache() {
        let src = r#"
function range(start, end) { var arr = []; for (var i = start; i < end; i = i + 1) { arr.push(i); } return arr; }
function sum(arr) { var s = 0; for (var i = 0; i < arr.length; i = i + 1) { s = s + arr[i]; } return s; }
function map(arr, fn) { var out = []; for (var i = 0; i < arr.length; i = i + 1) { out.push(fn(arr[i])); } return out; }
function filter(arr, fn) { var out = []; for (var i = 0; i < arr.length; i = i + 1) { if (fn(arr[i])) out.push(arr[i]); } return out; }
function square(x) { return x * x; }
function cube(x) { return x * x * x; }
function isEven(x) { return x % 2 === 0; }
function Point(x, y) { this.x = x; this.y = y; }
Point.prototype.distance = function() { return this.x * this.x + this.y * this.y; };
Point.prototype.add = function(p) { return new Point(this.x + p.x, this.y + p.y); };
function Vector3(x, y, z) { this.x = x; this.y = y; this.z = z; }
Vector3.prototype.length = function() { return this.x * this.x + this.y * this.y + this.z * this.z; };
Vector3.prototype.dot = function(v) { return this.x * v.x + this.y * v.y + this.z * v.z; };
function factorial(n) { if (n <= 1) return 1; return n * factorial(n - 1); }
function fibonacci(n) { if (n <= 1) return n; return fibonacci(n - 1) + fibonacci(n - 2); }
function gcd(a, b) { if (b === 0) return a; return gcd(b, a % b); }
function isPrime(n) { if (n < 2) return false; for (var i = 2; i * i <= n; i = i + 1) { if (n % i === 0) return false; } return true; }
var nums = range(1, 100);
var s = sum(map(nums, square));
var r = sum(filter(nums, isEven));
var p1 = new Point(3, 4);
var d = p1.distance();
var v1 = new Vector3(1, 2, 3);
var len = v1.length();
var f10 = factorial(10);
var fib20 = fibonacci(20);
s + r + d + len + f10 + fib20;
"#;
        let cache_path = std::env::temp_dir().join("rune_real_bench.cache");
        let _ = std::fs::remove_file(&cache_path);

        let n = 500;

        // 1. Compile only (parse + emit, no execute)
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            let ctx = Context::new_small();
            ctx.compile(src).unwrap();
        }
        let compile_us = t0.elapsed().as_micros() as f64 / n as f64;

        // 2. Build + save cache once, then load from disk
        {
            let ctx = Context::new_small();
            let bc = ctx.compile(src).unwrap();
            let cache = crate::afpc::AfpcCache::from_runtime(bc, vec![]);
            crate::afpc::save_afpc_cache(&cache_path, &cache).unwrap();
        }
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            let _cache = crate::afpc::load_afpc_cache(&cache_path).unwrap();
        }
        let load_us = t0.elapsed().as_micros() as f64 / n as f64;

        // 3. Compile + execute (full cold)
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            let mut ctx = Context::new_small();
            ctx.eval(src).unwrap();
        }
        let cold_us = t0.elapsed().as_micros() as f64 / n as f64;

        // 4. Cache load + execute
        let t0 = std::time::Instant::now();
        for _ in 0..n {
            let cache = crate::afpc::load_afpc_cache(&cache_path).unwrap();
            let mut ctx = Context::new_small();
            cache.restore_shapes();
            ctx.set_ics(cache.ic_table.clone());
            ctx.eval_bytecode_owned(cache.bytecode).unwrap();
        }
        let cached_us = t0.elapsed().as_micros() as f64 / n as f64;

        eprintln!("\n╔══════════════════════════════════════════╗");
        eprintln!("║  AFPC Cache — Real 20-fn program        ║");
        eprintln!("╠══════════════════════════════════════════╣");
        eprintln!("║  Compile (parse+emit): {:>8.1} µs       ║", compile_us);
        eprintln!("║  Cache load (disk):    {:>8.1} µs       ║", load_us);
        eprintln!("║  Speedup (compile):    {:>8.1}x          ║", compile_us / load_us);
        eprintln!("╠══════════════════════════════════════════╣");
        eprintln!("║  Cold (parse+emit+exec):{:>8.1} µs      ║", cold_us);
        eprintln!("║  Cached (load+exec):   {:>8.1} µs       ║", cached_us);
        eprintln!("║  Speedup (end-to-end): {:>8.1}x          ║", cold_us / cached_us);
        eprintln!("╚══════════════════════════════════════════╝");

        let _ = std::fs::remove_file(&cache_path);
    }
}
