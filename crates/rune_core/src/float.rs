use crate::gc::{GcHeader, SemiSpace, TAG_FLOAT64, size_of};

/// A GC-allocated double-precision floating point number.
///
/// Memory layout:
///   [GcHeader(8) | f64 value(8)]
///
/// TODO Phase 5: Replace with NaN-boxing to inline f64 directly in Value,
/// eliminating GC allocation for every float operation.
pub struct HeapFloat64;

impl HeapFloat64 {
    pub fn allocate(ss: &mut SemiSpace, val: f64) -> *mut HeapFloat64 {
        let obj_size = size_of::<GcHeader>() + size_of::<f64>();
        let ptr = ss.alloc(obj_size) as *mut u8;
        unsafe {
            let header = &mut *(ptr as *mut GcHeader);
            header.word = std::sync::atomic::AtomicU64::new(TAG_FLOAT64);

            let val_ptr = ptr.add(size_of::<GcHeader>()) as *mut f64;
            *val_ptr = val;
        }
        ptr as *mut HeapFloat64
    }

    pub unsafe fn from_ptr(ptr: *mut HeapFloat64) -> &'static Self {
        unsafe { &*ptr }
    }

    pub unsafe fn value(ptr: *mut HeapFloat64) -> f64 {
        unsafe {
            let ptr = ptr as *mut u8;
            let val_ptr = ptr.add(size_of::<GcHeader>()) as *const f64;
            *val_ptr
        }
    }
}
