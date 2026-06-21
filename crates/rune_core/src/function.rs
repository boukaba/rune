use crate::gc::{GcHeader, SemiSpace, TAG_FUNC, size_of};

/// A GC-allocated function object storing an index into the bytecode function table,
/// a pointer to the bytecode program that owns the function table,
/// and a pointer to the function's `.prototype` object.
///
/// Memory layout:
///   [GcHeader(8) | func_idx: u64(8) | prog_ptr: *const u8(8) | prototype: *mut u8(8)]
///   Total: 32 bytes
pub struct Func;

impl Func {
    pub fn allocate(ss: &mut SemiSpace, func_idx: u64, prog_ptr: *const u8) -> *mut Func {
        let ptr = ss.alloc(32) as *mut u8;
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
}
