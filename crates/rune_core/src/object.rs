use crate::gc::{GcHeader, SemiSpace, TAG_OBJECT, size_of};
use crate::shape::Shape;
use crate::value::Value;
use std::ptr;

/// Memory layout offsets (in bytes) for a JSObject:
///   [0..8)   GcHeader
///   [8..16)  shape: *const Shape
///   [16..20) capacity: u32 (total slot capacity including reserved)
///   [20..24) slot_count: u32 (used slots)
///   [24..32) prototype: *mut u8 (heap pointer or null)
///   [32..)   slots: Value[]
pub const OBJECT_HEADER_END: usize = 32;

/// Number of extra slots to reserve beyond the initial shape's property count.
const RESERVED_SLOTS: usize = 4;

/// A GC-allocated JavaScript object.
pub struct JSObject;

impl JSObject {
    pub fn allocate(ss: &mut SemiSpace, shape: &'static Shape, slot_values: &[Value]) -> *mut JSObject {
        let slot_count = slot_values.len();
        let capacity = slot_count + RESERVED_SLOTS;
        let obj_size = OBJECT_HEADER_END + capacity * size_of::<Value>();
        let ptr = ss.alloc(obj_size) as *mut u8;
        unsafe {
            let header = &mut *(ptr as *mut GcHeader);
            header.word = std::sync::atomic::AtomicU64::new(TAG_OBJECT);

            let shape_ptr = ptr.add(8) as *mut *const Shape;
            *shape_ptr = shape as *const Shape;

            let cap_ptr = ptr.add(16) as *mut u32;
            *cap_ptr = capacity as u32;

            let sc_ptr = ptr.add(20) as *mut u32;
            *sc_ptr = slot_count as u32;

            // Prototype starts as null (no prototype)
            let proto_ptr = ptr.add(24) as *mut *mut u8;
            *proto_ptr = std::ptr::null_mut();

            let slots_ptr = ptr.add(OBJECT_HEADER_END) as *mut Value;
            ptr::copy_nonoverlapping(slot_values.as_ptr(), slots_ptr, slot_count);
            // Zero out reserved slots
            for i in slot_count..capacity {
                *slots_ptr.add(i) = Value::undefined();
            }
        }
        ptr as *mut JSObject
    }

    pub unsafe fn capacity(ptr: *mut JSObject) -> usize {
        unsafe {
            let ptr = ptr as *mut u8;
            let cap_ptr = ptr.add(16) as *const u32;
            *cap_ptr as usize
        }
    }

    pub unsafe fn slot_count(ptr: *mut JSObject) -> usize {
        unsafe {
            let ptr = ptr as *mut u8;
            let sc_ptr = ptr.add(20) as *const u32;
            *sc_ptr as usize
        }
    }

    unsafe fn set_slot_count(ptr: *mut JSObject, n: usize) {
        unsafe {
            let ptr = ptr as *mut u8;
            let sc_ptr = ptr.add(20) as *mut u32;
            *sc_ptr = n as u32;
        }
    }

    unsafe fn set_shape_ptr(ptr: *mut JSObject, shape: &'static Shape) {
        unsafe {
            let ptr = ptr as *mut u8;
            let shape_ptr_ptr = ptr.add(8) as *mut *const Shape;
            *shape_ptr_ptr = shape as *const Shape;
        }
    }

    pub unsafe fn prototype(ptr: *mut JSObject) -> *mut u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let proto_ptr = ptr_bytes.add(24) as *const *mut u8;
            *proto_ptr
        }
    }

    pub unsafe fn set_prototype(ptr: *mut JSObject, proto: *mut u8) {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let proto_ptr = ptr_bytes.add(24) as *mut *mut u8;
            *proto_ptr = proto;
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

    /// Remove a property from the object. Returns true if the property was found and removed.
    pub unsafe fn remove_property(ptr: *mut JSObject, key: &crate::shape::PropertyKey) -> bool {
        unsafe {
            let shape = Self::shape_ptr(ptr);
            if let Some(slot) = shape.lookup(key) {
                let count = Self::slot_count(ptr);
                let mut new_entries: Vec<(crate::shape::PropertyKey, usize)> = Vec::with_capacity(count - 1);
                let mut new_key_names: Vec<String> = Vec::with_capacity(count - 1);
                for i in 0..count {
                    if i == slot { continue; }
                    let new_offset = new_entries.len();
                    new_entries.push((shape.entries[i].0, new_offset));
                    new_key_names.push(shape.key_names[i].clone());
                }
                let new_shape = crate::shape::Shape::intern(new_entries, new_key_names);
                Self::set_shape_ptr(ptr, new_shape);
                for i in slot..(count - 1) {
                    Self::set_slot(ptr, i, Self::get_slot(ptr, i + 1));
                }
                Self::set_slot_count(ptr, count - 1);
                true
            } else {
                false
            }
        }
    }

    /// Add a new property to the object in place, extending the shape and slot array.
    /// Returns the slot index of the new property.
    /// Panics if the object has no reserved capacity left.
    pub unsafe fn add_property(ptr: *mut JSObject, key: crate::shape::PropertyKey, key_name: String, val: Value) -> usize {
        unsafe {
            let cap = Self::capacity(ptr);
            let count = Self::slot_count(ptr);
            assert!(count < cap, "JSObject: out of reserved slot capacity");
            let shape = Self::shape_ptr(ptr);
            let new_shape = Shape::intern_with_parent(shape, key, key_name);
            Self::set_shape_ptr(ptr, new_shape);
            Self::set_slot(ptr, count, val);
            Self::set_slot_count(ptr, count + 1);
            count
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
