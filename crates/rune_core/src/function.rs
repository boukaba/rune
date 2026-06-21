use crate::gc::{GcHeader, SemiSpace, TAG_FUNC, size_of};

/// A GC-allocated function object storing an index into the bytecode function table,
/// a pointer to the bytecode program that owns the function table,
/// and a pointer to the function's `.prototype` object.
///
/// Memory layout:
///   [GcHeader(8) | func_idx(8) | prog_ptr(8) | prototype(8) |
///    call_count(4) | padding(4) | jit_entry(8)]
///   Total: 48 bytes
pub struct Func;

impl Func {
    pub fn allocate(ss: &mut SemiSpace, func_idx: u64, prog_ptr: *const u8) -> *mut Func {
        let ptr = ss.alloc(48) as *mut u8;
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
            // call_count = 0, padding = 0
            let count_ptr = ptr.add(size_of::<GcHeader>() + 24) as *mut u32;
            *count_ptr = 0;
            let pad_ptr = ptr.add(size_of::<GcHeader>() + 28) as *mut u32;
            *pad_ptr = 0;
            // jit_entry = null
            let jit_ptr = ptr.add(size_of::<GcHeader>() + 32) as *mut u64;
            *jit_ptr = 0;
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
            *p = *p + 1;
        }
    }

    /// Set the JIT entry point. `entry` is a pointer to compiled native code.
    pub unsafe fn set_jit_entry(ptr: *mut Func, entry: *const u8) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let p = ptr_bytes.add(size_of::<GcHeader>() + 32) as *mut u64;
            *p = entry as u64;
        }
    }

    /// Get the JIT entry point. Returns null if not JIT-compiled.
    pub unsafe fn jit_entry(ptr: *mut Func) -> *const u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let raw = *(ptr_bytes.add(size_of::<GcHeader>() + 32) as *const u64);
            if raw == 0 { std::ptr::null() } else { raw as *const u8 }
        }
    }
}
