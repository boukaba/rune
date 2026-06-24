use std::fmt;

use crate::gc::{GcHeader, TAG_STRING};
use crate::string::HeapString;

/// 64-bit tagged value (V8-style).
///
/// Tag scheme (lowest bit):
///   - bit 0 = 1: Smi (value = n << 1 | 1; decode = (raw >> 1) as i32)
///   - bit 0 = 0: heap pointer or sentinel
///     - raw == 0x00: `undefined`
///     - raw == 0x02: `null`
///     - raw == 0x04: `false`
///     - raw == 0x06: `true`
///     - else: heap pointer (8-byte aligned, lowest 3 bits = 0)
///
/// Smi range: -(2^30) .. (2^30 - 1)
#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct Value(u64);

const SMI_TAG: u64 = 0x01;
const TAG_MASK: u64 = 0x01;
const UNDEFINED_RAW: u64 = 0x00;
const NULL_RAW: u64 = 0x02;
const FALSE_RAW: u64 = 0x04;
const TRUE_RAW: u64 = 0x06;

impl Value {
    /// Create a Smi value (small integer).
    /// Debug-asserts value is within i31 range.
    pub fn smi(value: i32) -> Self {
        debug_assert!(
            (-(1 << 30)..(1 << 30)).contains(&value),
            "Smi value out of range: {value}"
        );
        let raw = ((value as i64) << 1) | SMI_TAG as i64;
        Value(raw as u64)
    }

    pub fn is_smi(&self) -> bool {
        self.0 & TAG_MASK == SMI_TAG
    }

    pub fn as_smi(&self) -> Option<i32> {
        if self.is_smi() {
            let raw = self.0 as i64;
            Some((raw >> 1) as i32)
        } else {
            None
        }
    }

    /// Check if this is a heap pointer (bit 0 = 0, non-zero, not a sentinel).
    pub fn is_heap_object(&self) -> bool {
        self.0 & TAG_MASK == 0 && !self.is_sentinel()
    }

    fn is_sentinel(&self) -> bool {
        self.0 <= 6 && self.0 & 1 == 0
    }

    /// Get the raw heap address, if this is a heap object.
    pub fn heap_ptr(&self) -> Option<*mut u8> {
        if self.is_heap_object() {
            Some(self.0 as *mut u8)
        } else {
            None
        }
    }

    /// Create a Value from a heap object pointer.
    /// `ptr` must be at least 2-byte aligned (bit 0 = 0).
    pub fn from_heap_ptr(ptr: *mut u8) -> Self {
        debug_assert!(ptr as usize & 1 == 0, "misaligned heap pointer");
        Value(ptr as u64)
    }

    /// Reconstruct a Value from its raw u64 representation (e.g. from JIT output).
    pub fn from_raw(raw: u64) -> Self {
        Value(raw)
    }

    pub const fn undefined() -> Self {
        Value(UNDEFINED_RAW)
    }

    pub const fn null() -> Self {
        Value(NULL_RAW)
    }

    pub const fn boolean(b: bool) -> Self {
        Value(if b { TRUE_RAW } else { FALSE_RAW })
    }

    pub fn is_boolean(&self) -> bool {
        self.0 == TRUE_RAW || self.0 == FALSE_RAW
    }

    pub fn to_boolean(&self) -> Option<bool> {
        if self.0 == TRUE_RAW {
            Some(true)
        } else if self.0 == FALSE_RAW {
            Some(false)
        } else {
            None
        }
    }

    pub fn is_undefined(&self) -> bool {
        self.0 == UNDEFINED_RAW
    }

    pub fn is_null(&self) -> bool {
        self.0 == NULL_RAW
    }

    /// ECMAScript ToBoolean (§7.1.2).
    pub fn to_bool(&self) -> bool {
        if self.is_undefined() || self.is_null() {
            return false;
        }
        if let Some(b) = self.to_boolean() {
            return b;
        }
        if let Some(v) = self.as_smi() {
            return v != 0;
        }
        if let Some(v) = self.as_float64() {
            return !v.is_nan() && v != 0.0;
        }
        // §7.1.2: String → false if empty, true otherwise
        if let Some(ptr) = self.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_STRING {
                let len = unsafe { HeapString::len(ptr as *mut HeapString) };
                return len > 0;
            }
        }
        // Other heap objects (Object, Array, Function) → true
        true
    }

    /// Check if this is a heap-allocated float64 value.
    pub fn is_float64(&self) -> bool {
        if let Some(ptr) = self.heap_ptr() {
            unsafe { (*(ptr as *const u64)) & 0b111 == 3 }
        } else {
            false
        }
    }

    /// Extract f64 value if this is a heap-allocated float64.
    pub fn as_float64(&self) -> Option<f64> {
        if let Some(ptr) = self.heap_ptr() {
            unsafe {
                let header_word = *(ptr as *const u64);
                if header_word & 0b111 == 3 {
                    let val_ptr = ptr.add(8) as *const f64;
                    return Some(*val_ptr);
                }
            }
        }
        None
    }

    /// Create a Value from a heap-allocated float64 pointer.
    pub fn from_float64_ptr(ptr: *mut u8) -> Self {
        debug_assert!(ptr as usize & 1 == 0, "misaligned float64 pointer");
        Value(ptr as u64)
    }

    pub fn raw(&self) -> u64 {
        self.0
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::smi(v)
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_undefined() {
            write!(f, "undefined")
        } else if self.is_null() {
            write!(f, "null")
        } else if let Some(b) = self.to_boolean() {
            write!(f, "{b}")
        } else if let Some(v) = self.as_smi() {
            write!(f, "{v}")
        } else if let Some(v) = self.as_float64() {
            write!(f, "{v}")
        } else if self.is_heap_object() {
            write!(f, "<object @ {:#x}>", self.0)
        } else {
            write!(f, "<value {:#x}>", self.0)
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for Value {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_smi_roundtrip() {
        let cases = [0, 1, -1, 42, -1073741824, 1073741823];
        for &v in &cases {
            let val = Value::smi(v);
            assert!(val.is_smi(), "should be Smi: {v}");
            assert_eq!(val.as_smi(), Some(v), "roundtrip failed: {v}");
        }
    }

    #[test]
    fn test_smi_range() {
        let max = (1 << 30) - 1;
        let min = -(1 << 30);
        assert!(Value::smi(max).is_smi());
        assert!(Value::smi(min).is_smi());
    }

    #[test]
    fn test_undefined_null() {
        let u = Value::undefined();
        let n = Value::null();
        assert!(u.is_undefined());
        assert!(n.is_null());
        assert!(!u.is_null());
        assert!(!n.is_undefined());
        assert!(!u.to_bool());
        assert!(!n.to_bool());
    }

    #[test]
    fn test_boolean_type() {
        let t = Value::boolean(true);
        let f = Value::boolean(false);
        assert!(t.is_boolean());
        assert!(f.is_boolean());
        assert_eq!(t.to_boolean(), Some(true));
        assert_eq!(f.to_boolean(), Some(false));
        assert!(t.to_bool());
        assert!(!f.to_bool());
        assert!(!t.is_heap_object());
        assert!(!f.is_heap_object());
        assert!(!t.is_smi());
        assert!(!f.is_smi());
        assert!(!t.is_undefined());
        assert!(!f.is_null());
    }

    #[test]
    fn test_boolean_conversion() {
        assert!(!Value::undefined().to_bool());
        assert!(!Value::null().to_bool());
        assert!(!Value::boolean(false).to_bool());
        assert!(Value::boolean(true).to_bool());
        assert!(!Value::smi(0).to_bool());
        assert!(Value::smi(1).to_bool());
        assert!(Value::smi(-1).to_bool());
    }

    #[test]
    fn test_heap_ptr() {
        let mut x = 42u64;
        let ptr = &mut x as *mut u64 as *mut u8;
        let val = Value::from_heap_ptr(ptr);
        assert!(val.is_heap_object());
        assert_eq!(val.heap_ptr(), Some(ptr));
    }

    #[test]
    fn test_smi_tag_does_not_overlap() {
        let v = Value::smi(42);
        assert!(!v.is_heap_object());
        assert!(!v.is_undefined());
        assert!(!v.is_null());
        assert!(!v.is_boolean());
    }
}
