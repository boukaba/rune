//! F2: single choke point for all error-value creation (completed in A2).
//!
//! Every error the engine raises — VM TypeErrors/ReferenceErrors, builtin
//! pending exceptions, assert failures — is built by [`error_object`], with
//! the error kind as data instead of baked into ad-hoc `"Kind: message"`
//! strings. User `throw` of arbitrary values (strings, objects) still flows
//! through untouched — only engine-raised errors come from here.

use rune_core::gc::{GcHeader, SemiSpace, TAG_OBJECT};
use rune_core::object::JSObject;
use rune_core::shape::{PropertyKey, Shape};
use rune_core::string::HeapString;
use rune_core::value::Value;

/// The seven native error kinds (§19.5.5–§22). Index matches
/// `ERROR_TYPE_NAMES` / `Vm::error_protos` order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorKind {
    Error,
    EvalError,
    RangeError,
    ReferenceError,
    SyntaxError,
    TypeError,
    UriError,
}

impl ErrorKind {
    pub fn name(self) -> &'static str {
        match self {
            ErrorKind::Error => "Error",
            ErrorKind::EvalError => "EvalError",
            ErrorKind::RangeError => "RangeError",
            ErrorKind::ReferenceError => "ReferenceError",
            ErrorKind::SyntaxError => "SyntaxError",
            ErrorKind::TypeError => "TypeError",
            ErrorKind::UriError => "URIError",
        }
    }

    /// Index into `ERROR_TYPE_NAMES` / `Vm::error_protos`.
    pub fn proto_index(self) -> usize {
        match self {
            ErrorKind::Error => 0,
            ErrorKind::EvalError => 1,
            ErrorKind::RangeError => 2,
            ErrorKind::ReferenceError => 3,
            ErrorKind::SyntaxError => 4,
            ErrorKind::TypeError => 5,
            ErrorKind::UriError => 6,
        }
    }

    /// Parse a `"Kind"` prefix (the legacy `"Kind: message"` encoding).
    /// Returns `None` for unprefixed / unknown names (caller uses Error).
    pub fn parse(s: &str) -> Option<ErrorKind> {
        match s {
            "Error" => Some(ErrorKind::Error),
            "EvalError" => Some(ErrorKind::EvalError),
            "RangeError" => Some(ErrorKind::RangeError),
            "ReferenceError" => Some(ErrorKind::ReferenceError),
            "SyntaxError" => Some(ErrorKind::SyntaxError),
            "TypeError" => Some(ErrorKind::TypeError),
            "URIError" => Some(ErrorKind::UriError),
            _ => None,
        }
    }

    /// Split a legacy `"Kind: message"` string into (kind, message).
    /// No `": "` separator → (Error, whole string).
    pub fn split_legacy(msg: &str) -> (ErrorKind, &str) {
        if let Some(idx) = msg.find(": ") {
            if idx < 64 && !msg[..idx].is_empty() {
                if let Some(kind) = ErrorKind::parse(&msg[..idx]) {
                    return (kind, &msg[idx + 2..]);
                }
            }
        }
        (ErrorKind::Error, msg)
    }
}

/// Canonical error object: own `name` + `message` string properties with
/// [[Prototype]] linked to the kind's `error_protos` entry (when available).
/// Same own-properties as the old `make_error_object`; the prototype link
/// is new (needed by A2 instanceof work; suite-neutral — verified by the
/// Error/NativeErrors/Function diffs in the F2 commit).
pub fn error_object(
    gc: &mut SemiSpace,
    error_protos: &[Value],
    kind: ErrorKind,
    msg: &str,
) -> Value {
    let name_str = HeapString::allocate(gc, kind.name()) as *mut u8;
    let msg_str = HeapString::allocate(gc, msg) as *mut u8;
    let entries = vec![
        (PropertyKey::from_string("name"), 0usize),
        (PropertyKey::from_string("message"), 1usize),
    ];
    let key_names = vec!["name".to_string(), "message".to_string()];
    let shape = Shape::intern(entries, key_names);
    let obj = JSObject::allocate(
        gc,
        shape,
        &[
            Value::from_heap_ptr(name_str),
            Value::from_heap_ptr(msg_str),
        ],
    );
    if let Some(proto) = error_protos
        .get(kind.proto_index())
        .and_then(|v| v.heap_ptr())
    {
        unsafe {
            JSObject::set_prototype(obj, proto);
        }
    }
    Value::from_heap_ptr(obj as *mut u8)
}

/// TAG_OBJECT check helper for readers shared by both encodings.
pub fn is_error_object(val: Value) -> bool {
    val.heap_ptr()
        .is_some_and(|ptr| unsafe { (*(ptr as *const GcHeader)).tag() == TAG_OBJECT })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_core::object::JSObject;
    use rune_core::shape::Shape;

    #[test]
    fn test_kind_names_and_indices() {
        let cases = [
            (ErrorKind::Error, "Error", 0),
            (ErrorKind::EvalError, "EvalError", 1),
            (ErrorKind::RangeError, "RangeError", 2),
            (ErrorKind::ReferenceError, "ReferenceError", 3),
            (ErrorKind::SyntaxError, "SyntaxError", 4),
            (ErrorKind::TypeError, "TypeError", 5),
            (ErrorKind::UriError, "URIError", 6),
        ];
        for (kind, name, idx) in cases {
            assert_eq!(kind.name(), name);
            assert_eq!(kind.proto_index(), idx);
            assert_eq!(ErrorKind::parse(name), Some(kind));
        }
        assert_eq!(ErrorKind::parse("Bogus"), None);
        assert_eq!(ErrorKind::parse(""), None);
    }

    #[test]
    fn test_split_legacy() {
        assert_eq!(
            ErrorKind::split_legacy("TypeError: bad thing"),
            (ErrorKind::TypeError, "bad thing")
        );
        assert_eq!(
            ErrorKind::split_legacy("RangeError: x: y"),
            (ErrorKind::RangeError, "x: y")
        );
        // No prefix → plain Error with the whole string.
        assert_eq!(
            ErrorKind::split_legacy("assert.sameValue: nope"),
            (ErrorKind::Error, "assert.sameValue: nope")
        );
        assert_eq!(
            ErrorKind::split_legacy("140-character run of text with no colon space at all"),
            (
                ErrorKind::Error,
                "140-character run of text with no colon space at all"
            )
        );
        // Unknown prefix stays whole (matches the old read_error_name
        // behavior of returning the full string when no known prefix).
        assert_eq!(
            ErrorKind::split_legacy("Bogus: thing"),
            (ErrorKind::Error, "Bogus: thing")
        );
    }

    #[test]
    fn test_error_object_product() {
        let mut ss = SemiSpace::new();
        // Fake prototype stand-in (linkage stores the pointer, no deref).
        let fake_proto = JSObject::allocate(&mut ss, Shape::empty(), &[]) as *mut u8;
        let fake_val = Value::from_heap_ptr(fake_proto);
        let mut protos = vec![Value::undefined(); 7];
        protos[ErrorKind::TypeError.proto_index()] = fake_val;
        let v = error_object(&mut ss, &protos, ErrorKind::TypeError, "bad thing");
        let ptr = v.heap_ptr().unwrap() as *mut JSObject;
        unsafe {
            assert_eq!(JSObject::prototype(ptr), fake_proto);
            let shape = JSObject::shape_ptr(ptr);
            let get = |key: &str| {
                let slot = shape.lookup(&PropertyKey::from_string(key)).unwrap();
                JSObject::get_slot(ptr, slot)
            };
            let name = get("name").heap_ptr().unwrap();
            assert_eq!(HeapString::to_string(name as *mut HeapString), "TypeError");
            let msg = get("message").heap_ptr().unwrap();
            assert_eq!(HeapString::to_string(msg as *mut HeapString), "bad thing");
        }
    }

    #[test]
    fn test_error_object_no_protos() {
        // Empty proto table → null prototype, own props still set.
        let mut ss = SemiSpace::new();
        let v = error_object(&mut ss, &[], ErrorKind::RangeError, "too big");
        let ptr = v.heap_ptr().unwrap() as *mut JSObject;
        unsafe {
            assert!(JSObject::prototype(ptr).is_null());
        }
        assert!(is_error_object(v));
    }
}
