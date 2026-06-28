use crate::gc::{GcHeader, SemiSpace};
use crate::value::Value;

pub const TAG_STRING_OBJ: u64 = 6;
pub const STRING_OBJ_HEADER_SIZE: usize = 8;
pub const STRING_OBJ_PROTOTYPE_OFFSET: usize = 8;
pub const STRING_OBJ_STRING_PTR_OFFSET: usize = 16;
pub const STRING_OBJ_TOTAL_SIZE: usize = 24;

pub struct StringObject;

impl StringObject {
    pub fn allocate(
        ss: &mut SemiSpace,
        string_ptr: *mut u8,
        prototype: Value,
    ) -> *mut StringObject {
        let ptr = ss.alloc(STRING_OBJ_TOTAL_SIZE);
        unsafe {
            let header = &mut *(ptr as *mut GcHeader);
            header.word = std::sync::atomic::AtomicU64::new(TAG_STRING_OBJ);

            let proto_ptr = ptr.add(STRING_OBJ_PROTOTYPE_OFFSET) as *mut u64;
            if let Some(p) = prototype.heap_ptr() {
                *proto_ptr = p as u64;
            } else {
                *proto_ptr = 0;
            }

            let str_ptr = ptr.add(STRING_OBJ_STRING_PTR_OFFSET) as *mut u64;
            *str_ptr = string_ptr as u64;
        }
        ptr as *mut StringObject
    }

    pub unsafe fn prototype(ptr: *mut StringObject) -> *mut u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let proto_ptr = ptr_bytes.add(STRING_OBJ_PROTOTYPE_OFFSET) as *const u64;
            *proto_ptr as *mut u8
        }
    }

    pub unsafe fn set_prototype(ptr: *mut StringObject, proto: *mut u8) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let proto_ptr = ptr_bytes.add(STRING_OBJ_PROTOTYPE_OFFSET) as *mut u64;
            *proto_ptr = proto as u64;
        }
    }

    pub unsafe fn string_ptr(ptr: *mut StringObject) -> *mut u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let str_ptr = ptr_bytes.add(STRING_OBJ_STRING_PTR_OFFSET) as *const u64;
            *str_ptr as *mut u8
        }
    }
}
