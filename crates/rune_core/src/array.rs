use crate::gc::{GcHeader, SemiSpace, TAG_ARRAY, size_of};
use crate::value::Value;

/// Memory layout of a stable array header:
///   [0..8)   GcHeader with TAG_ARRAY
///   [8..16)  shape: *const Shape (DENSE_ARRAY_SHAPE for all arrays)
///   [16..20) length: u32 (number of elements)
///   [20..24) capacity: u32 (allocated element capacity + flags)
///   [24..32) prototype: *mut u8 (Array.prototype, set by VM)
///   [32..40) extra_props: *mut u8 (JSObject with named properties)
///   [40..48) elements: *mut SlotBuffer (never changes on growth)
///
/// Element storage lives in a separate TAG_ARRAY slot buffer:
///   [0..8) header, [8..16) unused, [16..20) capacity,
///   [20..24) capacity|ARRAY_BUFFER_BIT, [24..32) unused, [32..40) unused,
///   [40..) Value[]
///
/// The header address is stable across growth, so raw aliases held by roots,
/// other arrays, objects, and pending machines never need a growth rewrite.
/// The GC still moves headers; aliases in containers are updated as usual.
pub struct RuneArray;

/// Byte offset of the extra_props pointer (JSObject or null).
pub const EXTRA_PROPS_OFFSET: usize = 32;
/// Byte offset of the slot-buffer pointer in a normal array header.
pub const ELEMENTS_OFFSET: usize = 40;
/// Total normal-array header size (the elements pointer ends at 48).
pub const ARRAY_HEADER_END: usize = 48;
/// Byte offset of inline values in an internal slot buffer.
pub const BUFFER_HEAD_END: usize = 40;

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
/// Marks an internal slot buffer (not a user-visible array). It has a 40-byte
/// header and inline values.
pub const ARRAY_BUFFER_BIT: u32 = 1 << 27;
/// Initial arrays keep their values inline at offset 48. Growth converts them
/// to a stable slot buffer and clears this bit; the header never moves.
pub const ARRAY_INLINE_BIT: u32 = 1 << 26;
/// Mask of all flag bits in the capacity word (GC + capacity()).
pub const ARRAY_FLAG_MASK: u32 = ARRAY_NONEXTENSIBLE_BIT
    | ARRAY_LENGTH_NONWRITABLE_BIT
    | ARRAY_ELEMS_NONCONFIGURABLE_BIT
    | ARRAY_ELEMS_NONWRITABLE_BIT
    | ARRAY_BUFFER_BIT
    | ARRAY_INLINE_BIT;

/// Number of extra element slots to reserve beyond initial length.
const RESERVED_ELEMENTS: usize = 4;

impl RuneArray {
    /// Resolve a possibly-forwarded array pointer to the current header.
    #[inline]
    unsafe fn live(arr: *mut RuneArray) -> *mut RuneArray {
        unsafe { crate::gc::resolve_forwarded(arr as *mut u8) as *mut RuneArray }
    }
    /// Allocate an internal slot buffer with `cap` inline Value slots.
    unsafe fn slot_buffer_alloc(ss: &mut SemiSpace, cap: usize) -> *mut u8 {
        unsafe {
            let ptr = ss.alloc(BUFFER_HEAD_END + cap * size_of::<Value>());
            let header = &mut *(ptr as *mut GcHeader);
            header.word = std::sync::atomic::AtomicU64::new(TAG_ARRAY);
            *(ptr.add(8) as *mut *const u8) = std::ptr::null_mut();
            *(ptr.add(16) as *mut u32) = cap as u32;
            *(ptr.add(20) as *mut u32) = (cap as u32) | ARRAY_BUFFER_BIT;
            *(ptr.add(24) as *mut *mut u8) = std::ptr::null_mut();
            *(ptr.add(32) as *mut *mut u8) = std::ptr::null_mut();
            let values = ptr.add(BUFFER_HEAD_END) as *mut Value;
            for i in 0..cap {
                *values.add(i) = Value::empty_sentinel();
            }
            ptr
        }
    }

    /// Current slot buffer (resolves GC forwarding).
    unsafe fn elements_ptr(arr: *mut RuneArray) -> *mut Value {
        let arr = unsafe { Self::live(arr) };
        let word = unsafe { *((arr as *mut u8).add(20) as *const u32) };
        if word & ARRAY_INLINE_BIT != 0 {
            return unsafe { (arr as *mut u8).add(ARRAY_HEADER_END) as *mut Value };
        }
        let raw = unsafe { *((arr as *mut u8).add(ELEMENTS_OFFSET) as *const *mut u8) };
        if raw.is_null() {
            return std::ptr::null_mut();
        }
        unsafe { crate::gc::resolve_forwarded(raw).add(BUFFER_HEAD_END) as *mut Value }
    }

    /// Allocate a dense array with the given elements. Initial values are
    /// inline; `grow` converts to a stable slot buffer without moving the
    /// header.
    pub fn allocate(ss: &mut SemiSpace, elements: &[Value]) -> *mut RuneArray {
        let len = elements.len();
        let cap = len + RESERVED_ELEMENTS;
        let scratch_base = ss.scratch_len();
        // The allocation may collect and move heap Values in `elements`;
        // root them and copy the refreshed values after the allocation.
        for value in elements {
            ss.push_scratch_root(*value);
        }
        let ptr = ss.alloc(ARRAY_HEADER_END + cap * size_of::<Value>());
        unsafe {
            let header = &mut *(ptr as *mut GcHeader);
            header.word = std::sync::atomic::AtomicU64::new(TAG_ARRAY);
            *(ptr.add(8) as *mut *const u8) = std::ptr::null_mut();
            *(ptr.add(16) as *mut u32) = len as u32;
            *(ptr.add(20) as *mut u32) = (cap as u32) | ARRAY_INLINE_BIT;
            *(ptr.add(24) as *mut *mut u8) = std::ptr::null_mut();
            *(ptr.add(EXTRA_PROPS_OFFSET) as *mut *mut u8) = std::ptr::null_mut();
            *(ptr.add(ELEMENTS_OFFSET) as *mut *mut u8) = std::ptr::null_mut();
            let values = ptr.add(ARRAY_HEADER_END) as *mut Value;
            for i in 0..len {
                *values.add(i) = ss.scratch_value(scratch_base + i);
            }
            for i in len..cap {
                *values.add(i) = Value::empty_sentinel();
            }
        }
        ss.truncate_scratch(scratch_base);
        ptr as *mut RuneArray
    }

    pub unsafe fn length(arr: *mut RuneArray) -> u32 {
        let arr = unsafe { Self::live(arr) };
        unsafe { *((arr as *mut u8).add(16) as *const u32) }
    }

    pub unsafe fn set_length(arr: *mut RuneArray, n: u32) {
        let arr = unsafe { Self::live(arr) };
        unsafe {
            let old = Self::length(arr);
            // Shrinking punches holes (a later re-grow must not resurrect
            // stale values). Bounded by capacity — never writes OOB.
            if n < old {
                let cap = Self::capacity(arr);
                let upto = (old as usize).min(cap as usize);
                let elems_ptr = Self::elements_ptr(arr);
                if !elems_ptr.is_null() {
                    for i in n as usize..upto {
                        *elems_ptr.add(i) = Value::empty_sentinel();
                    }
                }
            }
            *((arr as *mut u8).add(16) as *mut u32) = n;
        }
    }

    pub unsafe fn capacity(arr: *mut RuneArray) -> u32 {
        let arr = unsafe { Self::live(arr) };
        // Mask the integrity flag bits (see above).
        unsafe { (*((arr as *mut u8).add(20) as *const u32)) & !ARRAY_FLAG_MASK }
    }

    /// Raw capacity word (flags included) — GC sizing and flag access only.
    pub unsafe fn capacity_word(arr: *mut RuneArray) -> u32 {
        let arr = unsafe { Self::live(arr) };
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
        let arr = unsafe { Self::live(arr) };
        unsafe {
            // Beyond-capacity reads are holes (a length-extended array has
            // length > capacity; its tail is unallocated). Never read OOB.
            if index >= Self::capacity(arr) as usize {
                return Value::empty_sentinel();
            }
            let elems_ptr = Self::elements_ptr(arr);
            if elems_ptr.is_null() {
                return Value::empty_sentinel();
            }
            *elems_ptr.add(index)
        }
    }

    pub unsafe fn set_element(arr: *mut RuneArray, index: usize, val: Value) {
        let arr = unsafe { Self::live(arr) };
        unsafe {
            let elems_ptr = Self::elements_ptr(arr);
            if !elems_ptr.is_null() {
                *elems_ptr.add(index) = val;
            }
        }
    }

    pub unsafe fn extra_props(arr: *mut RuneArray) -> *mut u8 {
        let arr = unsafe { Self::live(arr) };
        unsafe {
            crate::gc::resolve_forwarded(
                *((arr as *mut u8).add(EXTRA_PROPS_OFFSET) as *const *mut u8),
            )
        }
    }

    pub unsafe fn set_extra_props(arr: *mut RuneArray, props: *mut u8) {
        let arr = unsafe { Self::live(arr) };
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
        let arr = unsafe { Self::live(arr) };
        unsafe { crate::gc::resolve_forwarded(*((arr as *mut u8).add(24) as *const *mut u8)) }
    }

    pub unsafe fn set_prototype(arr: *mut RuneArray, proto: *mut u8) {
        let arr = unsafe { Self::live(arr) };
        unsafe {
            *((arr as *mut u8).add(24) as *mut *mut u8) = proto;
        }
    }

    /// Grow to ~1.5x capacity. Inline values are moved into a stable slot
    /// buffer; the array header does NOT move on growth.
    pub unsafe fn grow(ss: &mut SemiSpace, arr: *mut RuneArray) -> (*mut u8, *mut RuneArray) {
        unsafe {
            let arr = crate::gc::resolve_forwarded(arr as *mut u8) as *mut RuneArray;
            let old_len = Self::length(arr) as usize;
            let old_cap = Self::capacity(arr) as usize;
            let new_cap = (old_cap * 3 / 2).max(old_cap + 8);
            let scratch_base = ss.scratch_len();
            ss.push_scratch_root(Value::from_heap_ptr(arr as *mut u8));
            let old_inline = (*((arr as *mut u8).add(20) as *const u32)) & ARRAY_INLINE_BIT != 0;
            let old_buffer = if old_inline {
                std::ptr::null_mut()
            } else {
                Self::elements_ptr(arr)
            };
            let new_buffer = Self::slot_buffer_alloc(ss, new_cap);
            let arr_now = ss
                .scratch_value(scratch_base)
                .heap_ptr()
                .unwrap_or(std::ptr::null_mut()) as *mut RuneArray;
            let copied = old_len.min(old_cap);
            if old_inline {
                let old_values = (arr_now as *mut u8).add(ARRAY_HEADER_END) as *const Value;
                std::ptr::copy_nonoverlapping(
                    old_values,
                    new_buffer.add(BUFFER_HEAD_END) as *mut Value,
                    copied,
                );
            } else if !old_buffer.is_null() {
                let old_buffer =
                    crate::gc::resolve_forwarded(old_buffer as *mut u8) as *const Value;
                std::ptr::copy_nonoverlapping(
                    old_buffer,
                    new_buffer.add(BUFFER_HEAD_END) as *mut Value,
                    copied,
                );
            }
            let flags = Self::capacity_word(arr_now) & ARRAY_FLAG_MASK & !ARRAY_INLINE_BIT;
            *((arr_now as *mut u8).add(20) as *mut u32) = (new_cap as u32) | flags;
            *((arr_now as *mut u8).add(ELEMENTS_OFFSET) as *mut *mut u8) = new_buffer;
            ss.truncate_scratch(scratch_base);
            (arr_now as *mut u8, arr_now)
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
