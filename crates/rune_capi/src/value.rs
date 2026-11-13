//! C-facing value representation and host-callback support.

use rune_core::gc::{GcHeader, SemiSpace, TAG_ARRAY, TAG_FUNC, TAG_OBJECT, TAG_STRING};
use rune_core::string::HeapString;
use rune_core::value::Value;
use std::ffi::{CStr, CString, c_char, c_void};

pub const RUNE_UNDEFINED: u32 = 0;
pub const RUNE_NULL: u32 = 1;
pub const RUNE_BOOLEAN: u32 = 2;
pub const RUNE_NUMBER: u32 = 3;
pub const RUNE_STRING: u32 = 4;
pub const RUNE_OBJECT: u32 = 5;
pub const RUNE_FUNCTION: u32 = 6;

/// Fixed-layout value passed across the C boundary. `string` is borrowed for
/// the duration of a host callback (or copied by the engine from a return
/// value); objects are currently opaque/unsupported.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RuneValue {
    pub kind: u32,
    pub boolean: i32,
    pub number: f64,
    pub string: *const c_char,
    pub handle: u64,
}

impl RuneValue {
    pub fn undefined() -> Self {
        Self {
            kind: RUNE_UNDEFINED,
            boolean: 0,
            number: 0.0,
            string: std::ptr::null(),
            handle: 0,
        }
    }

    pub fn object() -> Self {
        Self {
            kind: RUNE_OBJECT,
            boolean: 0,
            number: 0.0,
            string: std::ptr::null(),
            handle: 0,
        }
    }
}

pub type HostCallback = unsafe extern "C" fn(*mut c_void, usize, *const RuneValue) -> RuneValue;

/// Convert an engine value to the C representation. Strings are interned into
/// `strings` so the pointers stay valid for the callback duration.
pub fn value_to_c(value: Value, strings: &mut Vec<CString>) -> RuneValue {
    if value.is_undefined() {
        return RuneValue::undefined();
    }
    if value.is_null() {
        return RuneValue {
            kind: RUNE_NULL,
            boolean: 0,
            number: 0.0,
            string: std::ptr::null(),
            handle: 0,
        };
    }
    if let Some(boolean) = value.to_boolean() {
        return RuneValue {
            kind: RUNE_BOOLEAN,
            boolean: i32::from(boolean),
            number: 0.0,
            string: std::ptr::null(),
            handle: 0,
        };
    }
    if let Some(smi) = value.as_smi() {
        return RuneValue {
            kind: RUNE_NUMBER,
            boolean: 0,
            number: smi as f64,
            string: std::ptr::null(),
            handle: 0,
        };
    }
    if let Some(float) = value.as_float64() {
        return RuneValue {
            kind: RUNE_NUMBER,
            boolean: 0,
            number: float,
            string: std::ptr::null(),
            handle: 0,
        };
    }
    if let Some(ptr) = value.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_STRING {
            let text = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            let text = CString::new(text.replace('\0', "\\0")).unwrap_or_default();
            let c_string = text.as_ptr();
            strings.push(text);
            return RuneValue {
                kind: RUNE_STRING,
                boolean: 0,
                number: 0.0,
                string: c_string,
                handle: 0,
            };
        }
    }
    RuneValue::object()
}

/// Convert a retained heap value to a C value carrying its handle.
pub fn retained_to_c(value: Value, handle: usize) -> RuneValue {
    let mut out = RuneValue::undefined();
    out.handle = handle as u64 + 1;
    if let Some(ptr) = value.heap_ptr() {
        out.kind = match unsafe { (*(ptr as *const GcHeader)).tag() } {
            TAG_STRING => RUNE_STRING,
            TAG_FUNC => RUNE_FUNCTION,
            TAG_ARRAY | TAG_OBJECT => RUNE_OBJECT,
            _ => RUNE_OBJECT,
        };
    }
    out
}

/// Convert a host-returned value back into an engine value.
pub fn value_from_c(gc: &mut SemiSpace, value: RuneValue) -> Value {
    match value.kind {
        RUNE_NULL => Value::null(),
        RUNE_BOOLEAN => Value::boolean(value.boolean != 0),
        RUNE_NUMBER => {
            if value.number.is_finite()
                && value.number.fract() == 0.0
                && value.number >= i32::MIN as f64
                && value.number <= i32::MAX as f64
            {
                Value::smi(value.number as i32)
            } else {
                Value::from_float64(value.number)
            }
        }
        RUNE_STRING if !value.string.is_null() => {
            let text = unsafe { CStr::from_ptr(value.string) }.to_string_lossy();
            let string = HeapString::allocate(gc, &text);
            Value::from_heap_ptr(string as *mut u8)
        }
        _ => Value::undefined(),
    }
}
