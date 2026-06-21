use std::fmt;

/// 64-bit tagged value (V8-style).
///
/// Tag scheme (lowest bit):
///   - bit 0 = 1: Smi (value = n << 1 | 1; decode = (raw >> 1) as i32)
///   - bit 0 = 0: heap pointer or sentinel
///     - raw == 0: `undefined`
///     - raw == 2: `null`
///     - else: heap pointer (aligned, ≥4 bytes)
///
/// Smi range: -(2^30) .. (2^30 - 1)
#[derive(Copy, Clone)]
pub struct Value(u64);

const SMI_TAG: u64 = 0x01;
const TAG_MASK: u64 = 0x01;
const UNDEFINED_RAW: u64 = 0x00;
const NULL_RAW: u64 = 0x02;

impl Value {
    /// Create a Smi value (small integer).
    /// Debug-asserts value is within i31 range.
    pub fn smi(value: i32) -> Self {
        debug_assert!(
            value >= -(1 << 30) && value < (1 << 30),
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

    /// Check if this is a heap pointer (bit 0 = 0, non-zero, not null sentinel).
    pub fn is_heap_object(&self) -> bool {
        self.0 & TAG_MASK == 0 && self.0 != 0 && self.0 != 2
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

    pub const fn undefined() -> Self {
        Value(UNDEFINED_RAW)
    }

    pub const fn null() -> Self {
        Value(NULL_RAW)
    }

    pub fn is_undefined(&self) -> bool {
        self.0 == UNDEFINED_RAW
    }

    pub fn is_null(&self) -> bool {
        self.0 == NULL_RAW
    }

    /// ECMAScript ToBoolean.
    pub fn to_bool(&self) -> bool {
        if self.is_undefined() || self.is_null() {
            return false;
        }
        if let Some(v) = self.as_smi() {
            return v != 0;
        }
        true
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
        } else if let Some(v) = self.as_smi() {
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
    fn test_boolean_conversion() {
        assert!(!Value::undefined().to_bool());
        assert!(!Value::null().to_bool());
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
    }
}
