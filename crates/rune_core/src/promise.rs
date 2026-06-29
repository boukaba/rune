use crate::gc::{GcHeader, SemiSpace, TAG_PROMISE};
use crate::value::Value;
use std::sync::atomic::Ordering;

pub const PROMISE_PENDING: u32 = 0;
pub const PROMISE_FULFILLED: u32 = 1;
pub const PROMISE_REJECTED: u32 = 2;

/// Promise heap object layout.
///
/// ```text
/// [ GcHeader(TAG_PROMISE) | state: u32 | _pad: u32 | result: Value | prototype: *mut u8 | reactions: *mut u8 ]
/// ```
///
/// reactions points to a TAG_ARRAY of [callback, chained_promise] pairs.
///
/// Size: 40 bytes.
pub struct Promise;
pub(crate) const PROMISE_SIZE: usize = 40;

impl Promise {
    pub fn allocate(gc: &mut SemiSpace, proto: Option<*mut u8>) -> *mut u8 {
        use crate::array::RuneArray;
        let ptr = gc.alloc(PROMISE_SIZE);
        unsafe {
            let hdr = ptr as *mut GcHeader;
            (*hdr).word.store(TAG_PROMISE, Ordering::Relaxed);
        }
        let reactions_arr = RuneArray::allocate(gc, &[]);
        let ptr = unsafe {
            if (*(ptr as *const GcHeader)).is_forwarded() {
                (*(ptr as *const GcHeader)).forwarding_addr()
            } else { ptr }
        };
        // Re-extract proto AFTER GC (it may have been moved)
        let proto_resolved = proto.map(|p| unsafe {
            if (*(p as *const GcHeader)).is_forwarded() {
                (*(p as *const GcHeader)).forwarding_addr()
            } else { p }
        });
        unsafe {
            let state_ptr = ptr.add(std::mem::size_of::<GcHeader>()) as *mut u32;
            *state_ptr = PROMISE_PENDING;
            let result_ptr = ptr.add(std::mem::size_of::<GcHeader>() + 8) as *mut Value;
            *result_ptr = Value::undefined();
            let proto_ptr = ptr.add(std::mem::size_of::<GcHeader>() + 16) as *mut *mut u8;
            *proto_ptr = proto_resolved.unwrap_or(std::ptr::null_mut());
            let reactions_ptr = ptr.add(std::mem::size_of::<GcHeader>() + 24) as *mut *mut u8;
            *reactions_ptr = reactions_arr as *mut u8;
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

    pub unsafe fn reactions(ptr: *mut u8) -> *mut u8 {
        unsafe { *(ptr.add(std::mem::size_of::<GcHeader>() + 24) as *const *mut u8) }
    }
}
