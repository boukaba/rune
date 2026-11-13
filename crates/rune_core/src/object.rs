use crate::gc::{GcHeader, SemiSpace, TAG_OBJECT, size_of};
use crate::shape::Shape;
use crate::value::Value;

/// Memory layout offsets (in bytes) for a JSObject:
///   [0..8)   GcHeader
///   [8..16)  shape: *const Shape
///   [16..20) capacity: u32 (total slot capacity including reserved)
///   [20..24) slot_count: u32 (used slots)
///   [24..32) prototype: *mut u8 (heap pointer or null)
///   [32..40) slots: *mut SlotBuffer (stable across growth)
///
/// Slot values live in an internal TAG_ARRAY buffer with values at offset 40;
/// object growth replaces the buffer, never the object header.
pub const OBJECT_HEADER_END: usize = 40;

/// Number of extra slots to reserve beyond the initial shape's property count.
const RESERVED_SLOTS: usize = 8;

/// Initial objects keep slots inline at offset 40. Growth converts them to a
/// stable slot buffer and clears this bit; the object header never moves.
pub const INLINE_SLOTS_BIT: u32 = 1 << 30;

/// Object extensibility (§10.1.9 [[Extensible]]): stored as the top bit of
/// the capacity word (capacities never approach 2^31). Set by
/// preventExtensions/seal/freeze (A4); consulted by property definition and
/// ordinary [[Set]] on new keys. Zero layout change; GC masks it (gc.rs).
pub const EXTENSIBLE_BIT: u32 = 1 << 31;

/// A GC-allocated JavaScript object.
pub struct JSObject;

impl JSObject {
    pub fn allocate(
        ss: &mut SemiSpace,
        shape: &'static Shape,
        slot_values: &[Value],
    ) -> *mut JSObject {
        let slot_count = slot_values.len();
        let capacity = slot_count + RESERVED_SLOTS;
        let scratch_base = ss.scratch_len();
        for value in slot_values {
            ss.push_scratch_root(*value);
        }
        let ptr = ss.alloc(OBJECT_HEADER_END + capacity * size_of::<Value>());
        unsafe {
            let header = &mut *(ptr as *mut GcHeader);
            header.word = std::sync::atomic::AtomicU64::new(TAG_OBJECT);
            *(ptr.add(8) as *mut *const Shape) = shape as *const Shape;
            *(ptr.add(16) as *mut u32) = (capacity as u32) | INLINE_SLOTS_BIT;
            *(ptr.add(20) as *mut u32) = slot_count as u32;
            *(ptr.add(24) as *mut *mut u8) = std::ptr::null_mut();
            // Inline objects keep a self pointer at 32 so JIT slot access can
            // use one uniform [ptr] then [ptr + 40 + offset*8] sequence.
            *(ptr.add(32) as *mut *mut u8) = ptr;
            let slots = ptr.add(OBJECT_HEADER_END) as *mut Value;
            for i in 0..slot_count {
                *slots.add(i) = ss.scratch_value(scratch_base + i);
            }
            for i in slot_count..capacity {
                *slots.add(i) = Value::undefined();
            }
        }
        ss.truncate_scratch(scratch_base);
        ptr as *mut JSObject
    }

    pub unsafe fn capacity(ptr: *mut JSObject) -> usize {
        unsafe {
            let ptr = ptr as *mut u8;
            let cap_ptr = ptr.add(16) as *const u32;
            // Mask extensibility/inline flags.
            (*cap_ptr & !(EXTENSIBLE_BIT | INLINE_SLOTS_BIT)) as usize
        }
    }

    /// Object extensibility (§10.1.9). Fresh objects are extensible.
    pub unsafe fn is_extensible(ptr: *mut JSObject) -> bool {
        unsafe {
            let ptr = ptr as *mut u8;
            let cap_ptr = ptr.add(16) as *const u32;
            (*cap_ptr & EXTENSIBLE_BIT) == 0
        }
    }

    /// Set object extensibility (A4: preventExtensions/seal/freeze clear it).
    pub unsafe fn set_extensible(ptr: *mut JSObject, extensible: bool) {
        unsafe {
            let ptr = ptr as *mut u8;
            let cap_ptr = ptr.add(16) as *mut u32;
            let cap = *cap_ptr;
            *cap_ptr = if extensible {
                cap & !EXTENSIBLE_BIT
            } else {
                cap | EXTENSIBLE_BIT
            };
        }
    }

    pub unsafe fn slot_count(ptr: *mut JSObject) -> usize {
        unsafe {
            let ptr = ptr as *mut u8;
            let sc_ptr = ptr.add(20) as *const u32;
            *sc_ptr as usize
        }
    }

    pub unsafe fn set_slot_count(ptr: *mut JSObject, n: usize) {
        unsafe {
            let ptr = ptr as *mut u8;
            let sc_ptr = ptr.add(20) as *mut u32;
            *sc_ptr = n as u32;
        }
    }

    pub unsafe fn set_shape_ptr(ptr: *mut JSObject, shape: &'static Shape) {
        unsafe {
            let ptr = ptr as *mut u8;
            let shape_ptr_ptr = ptr.add(8) as *mut *const Shape;
            *shape_ptr_ptr = shape as *const Shape;
        }
    }

    /// Grow to ~2x capacity. Inline slots convert to a stable slot buffer;
    /// the object header never moves. Returns (header, same_header).
    pub unsafe fn grow(ss: &mut SemiSpace, ptr: *mut JSObject) -> (*mut u8, *mut JSObject) {
        unsafe {
            let obj = crate::gc::resolve_forwarded(ptr as *mut u8) as *mut JSObject;
            let old_cap = Self::capacity(obj);
            let count = Self::slot_count(obj);
            let old_word = *((obj as *mut u8).add(16) as *const u32);
            let inline = old_word & INLINE_SLOTS_BIT != 0;
            let new_cap = (old_cap * 2).max(old_cap + 8);
            let scratch_base = ss.scratch_len();
            ss.push_scratch_root(Value::from_heap_ptr(obj as *mut u8));
            let old_buffer = if inline {
                std::ptr::null_mut()
            } else {
                *((obj as *mut u8).add(32) as *const *mut u8)
            };
            let new_buffer = crate::array::RuneArray::slot_buffer_alloc(ss, new_cap);
            let obj_now = ss
                .scratch_value(scratch_base)
                .heap_ptr()
                .unwrap_or(std::ptr::null_mut()) as *mut JSObject;
            let new_slots = new_buffer.add(crate::array::BUFFER_HEAD_END) as *mut Value;
            let copied = count.min(old_cap);
            if inline {
                let old_slots = (obj_now as *mut u8).add(OBJECT_HEADER_END) as *const Value;
                std::ptr::copy_nonoverlapping(old_slots, new_slots, copied);
            } else if !old_buffer.is_null() {
                let old_buffer = crate::gc::resolve_forwarded(old_buffer);
                let old_slots = old_buffer.add(crate::array::BUFFER_HEAD_END) as *const Value;
                std::ptr::copy_nonoverlapping(old_slots, new_slots, copied);
            }
            for i in copied..new_cap {
                *new_slots.add(i) = Value::undefined();
            }
            *((obj_now as *mut u8).add(16) as *mut u32) =
                (new_cap as u32) | (old_word & EXTENSIBLE_BIT);
            *((obj_now as *mut u8).add(32) as *mut *mut u8) = new_buffer;
            ss.truncate_scratch(scratch_base);
            (obj_now as *mut u8, obj_now)
        }
    }

    pub unsafe fn prototype(ptr: *mut JSObject) -> *mut u8 {
        unsafe {
            let ptr_bytes = ptr as *mut u8;
            let proto_ptr = ptr_bytes.add(24) as *const *mut u8;
            crate::gc::resolve_forwarded(*proto_ptr)
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
            let word = *(ptr_bytes.add(16) as *const u32);
            if word & INLINE_SLOTS_BIT != 0 {
                return ptr_bytes.add(OBJECT_HEADER_END) as *mut Value;
            }
            let raw = *(ptr_bytes.add(32) as *const *mut u8);
            if raw.is_null() {
                return std::ptr::null_mut();
            }
            crate::gc::resolve_forwarded(raw).add(crate::array::BUFFER_HEAD_END) as *mut Value
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
                let mut new_entries: Vec<(crate::shape::PropertyKey, usize)> =
                    Vec::with_capacity(count - 1);
                let mut new_key_names: Vec<String> = Vec::with_capacity(count - 1);
                // A4: sibling attributes survive the removal (parallel vec).
                let mut new_attrs: Vec<crate::shape::PropAttr> = Vec::with_capacity(count - 1);
                for i in 0..count {
                    if i == slot {
                        continue;
                    }
                    let new_offset = new_entries.len();
                    new_entries.push((shape.entries[i].0, new_offset));
                    new_key_names.push(shape.key_names[i].clone());
                    new_attrs.push(shape.attr_at(i));
                }
                let new_shape =
                    crate::shape::Shape::intern_with_attrs(new_entries, new_key_names, new_attrs);
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
    /// Panics if the object has no reserved capacity left (callers that grow
    /// beyond reserve must JSObject::grow first — see do_store_property and
    /// define_own_property).
    pub unsafe fn add_property(
        ptr: *mut JSObject,
        key: crate::shape::PropertyKey,
        key_name: String,
        val: Value,
    ) -> usize {
        unsafe {
            Self::add_property_with_attrs(ptr, key, key_name, val, crate::shape::ATTR_DEFAULT)
        }
    }

    /// Add a property with explicit attributes (A4: defineProperty with
    /// non-default descriptors transitions to a non-default shape).
    pub unsafe fn add_property_with_attrs(
        ptr: *mut JSObject,
        key: crate::shape::PropertyKey,
        key_name: String,
        val: Value,
        attr: crate::shape::PropAttr,
    ) -> usize {
        unsafe {
            let cap = Self::capacity(ptr);
            let count = Self::slot_count(ptr);
            assert!(count < cap, "JSObject: out of reserved slot capacity");
            let shape = Self::shape_ptr(ptr);
            let new_shape =
                crate::shape::Shape::intern_with_parent_attrs(shape, key, key_name, attr);
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
