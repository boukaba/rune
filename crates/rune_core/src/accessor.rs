use crate::gc::{GcHeader, SemiSpace, TAG_ACCESSOR};
use crate::value::Value;
use std::sync::atomic::Ordering;

/// An accessor property descriptor: a pair of (getter, setter) function references.
///
/// Memory layout:
///   [GcHeader(8) | getter(8) | setter(8)]
///   Total: 24 bytes
///
/// Either getter or setter may be undefined.
pub struct AccessorPair;
pub(crate) const ACCESSOR_SIZE: usize = 24;

impl AccessorPair {
    pub fn allocate(gc: &mut SemiSpace, getter: Value, setter: Value) -> *mut u8 {
        let ptr = gc.alloc(ACCESSOR_SIZE);
        unsafe {
            let hdr = ptr as *mut GcHeader;
            (*hdr).word.store(TAG_ACCESSOR, Ordering::Relaxed);
            let getter_ptr = ptr.add(std::mem::size_of::<GcHeader>()) as *mut Value;
            *getter_ptr = getter;
            let setter_ptr = ptr.add(std::mem::size_of::<GcHeader>() + 8) as *mut Value;
            *setter_ptr = setter;
        }
        ptr
    }

    pub unsafe fn getter(ptr: *mut u8) -> Value {
        unsafe { *(ptr.add(std::mem::size_of::<GcHeader>()) as *const Value) }
    }

    pub unsafe fn setter(ptr: *mut u8) -> Value {
        unsafe { *(ptr.add(std::mem::size_of::<GcHeader>() + 8) as *const Value) }
    }
}
