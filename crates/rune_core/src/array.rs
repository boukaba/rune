use crate::gc::{GcHeader, SemiSpace, TAG_ARRAY, size_of};
use crate::value::Value;
use std::ptr;

/// Memory layout (identical byte layout to JSObject):
///   [0..8)   GcHeader with TAG_ARRAY
///   [8..16)  shape: *const Shape (DENSE_ARRAY_SHAPE for all arrays)
///   [16..20) length: u32 (number of elements)
///   [20..24) capacity: u32 (allocated element capacity)
///   [24..32) prototype: *mut u8 (Array.prototype, set by VM)
///   [32..)   elements: Value[]
///
/// Reuses OBJECT_HEADER_END (32) and OBJECT_PROTOTYPE_OFFSET (24) from object.rs
pub struct RuneArray;

/// Number of extra element slots to reserve beyond initial length.
const RESERVED_ELEMENTS: usize = 4;

impl RuneArray {
    /// Allocate a dense array with the given elements.
    pub fn allocate(ss: &mut SemiSpace, elements: &[Value]) -> *mut RuneArray {
        let len = elements.len();
        let cap = len + RESERVED_ELEMENTS;
        let total_size = crate::object::OBJECT_HEADER_END + cap * size_of::<Value>();
        let ptr = ss.alloc(total_size) as *mut u8;
        unsafe {
            let header = &mut *(ptr as *mut GcHeader);
            header.word = std::sync::atomic::AtomicU64::new(TAG_ARRAY);

            // Shape pointer — DENSE_ARRAY_SHAPE, set externally via set_shape
            let shape_ptr = ptr.add(8) as *mut *const u8;
            *shape_ptr = std::ptr::null_mut();

            let len_ptr = ptr.add(16) as *mut u32;
            *len_ptr = len as u32;

            let cap_ptr = ptr.add(20) as *mut u32;
            *cap_ptr = cap as u32;

            // Prototype starts as null (set externally)
            let proto_ptr = ptr.add(24) as *mut *mut u8;
            *proto_ptr = std::ptr::null_mut();

            let elems_ptr = ptr.add(crate::object::OBJECT_HEADER_END) as *mut Value;
            ptr::copy_nonoverlapping(elements.as_ptr(), elems_ptr, len);
            // Zero out reserved elements
            for i in len..cap {
                *elems_ptr.add(i) = Value::undefined();
            }
        }
        ptr as *mut RuneArray
    }

    pub unsafe fn length(arr: *mut RuneArray) -> u32 {
        unsafe { *((arr as *mut u8).add(16) as *const u32) }
    }

    pub unsafe fn set_length(arr: *mut RuneArray, n: u32) {
        unsafe { *((arr as *mut u8).add(16) as *mut u32) = n; }
    }

    pub unsafe fn capacity(arr: *mut RuneArray) -> u32 {
        unsafe { *((arr as *mut u8).add(20) as *const u32) }
    }

    pub unsafe fn get_element(arr: *mut RuneArray, index: usize) -> Value {
        unsafe {
            let elems_ptr = (arr as *mut u8).add(crate::object::OBJECT_HEADER_END) as *const Value;
            *elems_ptr.add(index)
        }
    }

    pub unsafe fn set_element(arr: *mut RuneArray, index: usize, val: Value) {
        unsafe {
            let elems_ptr = (arr as *mut u8).add(crate::object::OBJECT_HEADER_END) as *mut Value;
            *elems_ptr.add(index) = val;
        }
    }

    /// Push a value to the end of the array.
    /// Returns false if there's no reserved capacity left.
    pub unsafe fn push(_ss: &mut SemiSpace, arr: *mut RuneArray, val: Value) -> bool {
        unsafe {
            let len = Self::length(arr);
            let cap = Self::capacity(arr);
            if (len as usize) < cap as usize {
                Self::set_element(arr, len as usize, val);
                Self::set_length(arr, len + 1);
                true
            } else {
                false // OOM — caller should grow or realloc
            }
        }
    }

    /// Pop the last element. Returns undefined for empty arrays.
    pub unsafe fn pop(arr: *mut RuneArray) -> Value {
        unsafe {
            let len = Self::length(arr);
            if len == 0 {
                return Value::undefined();
            }
            let new_len = len - 1;
            let val = Self::get_element(arr, new_len as usize);
            Self::set_length(arr, new_len);
            val
        }
    }
}
