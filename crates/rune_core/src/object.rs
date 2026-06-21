use crate::gc::{GcHeader, SemiSpace, TAG_OBJECT, size_of};
use crate::shape::Shape;
use crate::value::Value;
use std::ptr;

/// Memory layout offsets (in bytes) for a JSObject:
///   [0..8)  GcHeader
///   [8..16) shape: *const Shape
///   [16..20) slot_count: u32
///   [20..24) padding
///   [24..)  slots: Value[]
pub const OBJECT_HEADER_END: usize = 24;

/// A GC-allocated JavaScript object.
pub struct JSObject;

impl JSObject {
    pub fn allocate(ss: &mut SemiSpace, shape: &'static Shape, slot_values: &[Value]) -> *mut JSObject {
        let slot_count = slot_values.len();
        let obj_size = OBJECT_HEADER_END + slot_count * size_of::<Value>();
        let ptr = ss.alloc(obj_size) as *mut u8;
        unsafe {
            let header = &mut *(ptr as *mut GcHeader);
            header.word = std::sync::atomic::AtomicU64::new(TAG_OBJECT);

            let shape_ptr = ptr.add(8) as *mut *const Shape;
            *shape_ptr = shape as *const Shape;

            let sc_ptr = ptr.add(16) as *mut u32;
            *sc_ptr = slot_count as u32;

            let slots_ptr = ptr.add(OBJECT_HEADER_END) as *mut Value;
            ptr::copy_nonoverlapping(slot_values.as_ptr(), slots_ptr, slot_count);
        }
        ptr as *mut JSObject
    }

    pub unsafe fn slot_count(ptr: *mut JSObject) -> usize {
        unsafe {
            let ptr = ptr as *mut u8;
            let sc_ptr = ptr.add(16) as *const u32;
            *sc_ptr as usize
        }
    }

    pub unsafe fn shape_ptr(ptr: *mut JSObject) -> &'static Shape {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let shape_ptr_ptr = ptr_bytes.add(8) as *const *const Shape;
            let shape_ptr = *shape_ptr_ptr;
            if shape_ptr.is_null() {
                panic!("shape pointer is null for object at {:p}", ptr);
            }
            &*shape_ptr
        }
    }

    pub unsafe fn slots_ptr(ptr: *mut JSObject) -> *mut Value {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            ptr_bytes.add(OBJECT_HEADER_END) as *mut Value
        }
    }

    pub unsafe fn get_slot(ptr: *mut JSObject, index: usize) -> Value {
        unsafe { *Self::slots_ptr(ptr).add(index) }
    }

    pub unsafe fn set_slot(ptr: *mut JSObject, index: usize, val: Value) {
        unsafe {
            *Self::slots_ptr(ptr).add(index) = val;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gc::SemiSpace;
    use crate::shape::Shape;

    #[test]
    fn test_alloc_object() {
        let mut ss = SemiSpace::new();
        let shape = Shape::empty();
        let obj = JSObject::allocate(&mut ss, shape, &[]);
        unsafe {
            assert_eq!(JSObject::slot_count(obj), 0);
        }
    }

    #[test]
    fn test_object_with_slots() {
        let mut ss = SemiSpace::new();
        let shape = Shape::empty();
        let vals = vec![Value::smi(42), Value::smi(100)];
        let obj = JSObject::allocate(&mut ss, shape, &vals);
        unsafe {
            assert_eq!(JSObject::slot_count(obj), 2);
            assert_eq!(JSObject::get_slot(obj, 0).as_smi(), Some(42));
            assert_eq!(JSObject::get_slot(obj, 1).as_smi(), Some(100));
        }
    }
}
