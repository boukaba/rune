use rune_core::gc::{SemiSpace, GcHeader, TAG_STRING};
use rune_core::value::Value;
use rune_core::string::HeapString;
use rune_core::shape::{Shape, PropertyKey};
use rune_core::object::JSObject;
use crate::vm::Vm;

/// A registered built-in function.
pub struct Builtin {
    pub name: &'static str,
    pub func: BuiltinFn,
}

/// Signature for a built-in function: receives GC access, args, and VM reference.
pub type BuiltinFn = fn(gc: &mut SemiSpace, args: &[Value], vm: &Vm) -> Value;

/// Format a Value into its JS string representation.
fn value_to_js_string(v: Value) -> String {
    if v.is_undefined() {
        "undefined".to_string()
    } else if v.is_null() {
        "null".to_string()
    } else if let Some(n) = v.as_smi() {
        n.to_string()
    } else if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            unsafe { HeapString::to_string(ptr as *mut HeapString) }
        } else {
            "[object Object]".to_string()
        }
    } else {
        "undefined".to_string()
    }
}

/// print(...) — outputs values to stdout.
pub fn print_builtin(_gc: &mut SemiSpace, args: &[Value], _vm: &Vm) -> Value {
    let s = args
        .iter()
        .map(|v| format!("{v:?}"))
        .collect::<Vec<_>>()
        .join(" ");
    println!("{s}");
    Value::undefined()
}

/// String(value) — converts a value to its string representation.
pub fn string_builtin(gc: &mut SemiSpace, args: &[Value], _vm: &Vm) -> Value {
    let arg = args.first().copied().unwrap_or(Value::undefined());
    let s = value_to_js_string(arg);
    let ptr = HeapString::allocate(gc, &s);
    Value::from_heap_ptr(ptr as *mut u8)
}

/// Create a minimal JS object with the given property key and string value.
fn make_simple_object(gc: &mut SemiSpace, key: &str, val: Value) -> Value {
    let entries = vec![(PropertyKey::from_string(key), 0usize)];
    let shape = Shape::intern(entries);
    let obj = JSObject::allocate(gc, shape, &[val]);
    Value::from_heap_ptr(obj as *mut u8)
}

/// Error(message) — creates a minimal error object with a `message` property.
pub fn error_builtin(gc: &mut SemiSpace, args: &[Value], _vm: &Vm) -> Value {
    let msg = if let Some(arg) = args.first() {
        value_to_js_string(*arg)
    } else {
        String::new()
    };
    let msg_str = HeapString::allocate(gc, &msg);
    let msg_val = Value::from_heap_ptr(msg_str as *mut u8);
    make_simple_object(gc, "message", msg_val)
}

/// Test262Error(message) — built-in replacement for sta.js Test262Error constructor.
pub fn test262_error_builtin(gc: &mut SemiSpace, args: &[Value], vm: &Vm) -> Value {
    error_builtin(gc, args, vm)
}

/// $DONOTEVALUATE() — throws an error (should be optimized away by runner).
pub fn donot_evaluate_builtin(_gc: &mut SemiSpace, _args: &[Value], _vm: &Vm) -> Value {
    panic!("$DONOTEVALUATE was called");
}

/// Object(value) — returns a new empty object (ignores argument).
pub fn object_builtin(gc: &mut SemiSpace, _args: &[Value], _vm: &Vm) -> Value {
    let shape = Shape::empty();
    let ptr = JSObject::allocate(gc, shape, &[]);
    Value::from_heap_ptr(ptr as *mut u8)
}

/// Object.create(proto) — creates a new object with the given prototype.
/// Per §20.1.2.2, throws TypeError if proto is not an Object or null.
pub fn object_create_builtin(gc: &mut SemiSpace, args: &[Value], _vm: &Vm) -> Value {
    let shape = Shape::empty();
    let ptr = JSObject::allocate(gc, shape, &[]);
    if let Some(proto) = args.first() {
        if proto.is_null() {
            // null prototype: already set by default (prototype field = null)
        } else if let Some(proto_ptr) = proto.heap_ptr() {
            unsafe {
                JSObject::set_prototype(ptr, proto_ptr);
            }
        } else {
            // proto is not an object and not null — throw TypeError
            panic!("TypeError: Object.prototype.toString called on non-object"); // simplified
        }
    }
    Value::from_heap_ptr(ptr as *mut u8)
}

/// eval(source) — currently not implemented; returns undefined.
pub fn eval_builtin(_gc: &mut SemiSpace, _args: &[Value], _vm: &Vm) -> Value {
    Value::undefined()
}

/// Return a list of builtins to register in every new Vm.
pub fn default_builtins() -> Vec<Builtin> {
    vec![
        Builtin { name: "print", func: print_builtin },
        Builtin { name: "String", func: string_builtin },
        Builtin { name: "Object", func: object_builtin },
        Builtin { name: "Error", func: error_builtin },
        Builtin { name: "Test262Error", func: test262_error_builtin },
        Builtin { name: "$DONOTEVALUATE", func: donot_evaluate_builtin },
        Builtin { name: "eval", func: eval_builtin },
        Builtin { name: "Object_create", func: object_create_builtin }, // accessible only via Object.create
    ]
}

/// Build a wrapper object for the Object constructor, exposing methods like .create().
/// Returns (object_value, create_builtin_smi_index).
pub fn build_object_constructor(gc: &mut SemiSpace) -> Value {
    let shape = Shape::empty();
    let ptr = JSObject::allocate(gc, shape, &[]);
    Value::from_heap_ptr(ptr as *mut u8)
}
