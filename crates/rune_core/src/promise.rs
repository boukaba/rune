use crate::gc::{GcHeader, SemiSpace, TAG_PROMISE};
use crate::value::Value;
use std::sync::atomic::Ordering;

pub const PROMISE_PENDING: u32 = 0;
pub const PROMISE_FULFILLED: u32 = 1;
pub const PROMISE_REJECTED: u32 = 2;

pub struct Promise;
pub(crate) const PROMISE_SIZE: usize = 32;

impl Promise {
    pub fn allocate(gc: &mut SemiSpace, proto: Option<*mut u8>) -> *mut u8 {
        let ptr = gc.alloc(PROMISE_SIZE);
        unsafe {
            let hdr = ptr as *mut GcHeader;
            (*hdr).word.store(TAG_PROMISE, Ordering::Relaxed);
            let state_ptr = ptr.add(std::mem::size_of::<GcHeader>()) as *mut u32;
            *state_ptr = PROMISE_PENDING;
            let result_ptr = ptr.add(std::mem::size_of::<GcHeader>() + 8) as *mut Value;
            *result_ptr = Value::undefined();
            let proto_ptr = ptr.add(std::mem::size_of::<GcHeader>() + 16) as *mut *mut u8;
            *proto_ptr = proto.unwrap_or(std::ptr::null_mut());
        }
        ptr
    }

    pub unsafe fn state(ptr: *mut u8) -> u32 {
        unsafe { *(ptr.add(std::mem::size_of::<GcHeader>()) as *const u32) }
    }

    pub unsafe fn set_state(ptr: *mut u8, s: u32) {
        unsafe { *(ptr.add(std::mem::size_of::<GcHeader>()) as *mut u32) = s; }
    }

    pub unsafe fn result(ptr: *mut u8) -> Value {
        unsafe { *(ptr.add(std::mem::size_of::<GcHeader>() + 8) as *const Value) }
    }

    pub unsafe fn set_result(ptr: *mut u8, v: Value) {
        unsafe { *(ptr.add(std::mem::size_of::<GcHeader>() + 8) as *mut Value) = v; }
    }

    pub unsafe fn prototype(ptr: *mut u8) -> *mut u8 {
        unsafe { *(ptr.add(std::mem::size_of::<GcHeader>() + 16) as *const *mut u8) }
    }

    pub unsafe fn set_prototype(ptr: *mut u8, proto: *mut u8) {
        unsafe { *(ptr.add(std::mem::size_of::<GcHeader>() + 16) as *mut *mut u8) = proto; }
    }
}
