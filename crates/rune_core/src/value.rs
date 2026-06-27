use std::fmt;

use crate::gc::{GcHeader, TAG_STRING};
use crate::string::HeapString;

/// 64-bit Float Self-Tagging value (arxiv 2411.16544).
///
/// Float64 values are stored directly as raw IEEE 754 doubles.
/// Non-float values (Smi, heap pointers, sentinels) are encoded
/// as quiet NaN payloads using the 0x7FF8 prefix.
///
/// Encoding:
///   Float64:  raw = f64::to_bits(val)  (any non-NaN or NaN not matching our prefix)
///   Non-float: raw = QNAN_PREFIX | (tag << 45) | payload
///
///   QNAN_PREFIX = 0x7FF8_0000_0000_0000 (quiet NaN, exponent=0x7FF, bit51=1)
///
///   Tags (3 bits at positions 45-47):
///     0 = Smi:         payload = ((i31 as u64) << 1 | 1) & PAYLOAD_MASK  (bit0=1)
///     1 = Heap pointer: payload = (ptr as u64) >> 3
///     2 = Undefined
///     3 = Null
///     4 = False
///     5 = True
///
/// Check is_float64: (raw >> 48) != 0x7FF8
#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct Value(u64);

// NaN prefix for non-float values
const QNAN_PREFIX: u64 = 0x7FF8_0000_0000_0000u64;
const QNAN_TOP: u64 = 0x7FF8; // top 16 bits of QNAN_PREFIX

// Tag constants (3 bits at positions 45-47)
const TAG_SHIFT: u64 = 45;
const TAG_MASK: u64 = 0x7;
const SMI_TAG: u64 = 0;
const HEAP_PTR_TAG: u64 = 1;
const UNDEFINED_TAG: u64 = 2;
const NULL_TAG: u64 = 3;
const FALSE_TAG: u64 = 4;
const TRUE_TAG: u64 = 5;

// Payload mask: bits 0-44 (45 bits)
const PAYLOAD_MASK: u64 = (1 << 45) - 1;

impl Value {
    pub fn is_non_float(&self) -> bool {
        (self.0 >> 48) == QNAN_TOP
    }

    fn tag(&self) -> u64 {
        (self.0 >> TAG_SHIFT) & TAG_MASK
    }

    /// Create a Smi value (small integer).
    /// Debug-asserts value is within i31 range.
    pub fn smi(value: i32) -> Self {
        debug_assert!(
            (-(1 << 30)..(1 << 30)).contains(&value),
            "Smi value out of range: {value}"
        );
        let smi_raw = ((value as i64) << 1) | 1i64;
        Value(QNAN_PREFIX | (smi_raw as u64 & PAYLOAD_MASK))
    }

    pub fn is_smi(&self) -> bool {
        self.is_non_float() && self.tag() == SMI_TAG && (self.0 & 1) == 1
    }

    pub fn as_smi(&self) -> Option<i32> {
        if self.is_smi() {
            let payload = self.0 & PAYLOAD_MASK;
            Some((payload >> 1) as i32)
        } else {
            None
        }
    }

    /// Check if this is a heap pointer.
    pub fn is_heap_object(&self) -> bool {
        self.is_non_float() && self.tag() == HEAP_PTR_TAG
    }

    /// Get the raw heap address, if this is a heap object.
    pub fn heap_ptr(&self) -> Option<*mut u8> {
        if self.is_heap_object() {
            let ptr = ((self.0 & PAYLOAD_MASK) << 3) as *mut u8;
            Some(ptr)
        } else {
            None
        }
    }

    /// Create a Value from a heap object pointer.
    /// `ptr` must be at least 8-byte aligned (low 3 bits = 0).
    pub fn from_heap_ptr(ptr: *mut u8) -> Self {
        debug_assert!(ptr as usize & 7 == 0, "misaligned heap pointer");
        Value(QNAN_PREFIX | (HEAP_PTR_TAG << TAG_SHIFT) | ((ptr as u64) >> 3))
    }

    /// Reconstruct a Value from its raw u64 representation (e.g. from JIT output).
    pub fn from_raw(raw: u64) -> Self {
        Value(raw)
    }

    pub const fn undefined() -> Self {
        Value(QNAN_PREFIX | (UNDEFINED_TAG << TAG_SHIFT))
    }

    pub const fn null() -> Self {
        Value(QNAN_PREFIX | (NULL_TAG << TAG_SHIFT))
    }

    pub const fn boolean(b: bool) -> Self {
        Value(QNAN_PREFIX | ((if b { TRUE_TAG } else { FALSE_TAG }) << TAG_SHIFT))
    }

    pub fn is_boolean(&self) -> bool {
        if self.is_non_float() {
            let t = self.tag();
            t == TRUE_TAG || t == FALSE_TAG
        } else {
            false
        }
    }

    pub fn to_boolean(&self) -> Option<bool> {
        if self.is_non_float() {
            let t = self.tag();
            if t == TRUE_TAG {
                Some(true)
            } else if t == FALSE_TAG {
                Some(false)
            } else {
                None
            }
        } else {
            None
        }
    }

    pub fn is_undefined(&self) -> bool {
        self.is_non_float() && self.tag() == UNDEFINED_TAG
    }

    pub fn is_null(&self) -> bool {
        self.is_non_float() && self.tag() == NULL_TAG
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

    /// Check if this is a float64 value (raw double not in our NaN encoding space).
    pub fn is_float64(&self) -> bool {
        !self.is_non_float()
    }

    /// Extract f64 value if this is a float64 (raw double).
    pub fn as_float64(&self) -> Option<f64> {
        if self.is_float64() {
            Some(f64::from_bits(self.0))
        } else {
            None
        }
    }

    /// Create a float64 Value from an f64.
    pub fn from_float64(val: f64) -> Self {
        let raw = val.to_bits();
        // Collision check: if the raw bits match our non-float QNaN prefix,
        // flip bits 0 and 48 to map to a different QNaN pattern.
        // This changes the NaN payload but preserves NaN semantics (NaN ≠ NaN).
        if (raw >> 48) == QNAN_TOP {
            Value(raw ^ 0x0001_0000_0000_0001)
        } else {
            Value(raw)
        }
    }

    /// Create a Value from a heap-allocated float64 pointer (legacy, for GC compat).
    pub fn from_float64_ptr(ptr: *mut u8) -> Self {
        debug_assert!(ptr as usize & 7 == 0, "misaligned float64 pointer");
        Value(QNAN_PREFIX | (HEAP_PTR_TAG << TAG_SHIFT) | ((ptr as u64) >> 3))
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

    #[test]
    fn test_float64_roundtrip() {
        let cases = [0.0, -0.0, 1.0, -1.0, std::f64::consts::PI, f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
        for &v in &cases {
            let val = Value::from_float64(v);
            assert!(val.is_float64(), "should be float64: {v}");
            let recovered = val.as_float64().unwrap();
            if v.is_nan() {
                assert!(recovered.is_nan(), "NaN preserved: {recovered}");
            } else {
                assert_eq!(recovered, v, "roundtrip failed: {v}");
            }
        }
    }

    #[test]
    fn test_float64_not_non_float() {
        let val = Value::from_float64(std::f64::consts::PI);
        assert!(!val.is_non_float());
        assert!(!val.is_smi());
        assert!(!val.is_undefined());
        assert!(!val.is_null());
        assert!(!val.is_boolean());
        assert!(!val.is_heap_object());
    }

    #[test]
    fn test_non_float_not_float64() {
        assert!(!Value::smi(42).is_float64());
        assert!(!Value::undefined().is_float64());
        assert!(!Value::null().is_float64());
        assert!(!Value::boolean(true).is_float64());
        let mut x = 0u64;
        let ptr = &mut x as *mut u64 as *mut u8;
        assert!(!Value::from_heap_ptr(ptr).is_float64());
    }

    #[test]
    fn test_collision_avoidance() {
        // The QNAN_PREFIX itself as f64::from_bits
        let collision_val = f64::from_bits(QNAN_PREFIX);
        let val = Value::from_float64(collision_val);
        // Must still be detected as float64 (flipped a bit to avoid collision)
        assert!(val.is_float64(), "collision avoidance should keep it as float64");
        let recovered = val.as_float64().unwrap();
        assert!(recovered.is_nan());
    }
}
