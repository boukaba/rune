use crate::gc::{GcHeader, SemiSpace, TAG_ARRAY, size_of};
use crate::value::Value;
use std::ptr;

/// Memory layout:
///   [0..8)   GcHeader with TAG_ARRAY
///   [8..16)  shape: *const Shape (DENSE_ARRAY_SHAPE for all arrays)
///   [16..20) length: u32 (number of elements)
///   [20..24) capacity: u32 (allocated element capacity)
///   [24..32) prototype: *mut u8 (Array.prototype, set by VM)
///   [32..40) extra_props: *mut u8 (JSObject with named properties such as
///            the non-enumerable "index"/"input" on match-result arrays)
///   [40..)   elements: Value[]
///
/// Reuses OBJECT_PROTOTYPE_OFFSET (24) from object.rs
pub struct RuneArray;

/// Byte offset of the extra_props pointer (JSObject or null).
pub const EXTRA_PROPS_OFFSET: usize = 32;
/// Byte offset of the first element slot (header end).
pub const ARRAY_HEADER_END: usize = 40;

/// Array integrity flags in the capacity word (B1f-5; A4 object precedent —
/// bit set = restricted). Capacities never approach 2^28 in practice (1M
/// sparse threshold; huge lengths stay length-extended with small caps), and
/// `capacity()` masks them out everywhere (get_element guard, grow math).
pub const ARRAY_NONEXTENSIBLE_BIT: u32 = 1 << 31;
/// Set when `length` is non-writable (freeze, or defineProperty length
/// writable:false). Fresh arrays are length-writable.
pub const ARRAY_LENGTH_NONWRITABLE_BIT: u32 = 1 << 30;
/// Set when elements are non-configurable (seal/freeze whole-array lock).
pub const ARRAY_ELEMS_NONCONFIGURABLE_BIT: u32 = 1 << 29;
/// Set when elements are non-writable (freeze whole-array lock; seal keeps
/// writability).
pub const ARRAY_ELEMS_NONWRITABLE_BIT: u32 = 1 << 28;
/// Mask of all integrity flag bits in the capacity word (GC + capacity()).
pub const ARRAY_FLAG_MASK: u32 = ARRAY_NONEXTENSIBLE_BIT
    | ARRAY_LENGTH_NONWRITABLE_BIT
    | ARRAY_ELEMS_NONCONFIGURABLE_BIT
    | ARRAY_ELEMS_NONWRITABLE_BIT;

/// Number of extra element slots to reserve beyond initial length.
const RESERVED_ELEMENTS: usize = 4;

impl RuneArray {
    /// Allocate a dense array with the given elements.
    pub fn allocate(ss: &mut SemiSpace, elements: &[Value]) -> *mut RuneArray {
        let len = elements.len();
        let cap = len + RESERVED_ELEMENTS;
        let total_size = ARRAY_HEADER_END + cap * size_of::<Value>();
        let ptr = ss.alloc(total_size);
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

            // extra_props starts as null
            let extra_ptr = ptr.add(EXTRA_PROPS_OFFSET) as *mut *mut u8;
            *extra_ptr = std::ptr::null_mut();

            let elems_ptr = ptr.add(ARRAY_HEADER_END) as *mut Value;
            ptr::copy_nonoverlapping(elements.as_ptr(), elems_ptr, len);
            // Reserved slots are holes (empty sentinel), never undefined:
            // length-extension exposes them and they must read as absent
            // (B1f — an undefined fill here shadowed the prototype for
            // length-grown tails).
            for i in len..cap {
                *elems_ptr.add(i) = Value::empty_sentinel();
            }
        }
        ptr as *mut RuneArray
    }

    pub unsafe fn length(arr: *mut RuneArray) -> u32 {
        unsafe { *((arr as *mut u8).add(16) as *const u32) }
    }

    pub unsafe fn set_length(arr: *mut RuneArray, n: u32) {
        unsafe {
            let old = Self::length(arr);
            // Shrinking punches holes (a later re-grow must not resurrect
            // stale values). Bounded by capacity — never writes OOB.
            if n < old {
                let cap = Self::capacity(arr);
                let upto = (old as usize).min(cap as usize);
                let elems_ptr = (arr as *mut u8).add(ARRAY_HEADER_END) as *mut Value;
                for i in n as usize..upto {
                    *elems_ptr.add(i) = Value::empty_sentinel();
                }
            }
            *((arr as *mut u8).add(16) as *mut u32) = n;
        }
    }

    pub unsafe fn capacity(arr: *mut RuneArray) -> u32 {
        // Mask the integrity flag bits (see above).
        unsafe { (*((arr as *mut u8).add(20) as *const u32)) & !ARRAY_FLAG_MASK }
    }

    /// Raw capacity word (flags included) — GC sizing and flag access only.
    pub unsafe fn capacity_word(arr: *mut RuneArray) -> u32 {
        unsafe { *((arr as *mut u8).add(20) as *const u32) }
    }

    /// Array extensibility (§10.1.9). Fresh arrays are extensible.
    pub unsafe fn is_extensible(arr: *mut RuneArray) -> bool {
        unsafe { Self::capacity_word(arr) & ARRAY_NONEXTENSIBLE_BIT == 0 }
    }

    /// Set array extensibility (preventExtensions/seal/freeze clear it).
    pub unsafe fn set_extensible(arr: *mut RuneArray, extensible: bool) {
        unsafe {
            let w = Self::capacity_word(arr);
            *((arr as *mut u8).add(20) as *mut u32) = if extensible {
                w & !ARRAY_NONEXTENSIBLE_BIT
            } else {
                w | ARRAY_NONEXTENSIBLE_BIT
            };
        }
    }

    /// `length` writability (§23.1.4.1). Fresh arrays are writable.
    pub unsafe fn length_is_writable(arr: *mut RuneArray) -> bool {
        unsafe { Self::capacity_word(arr) & ARRAY_LENGTH_NONWRITABLE_BIT == 0 }
    }

    /// Set `length` writability (freeze / defineProperty length writable:false
    /// clear it; nothing re-enables it — non-writable is sticky per spec).
    pub unsafe fn set_length_writable(arr: *mut RuneArray, writable: bool) {
        unsafe {
            let w = Self::capacity_word(arr);
            *((arr as *mut u8).add(20) as *mut u32) = if writable {
                w & !ARRAY_LENGTH_NONWRITABLE_BIT
            } else {
                w | ARRAY_LENGTH_NONWRITABLE_BIT
            };
        }
    }

    /// Whole-array element configurability (seal/freeze lock). Per-index
    /// overlay pairs keep their own shape attrs; the bit overrides them on
    /// reads (bits authoritative — no shape rewrite on seal/freeze).
    pub unsafe fn elems_are_nonconfigurable(arr: *mut RuneArray) -> bool {
        unsafe { Self::capacity_word(arr) & ARRAY_ELEMS_NONCONFIGURABLE_BIT != 0 }
    }

    /// Whole-array element writability (freeze lock; seal keeps writability).
    pub unsafe fn elems_are_nonwritable(arr: *mut RuneArray) -> bool {
        unsafe { Self::capacity_word(arr) & ARRAY_ELEMS_NONWRITABLE_BIT != 0 }
    }

    /// Apply an integrity level (B1f-5): seal locks configurability,
    /// freeze additionally locks writability (elements + length).
    pub unsafe fn seal_elements(arr: *mut RuneArray, freeze_writes: bool) {
        unsafe {
            let w = Self::capacity_word(arr);
            let mut nw = w | ARRAY_NONEXTENSIBLE_BIT | ARRAY_ELEMS_NONCONFIGURABLE_BIT;
            if freeze_writes {
                nw |= ARRAY_ELEMS_NONWRITABLE_BIT | ARRAY_LENGTH_NONWRITABLE_BIT;
            }
            *((arr as *mut u8).add(20) as *mut u32) = nw;
        }
    }

    pub unsafe fn get_element(arr: *mut RuneArray, index: usize) -> Value {
        unsafe {
            // Beyond-capacity reads are holes (a length-extended array has
            // length > capacity; its tail is unallocated). Never read OOB.
            if index >= Self::capacity(arr) as usize {
                return Value::empty_sentinel();
            }
            let elems_ptr = (arr as *mut u8).add(ARRAY_HEADER_END) as *const Value;
            *elems_ptr.add(index)
        }
    }

    pub unsafe fn set_element(arr: *mut RuneArray, index: usize, val: Value) {
        unsafe {
            let elems_ptr = (arr as *mut u8).add(ARRAY_HEADER_END) as *mut Value;
            *elems_ptr.add(index) = val;
        }
    }

    pub unsafe fn extra_props(arr: *mut RuneArray) -> *mut u8 {
        unsafe { *((arr as *mut u8).add(EXTRA_PROPS_OFFSET) as *const *mut u8) }
    }

    pub unsafe fn set_extra_props(arr: *mut RuneArray, props: *mut u8) {
        unsafe {
            *((arr as *mut u8).add(EXTRA_PROPS_OFFSET) as *mut *mut u8) = props;
        }
    }

    pub unsafe fn shape_ptr(arr: *mut RuneArray) -> *const crate::shape::Shape {
        unsafe { *((arr as *mut u8).add(8) as *const *const crate::shape::Shape) }
    }

    pub unsafe fn set_shape_ptr(arr: *mut RuneArray, shape: *const crate::shape::Shape) {
        unsafe {
            *((arr as *mut u8).add(8) as *mut *const crate::shape::Shape) = shape;
        }
    }

    pub unsafe fn prototype(arr: *mut RuneArray) -> *mut u8 {
        unsafe { *((arr as *mut u8).add(24) as *const *mut u8) }
    }

    pub unsafe fn set_prototype(arr: *mut RuneArray, proto: *mut u8) {
        unsafe {
            *((arr as *mut u8).add(24) as *mut *mut u8) = proto;
        }
    }

    /// Grow the array to ~1.5x capacity, copying all elements and header.
    /// Returns (resolved_old_ptr, new_array).
    /// `resolved_old_ptr` is the pointer that roots should be updated from
    /// (after any GC forwarding during the allocation).
    pub unsafe fn grow(ss: &mut SemiSpace, arr: *mut RuneArray) -> (*mut u8, *mut RuneArray) {
        unsafe {
            let old_len = Self::length(arr) as usize;
            let old_cap = Self::capacity(arr) as usize;
            let new_cap = (old_cap * 3 / 2).max(old_cap + 8);
            let total_size = ARRAY_HEADER_END + new_cap * size_of::<Value>();
            let new_ptr = ss.alloc(total_size);
            // If GC ran during alloc, `arr` may be a stale from-space pointer
            // with a forwarded header. Resolve to the to-space copy.
            let src = if (*(arr as *const GcHeader)).is_forwarded() {
                (*(arr as *const GcHeader)).forwarding_addr()
            } else {
                arr as *mut u8
            };
            // Copy header (GcHeader + shape + length + capacity + prototype +
            // extra_props) = 40 bytes
            std::ptr::copy_nonoverlapping(src, new_ptr, ARRAY_HEADER_END);
            // Update capacity in new header, preserving integrity flags
            // (B1f-5; A4 object precedent — seal/freeze survive growth).
            // Flags read from the resolved source (arr may be stale).
            *(new_ptr.add(20) as *mut u32) =
                (new_cap as u32) | (Self::capacity_word(src as *mut RuneArray) & ARRAY_FLAG_MASK);
            // Copy elements (only the allocated window is readable;
            // a length-extended array may have length > capacity).
            // Everything past the copied window is a hole (empty sentinel),
            // never undefined: reserved slots are unread and extended
            // windows must read as holes.
            let copied = old_len.min(old_cap);
            let old_elems = src.add(ARRAY_HEADER_END) as *const Value;
            let new_elems = new_ptr.add(ARRAY_HEADER_END) as *mut Value;
            std::ptr::copy_nonoverlapping(old_elems, new_elems, copied);
            for i in copied..new_cap {
                *new_elems.add(i) = Value::empty_sentinel();
            }
            (src, new_ptr as *mut RuneArray)
        }
    }

    /// Push a value to the end of the array.
    /// Auto-grows if capacity is exhausted.
    /// Returns the (possibly new) array pointer.
    pub unsafe fn push(ss: &mut SemiSpace, arr: *mut RuneArray, val: Value) -> *mut RuneArray {
        unsafe {
            let len = Self::length(arr);
            let cap = Self::capacity(arr);
            let (_, current) = if (len as usize) >= cap as usize {
                Self::grow(ss, arr)
            } else {
                (arr as *mut u8, arr)
            };
            Self::set_element(current, len as usize, val);
            Self::set_length(current, len + 1);
            current
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
