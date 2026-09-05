use crate::gc::{GcHeader, SemiSpace, TAG_FUNC, size_of};

/// A GC-allocated function object storing an index into the bytecode function table,
/// a pointer to the bytecode program that owns the function table,
/// a pointer to the function's `.prototype` object,
/// and a pointer to the function's captured lexical environment.
///
/// Memory layout:
///   [GcHeader(8) | func_idx(8) | prog_ptr(8) | prototype(8) |
///    call_count(4) | flags(4) | env_ptr(8) | jit_entry(8) | superclass(8) | extra_props(8) |
///    private_name_ids(8)]
///   Total: 80 bytes
///
/// flags word bit plan (F1 — carved from the old `module_mi << 1` layout;
/// module ids never approach 2^27, so the high bits were wasted):
///   bit 0 = is_arrow (not constructable, no prototype)
///   bit 1 = is_strict (strict-mode function; reserved — no producer yet,
///           needed by A3 strict caller/arguments poison checks)
///   bit 2 = is_class_constructor ([[FunctionKind]] == classConstructor;
///           set by emit_class for constructor programs, read by A3 Call check)
///   bit 3 = is_generator (mirrors BytecodeProgram.is_generator)
///   bit 4 = is_async (mirrors BytecodeProgram.is_async)
///   bits 5..=31 = module_mi + 1 (0 = none; same bias as before, new shift)
pub struct Func;

/// Low-5-bit function-kind flags (see bit plan above).
pub const FUNC_FLAG_ARROW: u32 = 1 << 0;
pub const FUNC_FLAG_STRICT: u32 = 1 << 1;
pub const FUNC_FLAG_CLASS_CTOR: u32 = 1 << 2;
pub const FUNC_FLAG_GENERATOR: u32 = 1 << 3;
pub const FUNC_FLAG_ASYNC: u32 = 1 << 4;
/// Bits reserved for kind flags; everything above is the module record index.
pub const FUNC_FLAG_MASK: u32 = 0x1F;
/// Shift of the biased module index (`module_mi + 1`) within the flags word.
pub const FUNC_MODULE_MI_SHIFT: u32 = 5;

impl Func {
    pub fn allocate(
        ss: &mut SemiSpace,
        func_idx: u64,
        prog_ptr: *const u8,
        is_arrow: bool,
        env_ptr: *mut u8,
    ) -> *mut Func {
        let ptr = ss.alloc(80);
        unsafe {
            let header = &mut *(ptr as *mut GcHeader);
            header.word = std::sync::atomic::AtomicU64::new(TAG_FUNC);
            let idx_ptr = ptr.add(size_of::<GcHeader>()) as *mut u64;
            *idx_ptr = func_idx;
            let prog_ptr_ptr = ptr.add(size_of::<GcHeader>() + 8) as *mut u64;
            *prog_ptr_ptr = prog_ptr as u64;
            // prototype starts as null; set by MakeFunction
            let proto_ptr = ptr.add(size_of::<GcHeader>() + 16) as *mut u64;
            *proto_ptr = 0;
            // call_count = 0
            let count_ptr = ptr.add(size_of::<GcHeader>() + 24) as *mut u32;
            *count_ptr = 0;
            // flags: only the arrow bit; kind bits default off, module_mi
            // defaults to -1 (all high bits zero = biased 0)
            let flags_ptr = ptr.add(size_of::<GcHeader>() + 28) as *mut u32;
            *flags_ptr = if is_arrow { FUNC_FLAG_ARROW } else { 0 };
            // env_ptr = captured lexical environment
            let env_field_ptr = ptr.add(size_of::<GcHeader>() + 32) as *mut u64;
            *env_field_ptr = env_ptr as u64;
            // jit_entry = null
            let jit_ptr = ptr.add(size_of::<GcHeader>() + 40) as *mut u64;
            *jit_ptr = 0;
            // superclass = null
            let super_ptr = ptr.add(size_of::<GcHeader>() + 48) as *mut u64;
            *super_ptr = 0;
            // extra_props = null (lazily allocated extensible object for arbitrary properties)
            let props_ptr = ptr.add(size_of::<GcHeader>() + 56) as *mut u64;
            *props_ptr = 0;
            // private_name_ids = null (set by PrivateNameScope during class evaluation)
            let priv_ids_ptr = ptr.add(size_of::<GcHeader>() + 64) as *mut u64;
            *priv_ids_ptr = 0;
        }
        ptr as *mut Func
    }

    pub unsafe fn func_index(ptr: *mut Func) -> u64 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            *(ptr_bytes.add(size_of::<GcHeader>()) as *const u64)
        }
    }

    pub unsafe fn prog_ptr(ptr: *mut Func) -> *const u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            *(ptr_bytes.add(size_of::<GcHeader>() + 8) as *const u64) as *const u8
        }
    }

    /// The owning module record index, or -1 if this function was created
    /// outside module evaluation. Stored biased (+1) in bits 5..=31 of the
    /// flags word (low 5 bits are kind flags). Set by MakeFunction while a
    /// module program runs; used by LoadGlobal/StoreGlobal to resolve
    /// module bindings.
    pub unsafe fn module_mi(ptr: *mut Func) -> i32 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let flags = *(ptr_bytes.add(size_of::<GcHeader>() + 28) as *const u32);
            ((flags >> FUNC_MODULE_MI_SHIFT) as i32) - 1
        }
    }

    pub unsafe fn set_module_mi(ptr: *mut Func, mi: i32) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let flags_ptr = ptr_bytes.add(size_of::<GcHeader>() + 28) as *mut u32;
            let flags = *flags_ptr;
            *flags_ptr = (flags & FUNC_FLAG_MASK) | (((mi + 1) as u32) << FUNC_MODULE_MI_SHIFT);
        }
    }

    /// Get the captured environment pointer (may be null).
    pub unsafe fn env_ptr(ptr: *mut Func) -> *mut u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            *(ptr_bytes.add(size_of::<GcHeader>() + 32) as *const u64) as *mut u8
        }
    }

    /// Set the captured environment pointer.
    pub unsafe fn set_env_ptr(ptr: *mut Func, env: *mut u8) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let field = ptr_bytes.add(size_of::<GcHeader>() + 32) as *mut u64;
            *field = env as u64;
        }
    }

    /// Get the prototype pointer. Returns null if no prototype has been set.
    pub unsafe fn prototype(ptr: *mut Func) -> *mut u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            *(ptr_bytes.add(size_of::<GcHeader>() + 16) as *const u64) as *mut u8
        }
    }

    /// Set the prototype pointer.
    pub unsafe fn set_prototype(ptr: *mut Func, proto: *mut u8) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let proto_ptr = ptr_bytes.add(size_of::<GcHeader>() + 16) as *mut u64;
            *proto_ptr = proto as u64;
        }
    }

    pub unsafe fn gc_header(ptr: *mut Func) -> *mut GcHeader {
        ptr as *mut GcHeader
    }

    /// Check if this function is an arrow function (not constructable).
    pub unsafe fn is_arrow(ptr: *mut Func) -> bool {
        unsafe { Self::has_flag(ptr, FUNC_FLAG_ARROW) }
    }

    /// Test one kind-flag bit.
    unsafe fn has_flag(ptr: *mut Func, flag: u32) -> bool {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let flags = *(ptr_bytes.add(size_of::<GcHeader>() + 28) as *const u32);
            flags & flag != 0
        }
    }

    /// Set or clear one kind-flag bit, preserving the rest of the word.
    unsafe fn set_flag(ptr: *mut Func, flag: u32, on: bool) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let flags_ptr = ptr_bytes.add(size_of::<GcHeader>() + 28) as *mut u32;
            let flags = *flags_ptr;
            *flags_ptr = if on { flags | flag } else { flags & !flag };
        }
    }

    /// OR a set of kind-flag bits in (used by MakeFunction from the
    /// compiled-program record + instruction operand).
    pub unsafe fn add_flags(ptr: *mut Func, flags: u32) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let flags_ptr = ptr_bytes.add(size_of::<GcHeader>() + 28) as *mut u32;
            *flags_ptr |= flags & FUNC_FLAG_MASK;
        }
    }

    /// Strict-mode function (no producer yet — reserved for A3).
    pub unsafe fn is_strict(ptr: *mut Func) -> bool {
        unsafe { Self::has_flag(ptr, FUNC_FLAG_STRICT) }
    }

    pub unsafe fn set_strict(ptr: *mut Func, on: bool) {
        unsafe { Self::set_flag(ptr, FUNC_FLAG_STRICT, on) }
    }

    /// Class constructor ([[FunctionKind]] == classConstructor).
    pub unsafe fn is_class_constructor(ptr: *mut Func) -> bool {
        unsafe { Self::has_flag(ptr, FUNC_FLAG_CLASS_CTOR) }
    }

    pub unsafe fn set_class_constructor(ptr: *mut Func, on: bool) {
        unsafe { Self::set_flag(ptr, FUNC_FLAG_CLASS_CTOR, on) }
    }

    /// Generator function (mirrors the compiled-program record).
    pub unsafe fn is_generator_fn(ptr: *mut Func) -> bool {
        unsafe { Self::has_flag(ptr, FUNC_FLAG_GENERATOR) }
    }

    pub unsafe fn set_generator_fn(ptr: *mut Func, on: bool) {
        unsafe { Self::set_flag(ptr, FUNC_FLAG_GENERATOR, on) }
    }

    /// Async function (mirrors the compiled-program record).
    pub unsafe fn is_async_fn(ptr: *mut Func) -> bool {
        unsafe { Self::has_flag(ptr, FUNC_FLAG_ASYNC) }
    }

    pub unsafe fn set_async_fn(ptr: *mut Func, on: bool) {
        unsafe { Self::set_flag(ptr, FUNC_FLAG_ASYNC, on) }
    }

    /// Get the call count.
    pub unsafe fn call_count(ptr: *mut Func) -> u32 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            *(ptr_bytes.add(size_of::<GcHeader>() + 24) as *const u32)
        }
    }

    /// Increment the call count.
    pub unsafe fn increment_call_count(ptr: *mut Func) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let p = ptr_bytes.add(size_of::<GcHeader>() + 24) as *mut u32;
            *p += 1;
        }
    }

    /// Set the JIT entry point. `entry` is a pointer to compiled native code.
    pub unsafe fn set_jit_entry(ptr: *mut Func, entry: *const u8) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let p = ptr_bytes.add(size_of::<GcHeader>() + 40) as *mut u64;
            *p = entry as u64;
        }
    }

    /// Get the JIT entry point. Returns null if not JIT-compiled.
    pub unsafe fn jit_entry(ptr: *mut Func) -> *const u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let raw = *(ptr_bytes.add(size_of::<GcHeader>() + 40) as *const u64);
            if raw == 0 {
                std::ptr::null()
            } else {
                raw as *const u8
            }
        }
    }

    /// Get the superclass constructor pointer (for extends). Returns null if no superclass.
    pub unsafe fn superclass(ptr: *mut Func) -> *mut u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            *(ptr_bytes.add(size_of::<GcHeader>() + 48) as *const u64) as *mut u8
        }
    }

    /// Set the superclass constructor pointer.
    pub unsafe fn set_superclass(ptr: *mut Func, superclass: *mut u8) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let field = ptr_bytes.add(size_of::<GcHeader>() + 48) as *mut u64;
            *field = superclass as u64;
        }
    }

    /// Get the extra properties object pointer (may be null).
    pub unsafe fn extra_props(ptr: *mut Func) -> *mut u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            *(ptr_bytes.add(size_of::<GcHeader>() + 56) as *const u64) as *mut u8
        }
    }

    /// Set the extra properties object pointer.
    pub unsafe fn set_extra_props(ptr: *mut Func, props: *mut u8) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let field = ptr_bytes.add(size_of::<GcHeader>() + 56) as *mut u64;
            *field = props as u64;
        }
    }

    /// Get the private name IDs pointer (set by PrivateNameScope during class evaluation).
    pub unsafe fn private_name_ids(ptr: *mut Func) -> *mut u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            *(ptr_bytes.add(size_of::<GcHeader>() + 64) as *const u64) as *mut u8
        }
    }

    /// Set the private name IDs pointer.
    pub unsafe fn set_private_name_ids(ptr: *mut Func, ids: *mut u8) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let field = ptr_bytes.add(size_of::<GcHeader>() + 64) as *mut u64;
            *field = ids as u64;
        }
    }
}
