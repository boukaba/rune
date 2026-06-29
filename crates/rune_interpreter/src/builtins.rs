use crate::vm::Vm;
use rune_core::array::RuneArray;
use rune_core::gc::{GcHeader, SemiSpace, TAG_ARRAY, TAG_FUNC, TAG_OBJECT, TAG_PROMISE, TAG_STRING, TAG_STRING_OBJ};
use rune_core::object::JSObject;
use rune_core::promise::{Promise, PROMISE_FULFILLED, PROMISE_PENDING, PROMISE_REJECTED};
use rune_core::shape::{DENSE_ARRAY_SHAPE, PropertyKey, Shape};
use rune_core::string::HeapString;
use rune_core::string_object::StringObject;
use rune_core::value::Value;

/// A registered built-in function.
pub struct Builtin {
    pub name: &'static str,
    pub length: u32,
    pub func: BuiltinFn,
}

/// Signature for a built-in function: receives GC access, `this` value, args, and VM reference.
pub type BuiltinFn = fn(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value;



/// Format a Value into its JS string representation.
pub fn value_to_js_string(v: Value) -> String {
    if v.is_undefined() {
        "undefined".to_string()
    } else if v.is_null() {
        "null".to_string()
    } else if let Some(b) = v.to_boolean() {
        b.to_string()
    } else if let Some(n) = v.as_smi() {
        n.to_string()
    } else if let Some(f) = v.as_float64() {
        f.to_string()
    } else if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            unsafe { HeapString::to_string(ptr as *mut HeapString) }
        } else if tag == TAG_STRING_OBJ {
            let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
            unsafe { HeapString::to_string(str_ptr as *mut HeapString) }
        } else {
            "[object Object]".to_string()
        }
    } else {
        "undefined".to_string()
    }
}

/// print(...) — outputs values to stdout.
pub fn print_builtin(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let s = args
        .iter()
        .map(|v| value_to_js_string(*v))
        .collect::<Vec<_>>()
        .join(" ");
    println!("{s}");
    Value::undefined()
}

/// Try to convert a value to a string by calling ToPrimitive with string hint.
/// For objects with a user-defined toString function, sets up the pending_call
/// callback pattern and returns None (the caller must return immediately).
/// For all other values, returns Some(string).
pub(crate) fn to_primitive_string(
    gc: &mut SemiSpace,
    val: Value,
    vm: &mut Vm,
) -> Option<String> {
    // Fast path: non-object values
    if !val.is_heap_object() {
        return Some(value_to_js_string(val));
    }
    let ptr = val.heap_ptr().unwrap();
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    // Strings and String wrappers are already primitive strings
    if tag == TAG_STRING {
        return Some(unsafe { HeapString::to_string(ptr as *mut HeapString) });
    }
    if tag == TAG_STRING_OBJ {
        let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
        return Some(unsafe { HeapString::to_string(str_ptr as *mut HeapString) });
    }
    if tag == TAG_OBJECT {
        // §7.1.1 ToPrimitive with string hint: call toString(), then valueOf()
        let key = PropertyKey::from_string("toString");
        let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
        if let Some(slot) = shape.lookup(&key) {
            let to_string_val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
            if let Some(smi) = to_string_val.as_smi() {
                if smi < 0 {
                    // Builtin toString — call it directly
                    let id = ((-smi) as usize) - 1;
                    if id < vm.builtins.len() {
                        let result = (vm.builtins[id].func)(gc, val, &[], vm);
                        if let Some(exc) = vm.pending_exception.take() {
                            vm.pending_exception = Some(exc);
                            return None;
                        }
                        // ToPrimitive: if result is a primitive, return it
                        if !result.is_heap_object() || {
                            if let Some(rp) = result.heap_ptr() {
                                let rt = unsafe { (*(rp as *const GcHeader)).tag() };
                                rt == TAG_STRING
                            } else {
                                false
                            }
                        } {
                            return Some(value_to_js_string(result));
                        }
                    }
                }
            } else if let Some(func_ptr) = to_string_val.heap_ptr() {
                let func_tag = unsafe { (*(func_ptr as *const GcHeader)).tag() };
                if func_tag == rune_core::gc::TAG_FUNC {
                    // User-defined toString — use pending callback pattern
                    let depth = vm.frame_depth();
                    vm.pending_call = Some(crate::vm::PendingCall {
                        source_frame_depth: depth,
                    });
                    vm.push_callback_call(gc, to_string_val, val, vec![]);
                    return None; // caller must return immediately
                }
            }
        }
        // Fall through to valueOf if no toString or toString didn't return a primitive
        let value_of_key = PropertyKey::from_string("valueOf");
        if let Some(slot) = shape.lookup(&value_of_key) {
            let value_of_val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
            if let Some(smi) = value_of_val.as_smi()
                && smi < 0 {
                    let id = ((-smi) as usize) - 1;
                    if id < vm.builtins.len() {
                        let result = (vm.builtins[id].func)(gc, val, &[], vm);
                        if let Some(exc) = vm.pending_exception.take() {
                            vm.pending_exception = Some(exc);
                            return None;
                        }
                        if !result.is_heap_object() || {
                            if let Some(rp) = result.heap_ptr() {
                                let rt = unsafe { (*(rp as *const GcHeader)).tag() };
                                rt == TAG_STRING
                            } else {
                                false
                            }
                        } {
                            return Some(value_to_js_string(result));
                        }
                    }
                }
        }
        // Neither toString nor valueOf returned a primitive
        return Some(value_to_js_string(val));
    }
    Some(value_to_js_string(val))
}

/// Synchronous version of to_primitive_string — never sets up callbacks.
/// User-defined toString/valueOf are skipped (fall through to [object Object]).
/// Use this for string method arguments where the callback pattern would leak.
pub(crate) fn to_primitive_string_sync(
    val: Value,
    gc: &mut SemiSpace,
    vm: &mut Vm,
) -> String {
    if !val.is_heap_object() {
        return value_to_js_string(val);
    }
    let ptr = val.heap_ptr().unwrap();
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag == TAG_STRING {
        return unsafe { HeapString::to_string(ptr as *mut HeapString) };
    }
    if tag == TAG_STRING_OBJ {
        let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
        return unsafe { HeapString::to_string(str_ptr as *mut HeapString) };
    }
    if tag == TAG_OBJECT {
        let key = PropertyKey::from_string("toString");
        let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
        if let Some(slot) = shape.lookup(&key) {
            let to_string_val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
            if let Some(smi) = to_string_val.as_smi()
                && smi < 0 {
                    let id = ((-smi) as usize) - 1;
                    if id < vm.builtins.len() {
                        let result = (vm.builtins[id].func)(gc, val, &[], vm);
                        if let Some(exc) = vm.pending_exception.take() {
                            vm.pending_exception = Some(exc);
                            return value_to_js_string(val);
                        }
                        if !result.is_heap_object() || {
                            if let Some(rp) = result.heap_ptr() {
                                let rt = unsafe { (*(rp as *const GcHeader)).tag() };
                                rt == TAG_STRING
                            } else { false }
                        } {
                            return value_to_js_string(result);
                        }
                    }
                }
            // User-defined or non-callable toString — skip
        }
        let value_of_key = PropertyKey::from_string("valueOf");
        if let Some(slot) = shape.lookup(&value_of_key) {
            let value_of_val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
            if let Some(smi) = value_of_val.as_smi()
                && smi < 0 {
                    let id = ((-smi) as usize) - 1;
                    if id < vm.builtins.len() {
                        let result = (vm.builtins[id].func)(gc, val, &[], vm);
                        if let Some(exc) = vm.pending_exception.take() {
                            vm.pending_exception = Some(exc);
                            return value_to_js_string(val);
                        }
                        if !result.is_heap_object() || {
                            if let Some(rp) = result.heap_ptr() {
                                let rt = unsafe { (*(rp as *const GcHeader)).tag() };
                                rt == TAG_STRING
                            } else { false }
                        } {
                            return value_to_js_string(result);
                        }
                    }
                }
            // User-defined or non-callable valueOf — skip
        }
        return value_to_js_string(val);
    }
    value_to_js_string(val)
}

/// Convert a string to f64 per ToNumber(string) spec.
fn string_to_number(s: &str) -> f64 {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return 0.0;
    }
    if let Ok(n) = trimmed.parse::<f64>() {
        return n;
    }
    let upper = trimmed.to_uppercase();
    if upper.starts_with("0X") && let Ok(n) = u64::from_str_radix(&upper[2..], 16) {
        return n as f64;
    }
    if trimmed.eq_ignore_ascii_case("infinity")
        || trimmed == "+Infinity"
    {
        return f64::INFINITY;
    }
    if trimmed == "-Infinity" {
        return f64::NEG_INFINITY;
    }
    f64::NAN
}

/// Build a comma-separated string representation of a dense array.
fn array_to_string(arr: *mut RuneArray) -> String {
    unsafe {
        let len = RuneArray::length(arr);
        if len == 0 {
            return String::new();
        }
        let mut parts: Vec<String> = Vec::with_capacity(len as usize);
        for i in 0..len as usize {
            let elem = RuneArray::get_element(arr, i);
            parts.push(value_to_js_string(elem));
        }
        parts.join(",")
    }
}

/// Number(value) — converts a value to a number.
/// Per §21.1.2.1: calls ToNumber via ToPrimitive with NUMBER hint.
pub fn number_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    // §21.1.2.1: If no arguments, return +0
    let val = match args.first().copied() {
        Some(v) => v,
        None => return Value::smi(0),
    };
    if val.is_undefined() {
        return Value::from_float64(f64::NAN);
    }
    if val.is_null() || val.is_boolean() {
        let n = if val.is_null() || val.to_boolean() == Some(false) { 0.0 } else { 1.0 };
        return Value::from_float64(n);
    }
    if let Some(n) = val.as_smi() {
        return Value::smi(n);
    }
    if let Some(f) = val.as_float64() {
        return Value::from_float64(f);
    }
    if let Some(ptr) = val.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            return Value::from_float64(string_to_number(&s));
        }
        if tag == TAG_STRING_OBJ {
            let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
            let s = unsafe { HeapString::to_string(str_ptr as *mut HeapString) };
            return Value::from_float64(string_to_number(&s));
        }
        if tag == TAG_ARRAY {
            let s = array_to_string(ptr as *mut RuneArray);
            return Value::from_float64(string_to_number(&s));
        }
        if tag == TAG_OBJECT {
            let s = to_primitive_string_sync(val, gc, vm);
            return Value::from_float64(string_to_number(&s));
        }
    }
    Value::from_float64(f64::NAN)
}

/// String(value) — converts a value to its string representation.
/// Per §21.1.2.1: calls ToString via ToPrimitive with string hint.
pub fn string_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let arg = args.first().copied().unwrap_or(Value::undefined());
    match to_primitive_string(gc, arg, vm) {
        Some(s) => {
            let ptr = HeapString::allocate(gc, &s);
            Value::from_heap_ptr(ptr as *mut u8)
        }
        None => {
            // Pending callback was set up — return undefined and let the
            // pending_call machinery handle the result.
            Value::undefined()
        }
    }
}

/// SameValueZero comparison for Array.prototype.includes.
/// - NaN matches NaN (unlike ===)
/// - +0 and -0 are equal (unlike SameValue)
/// - Smi 0 and float64 -0/+0 are equal (same numeric value)
fn same_value_zero(a: Value, b: Value) -> bool {
    if a.raw() == b.raw() {
        return true;
    }
    // Check for +0 vs -0 in any encoding (Smi or float64)
    let is_zero = |v: Value| -> bool {
        v.as_smi() == Some(0) || (v.is_float64() && f64::from_bits(v.raw()) == 0.0)
    };
    if is_zero(a) && is_zero(b) {
        return true;
    }
    false
}

/// Create a minimal JS object with the given property key and string value.
fn make_simple_object(gc: &mut SemiSpace, key: &str, val: Value) -> Value {
    let entries = vec![(PropertyKey::from_string(key), 0usize)];
    let key_names = vec![key.to_string()];
    let shape = Shape::intern(entries, key_names);
    let obj = JSObject::allocate(gc, shape, &[val]);
    Value::from_heap_ptr(obj as *mut u8)
}

/// Error(message) — creates a minimal error object with a `message` property.
pub fn error_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
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
pub fn test262_error_builtin(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    error_builtin(gc, _this, args, vm)
}

/// $DONOTEVALUATE() — throws an error (should be optimized away by runner).
pub fn donot_evaluate_builtin(
    _gc: &mut SemiSpace,
    _this: Value,
    _args: &[Value],
    _vm: &mut Vm,
) -> Value {
    panic!("$DONOTEVALUATE was called");
}

/// Object(value) — returns a new empty object (ignores argument).
pub fn object_builtin(gc: &mut SemiSpace, _this: Value, _args: &[Value], _vm: &mut Vm) -> Value {
    let shape = Shape::empty();
    let ptr = JSObject::allocate(gc, shape, &[]);
    Value::from_heap_ptr(ptr as *mut u8)
}

// ── Object.keys / values / entries ────────────────────────────────

/// Iterate own enumerable string-keyed properties of a value.
/// Returns Ok(entries) or Err(()) if a TypeError was thrown (null/undefined).
fn object_own_entries(
    gc: &mut SemiSpace,
    val: Value,
    vm: &mut Vm,
) -> Result<Vec<(String, Value)>, ()> {
    if val.is_null() || val.is_undefined() {
        let msg = crate::vm::heap_string(gc, "TypeError: Object.keys called on null or undefined");
        vm.set_pending_exception(Value::from_heap_ptr(msg));
        return Err(());
    }
    if let Some(ptr) = val.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        match tag {
            TAG_OBJECT => {
                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                let count = unsafe { JSObject::slot_count(ptr as *mut JSObject) };
                let mut entries = Vec::with_capacity(count);
                for i in 0..count {
                    let key = shape.key_name_at(i).unwrap_or("").to_string();
                    let value = unsafe { JSObject::get_slot(ptr as *mut JSObject, i) };
                    entries.push((key, value));
                }
                Ok(entries)
            }
            TAG_ARRAY => {
                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) } as usize;
                let mut entries = Vec::with_capacity(len);
                for i in 0..len {
                    let value = unsafe { RuneArray::get_element(ptr as *mut RuneArray, i) };
                    entries.push((i.to_string(), value));
                }
                Ok(entries)
            }
            TAG_STRING => {
                let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
                let mut entries = Vec::with_capacity(s.len());
                for (i, c) in s.chars().enumerate() {
                    let ch: String = c.to_string();
                    let ch_val =
                        Value::from_heap_ptr(HeapString::allocate(gc, &ch) as *mut u8);
                    entries.push((i.to_string(), ch_val));
                }
                Ok(entries)
            }
            TAG_STRING_OBJ => {
                let str_ptr =
                    unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
                let s = unsafe { HeapString::to_string(str_ptr as *mut HeapString) };
                let mut entries = Vec::with_capacity(s.len());
                for (i, c) in s.chars().enumerate() {
                    let ch: String = c.to_string();
                    let ch_val =
                        Value::from_heap_ptr(HeapString::allocate(gc, &ch) as *mut u8);
                    entries.push((i.to_string(), ch_val));
                }
                Ok(entries)
            }
            _ => Ok(Vec::new()),
        }
    } else {
        // Smi, float64, boolean — no own enumerable properties
        Ok(Vec::new())
    }
}

/// Build a dense RuneArray from element values, wired to Array.prototype.
fn build_array(gc: &mut SemiSpace, elements: &[Value], vm: &Vm) -> Value {
    let arr = RuneArray::allocate(gc, elements);
    unsafe {
        let arr_u8 = arr as *mut u8;
        *(arr_u8.add(8) as *mut *const Shape) = *DENSE_ARRAY_SHAPE as *const Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(arr_u8.add(24) as *mut *mut u8) = proto;
        }
    }
    Value::from_heap_ptr(arr as *mut u8)
}

/// Object.keys(obj) — returns array of own enumerable string-keyed property names.
pub fn object_keys(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let target = args.first().copied().unwrap_or(Value::undefined());
    let entries = match object_own_entries(gc, target, vm) {
        Ok(e) => e,
        Err(()) => return Value::undefined(),
    };
    let keys: Vec<Value> = entries
        .iter()
        .map(|(k, _)| Value::from_heap_ptr(HeapString::allocate(gc, k) as *mut u8))
        .collect();
    build_array(gc, &keys, vm)
}

/// Object.values(obj) — returns array of own enumerable property values.
pub fn object_values(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let target = args.first().copied().unwrap_or(Value::undefined());
    let entries = match object_own_entries(gc, target, vm) {
        Ok(e) => e,
        Err(()) => return Value::undefined(),
    };
    let vals: Vec<Value> = entries.iter().map(|(_, v)| *v).collect();
    build_array(gc, &vals, vm)
}

/// Object.entries(obj) — returns array of [key, value] pairs.
pub fn object_entries(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let target = args.first().copied().unwrap_or(Value::undefined());
    let entries = match object_own_entries(gc, target, vm) {
        Ok(e) => e,
        Err(()) => return Value::undefined(),
    };
    let pairs: Vec<Value> = entries
        .iter()
        .map(|(k, v)| {
            let key_val =
                Value::from_heap_ptr(HeapString::allocate(gc, k) as *mut u8);
            let pair_elems = [key_val, *v];
            let pair_arr = RuneArray::allocate(gc, &pair_elems);
            unsafe {
                let ptr = pair_arr as *mut u8;
                *(ptr.add(8) as *mut *const Shape) = *DENSE_ARRAY_SHAPE as *const Shape;
                if let Some(proto) = vm.array_prototype.heap_ptr() {
                    *(ptr.add(24) as *mut *mut u8) = proto;
                }
            }
            Value::from_heap_ptr(pair_arr as *mut u8)
        })
        .collect();
    build_array(gc, &pairs, vm)
}

/// Object.create(proto) — creates a new object with the given prototype.
/// Per §20.1.2.2, throws TypeError if proto is not an Object or null.
pub fn object_create_builtin(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
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
            // proto is not an object and not null — TypeError per §20.1.2.2
            let msg =
                crate::vm::heap_string(gc, "TypeError: Object.create expects an object or null");
            vm.set_pending_exception(Value::from_heap_ptr(msg));
        }
    }
    Value::from_heap_ptr(ptr as *mut u8)
}

/// eval(source) — currently not implemented; returns undefined.
pub fn eval_builtin(_gc: &mut SemiSpace, _this: Value, _args: &[Value], _vm: &mut Vm) -> Value {
    Value::undefined()
}

/// Array.isArray(arg) — returns true if arg is a dense array.
pub fn array_is_array(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let val = args.first().copied().unwrap_or(Value::undefined());
    if let Some(ptr) = val.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_ARRAY {
            return Value::boolean(true);
        }
    }
    Value::boolean(false)
}

/// Array.prototype.push(value) — pushes value to the array, returns new length.
/// Auto-grows the array if capacity is exhausted and updates VM references.
pub fn array_push(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let val = args.first().copied().unwrap_or(Value::undefined());
    if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_ARRAY {
            unsafe {
                let old_ptr = ptr;
                let new_arr = RuneArray::push(gc, old_ptr as *mut RuneArray, val);
                if new_arr as *mut u8 != old_ptr {
                    // If GC ran during push, old_ptr may be a stale from-space address.
                    // Resolve to the current to-space address for the root update.
                    let resolved_old = if (*(old_ptr as *const GcHeader)).is_forwarded() {
                        (*(old_ptr as *const GcHeader)).forwarding_addr()
                    } else {
                        old_ptr
                    };
                    if resolved_old != new_arr as *mut u8 {
                        vm.update_heap_reference(resolved_old, new_arr as *mut u8);
                    }
                }
                let len = RuneArray::length(new_arr);
                return Value::smi(len as i32);
            }
        }
    }
    Value::undefined()
}

/// Array.prototype.pop() — removes and returns the last element.
pub fn array_pop(_gc: &mut SemiSpace, this: Value, _args: &[Value], _vm: &mut Vm) -> Value {
    if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_ARRAY {
            unsafe {
                return RuneArray::pop(ptr as *mut RuneArray);
            }
        }
    }
    Value::undefined()
}

/// String.fromCharCode(codes...) — creates a string from char codes.
pub fn string_from_char_code(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    _vm: &mut Vm,
) -> Value {
    let mut s = String::new();
    for arg in args {
        if let Some(n) = arg.as_smi() {
            s.push(char::from_u32(n as u32).unwrap_or('\u{FFFD}'));
        }
    }
    let ptr = HeapString::allocate(gc, &s);
    Value::from_heap_ptr(ptr as *mut u8)
}

/// Extract the underlying string content from a TAG_STRING or TAG_STRING_OBJ value.
fn string_from_value(this: Value) -> String {
    if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            return unsafe { HeapString::to_string(ptr as *mut HeapString) };
        }
        if tag == TAG_STRING_OBJ {
            let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
            return unsafe { HeapString::to_string(str_ptr as *mut HeapString) };
        }
    }
    value_to_js_string(this)
}

/// RequireObjectCoercible(this) — throws TypeError if this is null or undefined.
fn require_object_coercible(this: Value, vm: &mut Vm, gc: &mut SemiSpace) -> bool {
    if this.is_null() || this.is_undefined() {
        let err = make_error(gc, "TypeError: Cannot convert undefined or null to object");
        vm.set_pending_exception(err);
        return false;
    }
    true
}

/// String.prototype.charAt(index) — returns the character at index as a string.
/// Per §22.1.3.1, OOB returns empty string, not undefined.
pub fn string_char_at(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let index = args.first().and_then(|v| v.as_smi()).unwrap_or(0) as usize;
    let s = string_from_value(this);
    if index >= s.chars().count() {
        let empty = HeapString::allocate(gc, "");
        return Value::from_heap_ptr(empty as *mut u8);
    }
    let ch = s.chars().nth(index).unwrap();
    let result = HeapString::allocate(gc, &ch.to_string());
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.slice(start, end) — returns a substring.
/// Per ECMAScript §22.1.3.23 (String.prototype.slice).
/// Uses byte-level slicing to match the spec (characters are 1 byte in Rune's use case).
pub fn string_slice(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    fn to_number(v: Value) -> f64 {
        v.as_smi().map(|n| n as f64)
            .or_else(|| v.as_float64())
            .unwrap_or(f64::NAN)
    }
    let s = string_from_value(this);
    let len = s.len() as f64;
    let raw_start = args.first().map(|&v| to_number(v)).unwrap_or(0.0);
    let raw_end = args.get(1).map(|&v| to_number(v));
    let int_start = if raw_start.is_nan() { 0.0 } else { raw_start };
    let int_end = match raw_end {
        Some(e) if e.is_nan() => 0.0,
        Some(e) => e,
        None => len,
    };
    let clamp = |v: f64| -> usize {
        let v = if v.is_infinite() { if v.is_sign_negative() { 0.0 } else { len } }
                else if v < 0.0 { (len + v).max(0.0) }
                else { v.min(len) };
        v as usize
    };
    let start = clamp(int_start);
    let end = clamp(int_end);
    if start >= end {
        let empty = HeapString::allocate(gc, "");
        return Value::from_heap_ptr(empty as *mut u8);
    }
    let result_s = &s[start..end];
    let result = HeapString::allocate(gc, result_s);
    Value::from_heap_ptr(result as *mut u8)
}

/// Convert an optional argument to a string via ToPrimitive (sync, no callbacks).
/// Never returns pending — use for string method arguments where the callback
/// pattern would leak the callback's result to the builtin's caller.
fn arg_to_string(gc: &mut SemiSpace, v: Option<Value>, vm: &mut Vm) -> String {
    let val = v.unwrap_or(Value::undefined());
    to_primitive_string_sync(val, gc, vm)
}

/// String.prototype.indexOf(searchString, position) — returns the index of the first occurrence.
pub fn string_index_of(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    let pos = args.get(1).copied().unwrap_or(Value::undefined());
    let start = if pos.is_undefined() {
        0
    } else if let Some(smi) = pos.as_smi() {
        if smi < 0 { 0 } else { smi as usize }
    } else if let Some(f) = pos.as_float64() {
        let clamped = if f.is_nan() || f < 0.0 { 0.0 } else { f };
        (clamped as usize).min(s.len())
    } else {
        0
    };
    let start = start.min(s.len());
    if search_str.is_empty() {
        return Value::smi(start as i32);
    }
    if start + search_str.len() > s.len() {
        return Value::smi(-1);
    }
    if let Some(idx) = s[start..].find(&search_str) {
        Value::smi((start + idx) as i32)
    } else {
        Value::smi(-1)
    }
}

/// String.prototype.includes(searchString, position) — returns true if searchString is found.
pub fn string_includes(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    let pos = args.get(1).copied().unwrap_or(Value::undefined());
    let start = if pos.is_undefined() {
        0
    } else if let Some(smi) = pos.as_smi() {
        if smi < 0 { 0 } else { smi as usize }
    } else if let Some(f) = pos.as_float64() {
        let clamped = if f.is_nan() || f < 0.0 { 0.0 } else { f };
        (clamped as usize).min(s.len())
    } else {
        0
    };
    let start = start.min(s.len());
    if search_str.is_empty() {
        return Value::boolean(true);
    }
    if start + search_str.len() > s.len() {
        return Value::boolean(false);
    }
    Value::boolean(s[start..].contains(&search_str))
}

/// String.prototype.startsWith(searchString, position) — checks if string starts with searchString.
pub fn string_starts_with(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    let pos = args.get(1).copied().unwrap_or(Value::undefined());
    let start = if pos.is_undefined() {
        0
    } else if let Some(smi) = pos.as_smi() {
        if smi < 0 { 0 } else { smi as usize }
    } else if let Some(f) = pos.as_float64() {
        let clamped = if f.is_nan() || f < 0.0 { 0.0 } else { f };
        (clamped as usize).min(s.len())
    } else {
        0
    };
    Value::boolean(s[start..].starts_with(&search_str))
}

/// String.prototype.endsWith(searchString, endPosition) — checks if string ends with searchString.
pub fn string_ends_with(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    let end_pos = args.get(1).copied().unwrap_or(Value::undefined());
    let end = if end_pos.is_undefined() {
        s.len()
    } else if let Some(smi) = end_pos.as_smi() {
        if smi < 0 { 0 } else { smi as usize }
    } else if let Some(f) = end_pos.as_float64() {
        let clamped = if f.is_nan() || f < 0.0 { 0.0 } else { f };
        (clamped as usize).min(s.len())
    } else {
        s.len()
    };
    Value::boolean(s[..end].ends_with(&search_str))
}

fn to_integer_or_infinity(v: Value) -> f64 {
    if v.is_undefined() || v.is_null() {
        return 0.0;
    }
    if let Some(b) = v.to_boolean() {
        return if b { 1.0 } else { 0.0 };
    }
    if let Some(smi) = v.as_smi() {
        return smi as f64;
    }
    if let Some(f) = v.as_float64() {
        if f.is_nan() {
            return 0.0;
        }
        return f.trunc();
    }
    0.0
}

/// String.prototype.charCodeAt(index) — returns 16-bit UTF-16 code unit at position.
pub fn string_char_code_at(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let pos = args.first().copied().unwrap_or(Value::undefined());
    let idx = to_integer_or_infinity(pos) as isize;
    if idx < 0 || idx as usize >= s.len() {
        return Value::from_float64(f64::NAN);
    }
    let byte = s.as_bytes()[idx as usize];
    Value::smi(byte as i32)
}

/// String.prototype.codePointAt(index) — returns Unicode code point at position.
pub fn string_code_point_at(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let pos = args.first().copied().unwrap_or(Value::undefined());
    let idx = to_integer_or_infinity(pos) as isize;
    if idx < 0 || idx as usize >= s.len() {
        return Value::undefined();
    }
    let byte = s.as_bytes()[idx as usize];
    Value::smi(byte as i32)
}

/// String.prototype.substring(start, end) — returns substring with args clamped/sorted.
pub fn string_substring(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let len = s.len() as f64;
    let raw_start = to_integer_or_infinity(args.first().copied().unwrap_or(Value::undefined()));
    let raw_end = args.get(1).map(|&v| to_integer_or_infinity(v));
    let final_start = raw_start.max(0.0).min(len) as usize;
    let final_end = match raw_end {
        Some(e) => e.max(0.0).min(len) as usize,
        None => s.len(),
    };
    let (lo, hi) = if final_start <= final_end {
        (final_start, final_end)
    } else {
        (final_end, final_start)
    };
    let result = HeapString::allocate(gc, &s[lo..hi]);
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.substr(start, length) — legacy, negative start offset.
pub fn string_substr(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let len = s.len();
    let raw_start = to_integer_or_infinity(args.first().copied().unwrap_or(Value::undefined()));
    let int_start = if raw_start < 0.0 {
        (len as f64 + raw_start).max(0.0) as usize
    } else {
        (raw_start as usize).min(len)
    };
    let int_len = args.get(1).map(|&v| to_integer_or_infinity(v));
    let end = match int_len {
        Some(l) => {
            let clamped = l.max(0.0) as usize;
            (int_start + clamped).min(len)
        }
        None => len,
    };
    let result = HeapString::allocate(gc, &s[int_start..end]);
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.trim() — removes whitespace from both ends.
pub fn string_trim(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let result = HeapString::allocate(gc, s.trim_matches(char::is_whitespace));
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.trimStart() — removes leading whitespace.
pub fn string_trim_start(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let result = HeapString::allocate(gc, s.trim_start_matches(char::is_whitespace));
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.trimEnd() — removes trailing whitespace.
pub fn string_trim_end(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let result = HeapString::allocate(gc, s.trim_end_matches(char::is_whitespace));
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.toLowerCase() — returns lowercased string.
pub fn string_to_lower_case(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let result = HeapString::allocate(gc, &s.to_lowercase());
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.toUpperCase() — returns uppercased string.
pub fn string_to_upper_case(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let result = HeapString::allocate(gc, &s.to_uppercase());
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.repeat(count) — returns string repeated count times.
pub fn string_repeat(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let count = args.first().copied().unwrap_or(Value::undefined());
    let n = to_integer_or_infinity(count);
    if n.is_infinite() || n < 0.0 || n.is_nan() {
        let err = make_error(gc, "RangeError: Invalid count value");
        vm.set_pending_exception(err);
        return Value::undefined();
    }
    let n = n as usize;
    if s.is_empty() || n == 0 {
        let empty = HeapString::allocate(gc, "");
        return Value::from_heap_ptr(empty as *mut u8);
    }
    let mut result = String::with_capacity(s.len() * n);
    for _ in 0..n {
        result.push_str(&s);
    }
    let heap = HeapString::allocate(gc, &result);
    Value::from_heap_ptr(heap as *mut u8)
}

/// String.prototype.padStart(maxLength, fillString) — pads string to maxLength with fillString.
pub fn string_pad_start(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let max_len = args.first().copied().unwrap_or(Value::undefined());
    let target_len = to_integer_or_infinity(max_len) as usize;
    if target_len <= s.len() {
        let result = HeapString::allocate(gc, &s);
        return Value::from_heap_ptr(result as *mut u8);
    }
    let fill = match args.get(1) {
        Some(v) if !v.is_undefined() => arg_to_string(gc, Some(*v), vm),
        None | Some(_) => " ".to_string(),
    };
    let fill = if fill.is_empty() { " ".to_string() } else { fill };
    let pad_len = target_len - s.len();
    let mut pad = String::with_capacity(pad_len);
    while pad.len() < pad_len {
        pad.push_str(&fill);
    }
    pad.truncate(pad_len);
    let result_str = pad + &s;
    let result = HeapString::allocate(gc, &result_str);
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.padEnd(maxLength, fillString) — pads string to maxLength with fillString.
pub fn string_pad_end(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let max_len = args.first().copied().unwrap_or(Value::undefined());
    let target_len = to_integer_or_infinity(max_len) as usize;
    if target_len <= s.len() {
        let result = HeapString::allocate(gc, &s);
        return Value::from_heap_ptr(result as *mut u8);
    }
    let fill = match args.get(1) {
        Some(v) if !v.is_undefined() => arg_to_string(gc, Some(*v), vm),
        None | Some(_) => " ".to_string(),
    };
    let fill = if fill.is_empty() { " ".to_string() } else { fill };
    let pad_len = target_len - s.len();
    let mut pad = String::with_capacity(pad_len);
    while pad.len() < pad_len {
        pad.push_str(&fill);
    }
    pad.truncate(pad_len);
    let result_str = s + &pad;
    let result = HeapString::allocate(gc, &result_str);
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.toString() — returns the string value of the String object.
pub fn string_to_string(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let result = HeapString::allocate(gc, &s);
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.valueOf() — returns the primitive string value.
/// Uses the same logic as toString for String.prototype.
pub fn string_value_of(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let result = HeapString::allocate(gc, &s);
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.concat(...args) — concatenates strings.
pub fn string_concat(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let mut result = s;
    for &arg in args {
        result.push_str(&arg_to_string(gc, Some(arg), vm));
    }
    let heap = HeapString::allocate(gc, &result);
    Value::from_heap_ptr(heap as *mut u8)
}

/// String.prototype.split(separator, limit) — splits a string into an array of substrings.
/// Per §22.1.3.17 (simplified: string separator only, no regex).
pub fn string_split(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    fn to_u32(v: Value) -> u32 {
        if let Some(n) = v.as_smi() {
            n.max(0) as u32
        } else if let Some(f) = v.as_float64() {
            if f.is_finite() { f.max(0.0) as u32 } else { 0 }
        } else {
            0
        }
    }
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let limit = args.get(1).copied().unwrap_or(Value::undefined());
    let lim = if limit.is_undefined() { u32::MAX } else { to_u32(limit) };
    if lim == 0 {
        let arr = RuneArray::allocate(gc, &[]);
        unsafe {
            let ptr = arr as *mut u8;
            *(ptr.add(8) as *mut *const rune_core::shape::Shape) = *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
            if let Some(proto) = vm.array_prototype.heap_ptr() {
                *(ptr.add(24) as *mut *mut u8) = proto;
            }
        }
        return Value::from_heap_ptr(arr as *mut u8);
    }
    let separator = args.first().copied().unwrap_or(Value::undefined());
    if separator.is_undefined() {
        let s_val = Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8);
        let arr = RuneArray::allocate(gc, &[]);
        unsafe {
            let ptr = arr as *mut u8;
            *(ptr.add(8) as *mut *const rune_core::shape::Shape) = *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
            if let Some(proto) = vm.array_prototype.heap_ptr() {
                *(ptr.add(24) as *mut *mut u8) = proto;
            }
            let result_ptr = RuneArray::push(gc, arr, s_val);
            Value::from_heap_ptr(result_ptr as *mut u8)
        }
    } else {
        let sep = arg_to_string(gc, Some(separator), vm);
        let pieces: Vec<String> = if sep.is_empty() {
            s.chars().map(|c| c.to_string()).collect()
        } else {
            s.split(&sep).map(|p| p.to_string()).collect()
        };
        let elem_count = (pieces.len() as u32).min(lim) as usize;
        let arr = RuneArray::allocate(gc, &[]);
        unsafe {
            let mut arr_ptr = arr as *mut u8;
            *(arr_ptr.add(8) as *mut *const rune_core::shape::Shape) = *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
            if let Some(proto) = vm.array_prototype.heap_ptr() {
                *(arr_ptr.add(24) as *mut *mut u8) = proto;
            }
            for p in pieces.iter().take(elem_count) {
                let heap_str = HeapString::allocate(gc, p);
                let new_ptr = RuneArray::push(gc, arr_ptr as *mut RuneArray, Value::from_heap_ptr(heap_str as *mut u8));
                if new_ptr as *mut u8 != arr_ptr {
                    arr_ptr = new_ptr as *mut u8;
                }
            }
            Value::from_heap_ptr(arr_ptr)
        }
    }
}

/// String.prototype.replace(searchValue, replaceValue) — first match only (string pattern, no regex).
pub fn string_replace(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    let replacement = if args.len() > 1 {
        arg_to_string(gc, Some(args[1]), vm)
    } else {
        HeapString::allocate(gc, "undefined");
        "undefined".to_string()
    };
    if search_str.is_empty() {
        let result = replacement.clone() + &s;
        return Value::from_heap_ptr(HeapString::allocate(gc, &result) as *mut u8);
    }
    if let Some(pos) = s.find(&search_str) {
        let result = s[..pos].to_string() + &replacement + &s[pos + search_str.len()..];
        Value::from_heap_ptr(HeapString::allocate(gc, &result) as *mut u8)
    } else {
        Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8)
    }
}

/// String.prototype.replaceAll(searchValue, replaceValue) — replace all non-overlapping matches (string pattern, no regex).
pub fn string_replace_all(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    let replacement = if args.len() > 1 {
        arg_to_string(gc, Some(args[1]), vm)
    } else {
        "undefined".to_string()
    };
    if search_str.is_empty() {
        let result = s.chars().map(|c| replacement.clone() + &c.to_string()).collect::<String>() + &replacement;
        return Value::from_heap_ptr(HeapString::allocate(gc, &result) as *mut u8);
    }
    let result = s.replace(&search_str, &replacement);
    Value::from_heap_ptr(HeapString::allocate(gc, &result) as *mut u8)
}

/// Math.floor(x) — rounds down.
fn math_op_unary(args: &[Value], op: fn(f64) -> f64) -> Value {
    let x = args.first().copied().unwrap_or(Value::smi(0));
    let n = x
        .as_smi()
        .map(|v| v as f64)
        .or_else(|| x.as_float64())
        .unwrap_or(f64::NAN);
    let result = op(n);
    if result.fract() == 0.0 && result.is_finite() {
        let i = result as i32;
        if i as f64 == result {
            return Value::smi(i);
        }
    }
    Value::from_float64(result)
}

fn math_op_binary(args: &[Value], op: fn(f64, f64) -> f64) -> Value {
    let a = args.first().copied().unwrap_or(Value::smi(0));
    let b = args.get(1).copied().unwrap_or(Value::smi(0));
    let na = a
        .as_smi()
        .map(|v| v as f64)
        .or_else(|| a.as_float64())
        .unwrap_or(f64::NAN);
    let nb = b
        .as_smi()
        .map(|v| v as f64)
        .or_else(|| b.as_float64())
        .unwrap_or(f64::NAN);
    let result = op(na, nb);
    if result.fract() == 0.0 && result.is_finite() {
        let i = result as i32;
        if i as f64 == result {
            return Value::smi(i);
        }
    }
    Value::from_float64(result)
}

pub fn math_floor(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    math_op_unary(args, f64::floor)
}

pub fn math_ceil(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    math_op_unary(args, f64::ceil)
}

pub fn math_abs(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    math_op_unary(args, f64::abs)
}

pub fn math_min(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let mut min = f64::INFINITY;
    for arg in args {
        let n = arg
            .as_smi()
            .map(|v| v as f64)
            .or_else(|| arg.as_float64())
            .unwrap_or(f64::NAN);
        if n < min {
            min = n;
        }
    }
    if min.fract() == 0.0 && min.is_finite() {
        let i = min as i32;
        if i as f64 == min {
            return Value::smi(i);
        }
    }
    Value::from_float64(min)
}

pub fn math_max(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let mut max = f64::NEG_INFINITY;
    for arg in args {
        let n = arg
            .as_smi()
            .map(|v| v as f64)
            .or_else(|| arg.as_float64())
            .unwrap_or(f64::NAN);
        if n > max {
            max = n;
        }
    }
    if max.fract() == 0.0 && max.is_finite() {
        let i = max as i32;
        if i as f64 == max {
            return Value::smi(i);
        }
    }
    Value::from_float64(max)
}

pub fn math_pow(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    math_op_binary(args, |a, b| a.powf(b))
}

pub fn math_sqrt(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    math_op_unary(args, f64::sqrt)
}

/// parseInt(string, radix) — parses a string argument and returns an integer.
/// Per §21.1.2.9.
pub fn parse_int_builtin(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let s = match args.first() {
        Some(v) => value_to_js_string(*v).trim().to_string(),
        None => return Value::from_float64(f64::NAN),
    };
    if s.is_empty() {
        return Value::from_float64(f64::NAN);
    }
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut sign = 1.0;
    if chars[i] == '-' { sign = -1.0; i += 1; }
    else if chars[i] == '+' { i += 1; }
    if i >= chars.len() {
        return Value::from_float64(f64::NAN);
    }
    // Determine radix
    let radix = if args.len() > 1 {
        let r = args[1];
        if r.is_undefined() { 0 } else {
            r.as_smi()
                .or_else(|| r.as_float64().map(|f| f as i32))
                .unwrap_or(0)
        }
    } else { 0 };
    let radix = if radix == 0 {
        if i + 2 <= chars.len() && chars[i] == '0' && (chars[i+1] == 'x' || chars[i+1] == 'X') {
            16
        } else {
            10
        }
    } else { radix };
    if !(2..=36).contains(&radix) {
        return Value::from_float64(f64::NAN);
    }
    if radix == 16 && i + 2 <= chars.len() && chars[i] == '0' && (chars[i+1] == 'x' || chars[i+1] == 'X') {
        i += 2;
    }
    let mut result = 0.0;
    let mut any_digit = false;
    while i < chars.len() {
        let d = match chars[i] {
            '0'..='9' => chars[i] as i32 - '0' as i32,
            'a'..='z' => chars[i] as i32 - 'a' as i32 + 10,
            'A'..='Z' => chars[i] as i32 - 'A' as i32 + 10,
            _ => break,
        };
        if d >= radix { break; }
        result = result * (radix as f64) + d as f64;
        any_digit = true;
        i += 1;
    }
    if !any_digit {
        return Value::from_float64(f64::NAN);
    }
    let result = sign * result;
    if result.fract() == 0.0 && result.is_finite() {
        let i = result as i32;
        if i as f64 == result && (-(1 << 30)..(1 << 30)).contains(&i) {
            return Value::smi(i);
        }
    }
    Value::from_float64(result)
}

/// parseFloat(string) — parses a string argument and returns a floating point number.
/// Per §21.1.2.10.
pub fn parse_float_builtin(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let s = match args.first() {
        Some(v) => value_to_js_string(*v).trim().to_string(),
        None => return Value::from_float64(f64::NAN),
    };
    if s.is_empty() {
        return Value::from_float64(f64::NAN);
    }
    // Parse the longest prefix that is a valid StrDecimalLiteral
    // We use Rust's f64::parse which handles Infinity, NaN, and regular floats
    // But we need to match JS semantics: leading whitespace already trimmed,
    // accept optional sign, then parse number.
    let chars: Vec<char> = s.chars().collect();
    let mut end = 0;
    let mut has_dot = false;
    let mut has_digit = false;
    let mut has_exp = false;
    // Skip sign
    if end < chars.len() && (chars[end] == '-' || chars[end] == '+') {
        end += 1;
    }
    // Check for Infinity
    if s[end..].starts_with("Infinity") || s[end..].starts_with("infinity") {
        let prefix = &s[end..end+8];
        if prefix == "Infinity" {
            return Value::from_float64(f64::INFINITY);
        }
    }
    // Check for NaN (case-insensitive)
    if end + 3 <= chars.len() {
        let na: String = chars[end..end+3].iter().collect();
        if na.eq_ignore_ascii_case("nan") {
            return Value::from_float64(f64::NAN);
        }
    }
    // Parse number
    while end < chars.len() {
        let c = chars[end];
        if c.is_ascii_digit() {
            has_digit = true;
            end += 1;
        } else if c == '.' && !has_dot && !has_exp {
            has_dot = true;
            end += 1;
        } else if (c == 'e' || c == 'E') && has_digit && !has_exp {
            has_exp = true;
            end += 1;
            // Optional sign after exponent
            if end < chars.len() && (chars[end] == '-' || chars[end] == '+') {
                end += 1;
            }
        } else {
            break;
        }
    }
    if !has_digit {
        return Value::from_float64(f64::NAN);
    }
    let sub: String = chars[..end].iter().collect();
    match sub.parse::<f64>() {
        Ok(n) => {
            if n.fract() == 0.0 && n.is_finite() {
                let i = n as i32;
                if i as f64 == n {
                    return Value::smi(i);
                }
            }
            Value::from_float64(n)
        }
        Err(_) => Value::from_float64(f64::NAN),
    }
}

/// JSON.parse(text) — parse a JSON string into Rune values.
pub fn json_parse(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let text = args.first().copied().unwrap_or(Value::undefined());
    let s = value_to_js_string(text);
    let chars = s.chars().collect::<Vec<char>>();
    let mut pos = 0;
    fn skip_ws(chars: &[char], pos: &mut usize) {
        while *pos < chars.len() && chars[*pos].is_ascii_whitespace() {
            *pos += 1;
        }
    }
    let array_proto = vm.array_prototype.heap_ptr();
    let object_proto = vm.object_prototype.heap_ptr();
    fn parse_value(
        gc: &mut SemiSpace,
        chars: &[char],
        pos: &mut usize,
        array_proto: Option<*mut u8>,
        object_proto: Option<*mut u8>,
    ) -> Option<Value> {
        use rune_core::shape::DENSE_ARRAY_SHAPE;
        skip_ws(chars, pos);
        if *pos >= chars.len() {
            return None;
        }
        match chars[*pos] {
            'n' => {
                if chars[*pos..].starts_with(&['n', 'u', 'l', 'l']) {
                    *pos += 4;
                    Some(Value::null())
                } else {
                    None
                }
            }
            't' => {
                if chars[*pos..].starts_with(&['t', 'r', 'u', 'e']) {
                    *pos += 4;
                    Some(Value::boolean(true))
                } else {
                    None
                }
            }
            'f' => {
                if chars[*pos..].starts_with(&['f', 'a', 'l', 's', 'e']) {
                    *pos += 5;
                    Some(Value::boolean(false))
                } else {
                    None
                }
            }
            '"' => {
                *pos += 1; // skip opening quote
                let mut s = String::new();
                while *pos < chars.len() && chars[*pos] != '"' {
                    if chars[*pos] == '\\' {
                        *pos += 1;
                        if *pos >= chars.len() {
                            return None;
                        }
                        match chars[*pos] {
                            '"' => s.push('"'),
                            '\\' => s.push('\\'),
                            '/' => s.push('/'),
                            'b' => s.push('\u{0008}'),
                            'f' => s.push('\u{000C}'),
                            'n' => s.push('\n'),
                            'r' => s.push('\r'),
                            't' => s.push('\t'),
                            'u' => {
                                if *pos + 4 < chars.len() {
                                    let hex: String = chars[*pos + 1..*pos + 5].iter().collect();
                                    if let Ok(code) = u32::from_str_radix(&hex, 16)
                                        && let Some(ch) = char::from_u32(code) {
                                            s.push(ch);
                                    }
                                    *pos += 4;
                                } else {
                                    return None;
                                }
                            }
                            _ => return None,
                        }
                    } else {
                        s.push(chars[*pos]);
                    }
                    *pos += 1;
                }
                if *pos >= chars.len() {
                    return None;
                }
                *pos += 1; // skip closing quote
                let ptr = HeapString::allocate(gc, &s);
                Some(Value::from_heap_ptr(ptr as *mut u8))
            }
            '-' | '0'..='9' => {
                let num_start = *pos;
                if chars[*pos] == '-' {
                    *pos += 1;
                }
                while *pos < chars.len() && chars[*pos].is_ascii_digit() {
                    *pos += 1;
                }
                if *pos < chars.len() && chars[*pos] == '.' {
                    *pos += 1;
                    while *pos < chars.len() && chars[*pos].is_ascii_digit() {
                        *pos += 1;
                    }
                }
                if *pos < chars.len() && (chars[*pos] == 'e' || chars[*pos] == 'E') {
                    *pos += 1;
                    if *pos < chars.len() && (chars[*pos] == '+' || chars[*pos] == '-') {
                        *pos += 1;
                    }
                    while *pos < chars.len() && chars[*pos].is_ascii_digit() {
                        *pos += 1;
                    }
                }
                let num_str: String = chars[num_start..*pos].iter().collect();
                if let Ok(n) = num_str.parse::<i32>() {
                    Some(Value::smi(n))
                } else if let Ok(f) = num_str.parse::<f64>() {
                    Some(Value::from_float64(f))
                } else {
                    None
                }
            }
            '[' => {
                *pos += 1;
                skip_ws(chars, pos);
                let mut elements: Vec<Value> = Vec::new();
                if *pos < chars.len() && chars[*pos] != ']' {
                    loop {
                        skip_ws(chars, pos);
                        if let Some(val) = parse_value(gc, chars, pos, array_proto, object_proto) {
                            elements.push(val);
                        } else {
                            return None;
                        }
                        skip_ws(chars, pos);
                        if *pos < chars.len() && chars[*pos] == ',' {
                            *pos += 1;
                        } else {
                            break;
                        }
                    }
                }
                skip_ws(chars, pos);
                if *pos >= chars.len() || chars[*pos] != ']' {
                    return None;
                }
                *pos += 1;
                let arr_ptr = RuneArray::allocate(gc, &elements);
                unsafe {
                    let ptr = arr_ptr as *mut u8;
                    let shape_ptr = ptr.add(8) as *mut *const rune_core::shape::Shape;
                    *shape_ptr = *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
                    if let Some(proto) = array_proto {
                        let proto_ptr = ptr.add(24) as *mut *mut u8;
                        *proto_ptr = proto;
                    }
                }
                Some(Value::from_heap_ptr(arr_ptr as *mut u8))
            }
            '{' => {
                *pos += 1;
                skip_ws(chars, pos);
                let mut keys: Vec<String> = Vec::new();
                let mut values: Vec<Value> = Vec::new();
                if *pos < chars.len() && chars[*pos] != '}' {
                    loop {
                        skip_ws(chars, pos);
                        if *pos >= chars.len() || chars[*pos] != '"' {
                            return None;
                        }
                        // Parse string key
                        *pos += 1;
                        let mut key = String::new();
                        while *pos < chars.len() && chars[*pos] != '"' {
                            if chars[*pos] == '\\' {
                                *pos += 1;
                                if *pos >= chars.len() {
                                    return None;
                                }
                                match chars[*pos] {
                                    '"' => key.push('"'),
                                    '\\' => key.push('\\'),
                                    '/' => key.push('/'),
                                    'b' => key.push('\u{0008}'),
                                    'f' => key.push('\u{000C}'),
                                    'n' => key.push('\n'),
                                    'r' => key.push('\r'),
                                    't' => key.push('\t'),
                                    'u' => {
                                        if *pos + 4 < chars.len() {
                                            let hex: String =
                                                chars[*pos + 1..*pos + 5].iter().collect();
                                            if let Ok(code) = u32::from_str_radix(&hex, 16)
                                                && let Some(ch) = char::from_u32(code) {
                                                    key.push(ch);
                                            }
                                            *pos += 4;
                                        } else {
                                            return None;
                                        }
                                    }
                                    _ => return None,
                                }
                            } else {
                                key.push(chars[*pos]);
                            }
                            *pos += 1;
                        }
                        if *pos >= chars.len() {
                            return None;
                        }
                        *pos += 1; // skip closing quote
                        skip_ws(chars, pos);
                        if *pos >= chars.len() || chars[*pos] != ':' {
                            return None;
                        }
                        *pos += 1;
                        skip_ws(chars, pos);
                        if let Some(val) = parse_value(gc, chars, pos, array_proto, object_proto) {
                            keys.push(key);
                            values.push(val);
                        } else {
                            return None;
                        }
                        skip_ws(chars, pos);
                        if *pos < chars.len() && chars[*pos] == ',' {
                            *pos += 1;
                        } else {
                            break;
                        }
                    }
                }
                skip_ws(chars, pos);
                if *pos >= chars.len() || chars[*pos] != '}' {
                    return None;
                }
                *pos += 1;
                // Build object with string-keyed properties
                let shape_entries: Vec<(PropertyKey, usize)> = keys
                    .iter()
                    .enumerate()
                    .map(|(i, k)| (PropertyKey::from_string(k), i))
                    .collect();
                let key_names: Vec<String> = keys.to_vec();
                let shape = Shape::intern(shape_entries, key_names);
                let obj_ptr = JSObject::allocate(gc, shape, &values);
                // Set prototype
                if let Some(proto) = object_proto {
                    unsafe {
                        JSObject::set_prototype(obj_ptr, proto);
                    }
                }
                Some(Value::from_heap_ptr(obj_ptr as *mut u8))
            }
            _ => None,
        }
    }
    parse_value(gc, &chars, &mut pos, array_proto, object_proto).unwrap_or_else(|| {
        let msg_ptr = HeapString::allocate(gc, "JSON.parse: unexpected end of JSON input");
        let err = make_simple_object(gc, "message", Value::from_heap_ptr(msg_ptr as *mut u8));
        vm.set_pending_exception(err);
        Value::undefined()
    })
}

/// JSON.stringify(value) — serialize a JS value to a JSON string.
pub fn json_stringify(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    fn escape_json(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        for ch in s.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\x08' => out.push_str("\\b"),
                '\x0C' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c.is_control() => {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                }
                c => out.push(c),
            }
        }
        out
    }
    fn stringify_val(gc: &mut SemiSpace, val: Value, stack: &mut Vec<*mut u8>, vm: &mut Vm) -> Result<String, ()> {
        if val.is_undefined() {
            return Err(());
        }
        if val.is_null() {
            return Ok("null".to_string());
        }
        if val.is_boolean() {
            return Ok(if val.to_boolean().unwrap() { "true" } else { "false" }.to_string());
        }
        if let Some(n) = val.as_smi() {
            return Ok(n.to_string());
        }
        if val.is_float64() {
            let f = val.as_float64().unwrap_or(f64::NAN);
            if f.is_nan() || f.is_infinite() {
                return Ok("null".to_string());
            }
            return Ok(f64_to_json_string(f));
        }
        if let Some(ptr) = val.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_STRING {
                let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
                return Ok(format!("\"{}\"", escape_json(&s)));
            }
            if tag == TAG_ARRAY {
                if stack.contains(&ptr) {
                    let err = make_error(gc, "TypeError: Converting circular structure to JSON");
                    vm.set_pending_exception(err);
                    return Err(());
                }
                stack.push(ptr);
                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) } as usize;
                let mut parts: Vec<String> = Vec::with_capacity(len);
                for i in 0..len {
                    let elem = unsafe { RuneArray::get_element(ptr as *mut RuneArray, i) };
                    parts.push(stringify_val(gc, elem, stack, vm).unwrap_or_else(|_| "null".to_string()));
                }
                stack.pop();
                return Ok(format!("[{}]", parts.join(",")));
            }
            if tag == TAG_OBJECT {
                if stack.contains(&ptr) {
                    let msg = HeapString::allocate(gc, "TypeError: Converting circular structure to JSON");
                    vm.set_pending_exception(Value::from_heap_ptr(msg as *mut u8));
                    return Err(());
                }
                stack.push(ptr);
                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                let count = unsafe { JSObject::slot_count(ptr as *mut JSObject) };
                let mut pairs: Vec<String> = Vec::new();
                for i in 0..count {
                    let key_name = shape.key_name_at(i).unwrap_or("");
                    let val = unsafe { JSObject::get_slot(ptr as *mut JSObject, i) };
                    if val.is_undefined() {
                        continue;
                    }
                    if let Ok(s) = stringify_val(gc, val, stack, vm) {
                        pairs.push(format!("\"{}\":{}", escape_json(key_name), s));
                    }
                }
                stack.pop();
                return Ok(format!("{{{}}}", pairs.join(",")));
            }
        }
        Ok("null".to_string())
    }
    let val = args.first().copied().unwrap_or(Value::undefined());
    let mut stack: Vec<*mut u8> = Vec::new();
    match stringify_val(gc, val, &mut stack, vm) {
        Ok(s) => {
            let heap_s = HeapString::allocate(gc, &s);
            Value::from_heap_ptr(heap_s as *mut u8)
        }
        Err(()) => Value::undefined(),
    }
}

/// Convert f64 to shortest-reasonable JSON string representation.
/// Known limitation: does not guarantee shortest round-trippable (Rust's `f64::to_string()`
/// differs from JS's Number.prototype.toString() for some high-precision values).
fn f64_to_json_string(f: f64) -> String {
    f64::to_string(&f)
}

/// Function.prototype.call(thisArg, ...args) — calls `this` with the given thisArg and arguments.
/// `this` is the function to call, args[0] is the new this value, args[1..] are call arguments.
pub fn call_builtin(_gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let target = this;
    let new_this = args.first().copied().unwrap_or(Value::undefined());
    let call_args: Vec<Value> = args.iter().skip(1).copied().collect();

    // If target is a builtin, call it directly.
    // If it sets up pending_array_op (like array methods), that works naturally.
    if let Some(smi) = target.as_smi()
        && smi < 0 {
            let id = ((-smi) as usize) - 1;
            if id < vm.builtins.len() {
                return (vm.builtins[id].func)(_gc, new_this, &call_args, vm);
            }
        }
    // If target is a JS function, use the pending callback pattern.
    if let Some(ptr) = target.heap_ptr() {
        let tag = unsafe { (*(ptr as *const rune_core::gc::GcHeader)).tag() };
        if tag == rune_core::gc::TAG_FUNC {
            vm.pending_call = Some(crate::vm::PendingCall {
                source_frame_depth: 0,
            });
            vm.push_callback_call(_gc, target, new_this, call_args);
            return Value::undefined();
        }
    }
    Value::undefined()
}

/// Array.prototype.slice(start, end) — returns a new dense array with elements from [start, end).
pub fn array_slice(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::undefined(),
    };
    let relative_start = args.first().and_then(|v| v.as_smi()).unwrap_or(0) as i64;
    let k = if relative_start < 0 {
        (length as i64 + relative_start).max(0) as u32
    } else {
        (relative_start as u32).min(length)
    };
    let final_idx = if args.len() > 1 {
        if let Some(relative_end) = args.get(1).and_then(|v| v.as_smi()) {
            let re = relative_end as i64;
            if re < 0 {
                ((length as i64 + re).max(0) as u32).min(length)
            } else {
                (re as u32).min(length)
            }
        } else {
            length
        }
    } else {
        length
    };
    let count = final_idx.saturating_sub(k) as usize;
    let result_arr = RuneArray::allocate(gc, &[]);
    unsafe {
        let ptr = result_arr as *mut u8;
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) = *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(ptr.add(24) as *mut *mut u8) = proto;
        }
    }
    let mut result_ptr = result_arr as *mut u8;
    for i in 0..count {
        let element = crate::vm::array_like_index(this, k + i as u32).unwrap_or(Value::undefined());
        unsafe {
            let new_ptr = RuneArray::push(gc, result_ptr as *mut RuneArray, element);
            if new_ptr as *mut u8 != result_ptr {
                result_ptr = new_ptr as *mut u8;
            }
        }
    }
    Value::from_heap_ptr(result_ptr)
}

/// Convert a Value to an integer for use as fromIndex in array methods.
/// Approximates ToInteger (omits valueOf/getter callbacks for objects).
fn to_index(v: Value, length: u32) -> u32 {
    if v.is_undefined() || v.is_null() {
        return 0;
    }
    if let Some(b) = v.to_boolean() {
        let n: i32 = if b { 1 } else { 0 };
        return if n < 0 { length.saturating_sub(n.unsigned_abs()) } else { (n as u32).min(length) };
    }
    if let Some(smi) = v.as_smi() {
        if smi < 0 {
            let tmp = length as i64 + smi as i64;
            if tmp < 0 { 0 } else { tmp as u32 }
        } else {
            smi as u32
        }
    } else if let Some(f) = v.as_float64() {
        if f.is_nan() || f < 0.0 {
            let tmp = length as f64 + f;
            if tmp < 0.0 { 0 } else { tmp as u32 }
        } else {
            (f as u32).min(length)
        }
    } else if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING || tag == TAG_STRING_OBJ {
            let s = if tag == TAG_STRING {
                unsafe { HeapString::to_string(ptr as *mut HeapString) }
            } else {
                let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
                unsafe { HeapString::to_string(str_ptr as *mut HeapString) }
            };
            let n: f64 = s.parse().unwrap_or(0.0);
            if n.is_nan() || n < 0.0 { 0 } else { (n as u32).min(length) }
        } else {
            0
        }
    } else {
        0
    }
}

/// Array.prototype.indexOf(searchElement, fromIndex) — returns index of first match, -1 if not found.
pub fn array_index_of(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) { return Value::undefined(); }
    let search = args.first().copied().unwrap_or(Value::undefined());
    let len = crate::vm::array_like_length(this).unwrap_or(0) as usize;
    let from = to_index(args.get(1).copied().unwrap_or(Value::smi(0)), len as u32) as usize;
    if from >= len { return Value::smi(-1); }
    for i in from..len {
        if let Some(elem) = crate::vm::array_like_index(this, i as u32) {
            #[allow(unused_assignments)]
            let mut eq = false;
            if elem.is_smi() && search.is_smi() {
                eq = elem.as_smi() == search.as_smi();
            } else if let (Some(ep), Some(sp)) = (elem.heap_ptr(), search.heap_ptr()) {
                let et = unsafe { (*(ep as *const GcHeader)).tag() };
                let st = unsafe { (*(sp as *const GcHeader)).tag() };
                if et == TAG_STRING && st == TAG_STRING {
                    let es = unsafe { HeapString::to_string(ep as *mut HeapString) };
                    let ss = unsafe { HeapString::to_string(sp as *mut HeapString) };
                    eq = es == ss;
                } else {
                    eq = ep == sp;
                }
            } else if let (Some(ef), Some(sf)) = (elem.as_float64(), search.as_float64()) {
                eq = ef.to_bits() == sf.to_bits();
            } else {
                eq = (elem.is_undefined() && search.is_undefined())
                    || (elem.is_null() && search.is_null())
                    || (elem.is_boolean() && search.is_boolean() && elem.as_smi() == search.as_smi());
            }
            if eq { return Value::smi(i as i32); }
        }
    }
    Value::smi(-1)
}

/// Array.prototype.includes(searchElement, fromIndex) — SameValueZero search.
pub fn array_includes(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::boolean(false),
    };
    let search = args.first().copied().unwrap_or(Value::undefined());
    let from_idx = args.get(1).copied().unwrap_or(Value::undefined());

    let k = to_index(from_idx, length);
    if k >= length {
        return Value::boolean(false);
    }

    for i in k..length {
        let element = crate::vm::array_like_index(this, i).unwrap_or(Value::undefined());
        if same_value_zero(element, search) {
            return Value::boolean(true);
        }
    }
    Value::boolean(false)
}

/// Array.prototype.forEach(callback, thisArg) — same state machine, no result array.
pub fn array_for_each(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::undefined(),
    };
    let callback = args.first().copied().unwrap_or(Value::undefined());
    let this_arg = args.get(1).copied().unwrap_or(Value::undefined());
    let source_ptr = this.heap_ptr().unwrap();
    if length == 0 {
        return Value::undefined();
    }
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::ForEach,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: 0,
        length,
        source_frame_depth: 0,
        accumulator: None,
    });
    let element = crate::vm::array_like_index(this, 0).unwrap_or(Value::undefined());
    vm.push_callback_call(gc, callback, this_arg, vec![element, Value::smi(0), this]);
    Value::undefined()
}

/// Array.prototype.filter(callback, thisArg) — set up state machine iteration.
pub fn array_filter(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::undefined(),
    };
    let callback = args.first().copied().unwrap_or(Value::undefined());
    let this_arg = args.get(1).copied().unwrap_or(Value::undefined());
    let source_ptr = this.heap_ptr().unwrap();
    let result_arr = RuneArray::allocate(gc, &[]);
    unsafe {
        let ptr = result_arr as *mut u8;
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) = *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(ptr.add(24) as *mut *mut u8) = proto;
        }
    }
    if length == 0 {
        return Value::from_heap_ptr(result_arr as *mut u8);
    }
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Filter,
        source: source_ptr,
        result: result_arr as *mut u8,
        callback,
        this_val: this_arg,
        source_val: this,
        index: 0,
        length,
        source_frame_depth: 0,
        accumulator: None,
    });
    let element = crate::vm::array_like_index(this, 0).unwrap_or(Value::undefined());
    vm.push_callback_call(gc, callback, this_arg, vec![element, Value::smi(0), this]);
    Value::undefined()
}

/// Array.prototype.map(callback, thisArg) — set up state machine iteration.
pub fn array_map(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::undefined(),
    };
    let callback = args.first().copied().unwrap_or(Value::undefined());
    let this_arg = args.get(1).copied().unwrap_or(Value::undefined());
    let source_ptr = this.heap_ptr().unwrap();
    let result_arr = RuneArray::allocate(gc, &[]);
    unsafe {
        let ptr = result_arr as *mut u8;
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) = *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(ptr.add(24) as *mut *mut u8) = proto;
        }
    }
    if length == 0 {
        return Value::from_heap_ptr(result_arr as *mut u8);
    }
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Map,
        source: source_ptr,
        result: result_arr as *mut u8,
        callback,
        this_val: this_arg,
        source_val: this,
        index: 0,
        length,
        source_frame_depth: 0,
        accumulator: None,
    });
    let element = crate::vm::array_like_index(this, 0).unwrap_or(Value::undefined());
    vm.push_callback_call(gc, callback, this_arg, vec![element, Value::smi(0), this]);
    Value::undefined()
}

/// Array.prototype.reduce(callback, initialValue) — set up state machine iteration.
pub fn array_reduce(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::undefined(),
    };
    let callback = args.first().copied().unwrap_or(Value::undefined());
    let has_initial = args.len() > 1;
    let initial = args.get(1).copied().unwrap_or(Value::undefined());
    if !has_initial && length == 0 {
        let msg = HeapString::allocate(gc, "TypeError: reduce of empty array with no initial value");
        vm.set_pending_exception(Value::from_heap_ptr(msg as *mut u8));
        return Value::undefined();
    }
    let start_index;
    let accumulator = if has_initial {
        start_index = 0;
        initial
    } else {
        start_index = 1;
        crate::vm::array_like_index(this, 0).unwrap_or(Value::undefined())
    };
    if start_index >= length as usize {
        return accumulator;
    }
    let source_ptr = this.heap_ptr().unwrap();
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Reduce,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: Value::undefined(),
        source_val: this,
        index: start_index,
        length,
        source_frame_depth: 0,
        accumulator: Some(accumulator),
    });
    let element = crate::vm::array_like_index(this, start_index as u32).unwrap_or(Value::undefined());
    vm.push_callback_call(gc, callback, Value::undefined(), vec![accumulator, element, Value::smi(start_index as i32), this]);
    Value::undefined()
}

/// Array.prototype.find(callback, thisArg) — set up state machine iteration.
pub fn array_find(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::undefined(),
    };
    let callback = args.first().copied().unwrap_or(Value::undefined());
    let this_arg = args.get(1).copied().unwrap_or(Value::undefined());
    let source_ptr = this.heap_ptr().unwrap();
    if length == 0 {
        return Value::undefined();
    }
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Find,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: 0,
        length,
        source_frame_depth: 0,
        accumulator: None,
    });
    let element = crate::vm::array_like_index(this, 0).unwrap_or(Value::undefined());
    vm.push_callback_call(gc, callback, this_arg, vec![element, Value::smi(0), this]);
    Value::undefined()
}

/// Array.prototype.findIndex(callback, thisArg) — set up state machine iteration.
pub fn array_find_index(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::smi(-1),
    };
    let callback = args.first().copied().unwrap_or(Value::undefined());
    let this_arg = args.get(1).copied().unwrap_or(Value::undefined());
    let source_ptr = this.heap_ptr().unwrap();
    if length == 0 {
        return Value::smi(-1);
    }
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::FindIndex,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: 0,
        length,
        source_frame_depth: 0,
        accumulator: None,
    });
    let element = crate::vm::array_like_index(this, 0).unwrap_or(Value::undefined());
    vm.push_callback_call(gc, callback, this_arg, vec![element, Value::smi(0), this]);
    Value::undefined()
}

/// Check if a Value is an Array (TAG_ARRAY).
fn is_array_val(v: Value) -> bool {
    if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        return tag == TAG_ARRAY;
    }
    false
}

/// Array.prototype.flat(depth) — flatten nested arrays to specified depth.
pub fn array_flat(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let depth = args.first().copied().unwrap_or(Value::undefined());
    let depth_num = if depth.is_undefined() {
        1.0
    } else if let Some(smi) = depth.as_smi() {
        smi as f64
    } else if let Some(f) = depth.as_float64() {
        f
    } else {
        to_integer_or_infinity(depth)
    };
    let effective_depth = if depth_num.is_infinite() || depth_num.is_nan() {
        if depth_num.is_sign_negative() { 0 } else { u32::MAX }
    } else {
        depth_num.max(0.0) as u32
    };
    fn flatten(gc: &mut SemiSpace, vm: &Vm, arr_val: Value, depth: u32) -> *mut u8 {
        let result_arr = RuneArray::allocate(gc, &[]);
        let mut result_ptr = result_arr as *mut u8;
        unsafe {
            *(result_ptr.add(8) as *mut *const rune_core::shape::Shape) = *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
            if let Some(proto) = vm.array_prototype.heap_ptr() {
                *(result_ptr.add(24) as *mut *mut u8) = proto;
            }
        }
        let src_len = crate::vm::array_like_length(arr_val).unwrap_or(0);
        for i in 0..src_len {
            let elem = crate::vm::array_like_index(arr_val, i).unwrap_or(Value::undefined());
            if depth > 0 && is_array_val(elem) {
                let flattened = flatten(gc, vm, elem, depth - 1);
                unsafe {
                    let flat_len = RuneArray::length(flattened as *mut RuneArray);
                    for j in 0..flat_len {
                        let flat_elem = RuneArray::get_element(flattened as *mut RuneArray, j as usize);
                        let new_ptr = RuneArray::push(gc, result_ptr as *mut RuneArray, flat_elem);
                        result_ptr = new_ptr as *mut u8;
                    }
                }
            } else {
                unsafe {
                    let new_ptr = RuneArray::push(gc, result_ptr as *mut RuneArray, elem);
                    result_ptr = new_ptr as *mut u8;
                }
            }
        }
        result_ptr
    }
    let result_ptr = flatten(gc, vm, this, effective_depth);
    Value::from_heap_ptr(result_ptr)
}

/// Array.prototype.sort(compareFn) — default lexicographic sort (no comparator). Throws TypeError if comparator is passed.
pub fn array_sort(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if args.first().filter(|c| !c.is_undefined()).is_some() {
        let msg = HeapString::allocate(gc, "TypeError: comparator sort is not yet supported");
        vm.set_pending_exception(Value::from_heap_ptr(msg as *mut u8));
        return Value::undefined();
    }
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return this,
    };
    if length <= 1 {
        return this;
    }
    let mut elements: Vec<Value> = Vec::with_capacity(length as usize);
    for i in 0..length {
        elements.push(crate::vm::array_like_index(this, i).unwrap_or(Value::undefined()));
    }
    elements.sort_by_key(|a| string_from_value(*a));
    // Write back sorted elements in-place
    if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_ARRAY {
            unsafe {
                RuneArray::set_length(ptr as *mut RuneArray, 0);
            }
            let mut cur_ptr = ptr;
            for elem in &elements {
                unsafe {
                    let new_ptr = RuneArray::push(gc, cur_ptr as *mut RuneArray, *elem);
                    if new_ptr as *mut u8 != cur_ptr {
                        let resolved = if (*(cur_ptr as *const GcHeader)).is_forwarded() {
                            (*(cur_ptr as *const GcHeader)).forwarding_addr()
                        } else {
                            cur_ptr
                        };
                        if resolved != new_ptr as *mut u8 {
                            vm.update_heap_reference(resolved, new_ptr as *mut u8);
                        }
                        cur_ptr = new_ptr as *mut u8;
                    }
                }
            }
            return Value::from_heap_ptr(cur_ptr);
        }
    }
    this
}

/// Array.prototype.flatMap(callback, thisArg) — set up state machine iteration, spreading array results.
pub fn array_flat_map(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::undefined(),
    };
    let callback = args.first().copied().unwrap_or(Value::undefined());
    let this_arg = args.get(1).copied().unwrap_or(Value::undefined());
    let source_ptr = this.heap_ptr().unwrap();
    let result_arr = RuneArray::allocate(gc, &[]);
    unsafe {
        let ptr = result_arr as *mut u8;
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) = *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(ptr.add(24) as *mut *mut u8) = proto;
        }
    }
    if length == 0 {
        return Value::from_heap_ptr(result_arr as *mut u8);
    }
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::FlatMap,
        source: source_ptr,
        result: result_arr as *mut u8,
        callback,
        this_val: this_arg,
        source_val: this,
        index: 0,
        length,
        source_frame_depth: 0,
        accumulator: None,
    });
    let element = crate::vm::array_like_index(this, 0).unwrap_or(Value::undefined());
    vm.push_callback_call(gc, callback, this_arg, vec![element, Value::smi(0), this]);
    Value::undefined()
}

/// Array.prototype.some(callback, thisArg) — set up state machine iteration.
pub fn array_some(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::boolean(false),
    };
    let callback = args.first().copied().unwrap_or(Value::undefined());
    let this_arg = args.get(1).copied().unwrap_or(Value::undefined());
    let source_ptr = this.heap_ptr().unwrap();
    if length == 0 {
        return Value::boolean(false);
    }
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Some,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: 0,
        length,
        source_frame_depth: 0,
        accumulator: None,
    });
    let element = crate::vm::array_like_index(this, 0).unwrap_or(Value::undefined());
    vm.push_callback_call(gc, callback, this_arg, vec![element, Value::smi(0), this]);
    Value::undefined()
}

/// Array.prototype.every(callback, thisArg) — set up state machine iteration.
pub fn array_every(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::boolean(true),
    };
    let callback = args.first().copied().unwrap_or(Value::undefined());
    let this_arg = args.get(1).copied().unwrap_or(Value::undefined());
    let source_ptr = this.heap_ptr().unwrap();
    if length == 0 {
        return Value::boolean(true);
    }
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Every,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: 0,
        length,
        source_frame_depth: 0,
        accumulator: None,
    });
    let element = crate::vm::array_like_index(this, 0).unwrap_or(Value::undefined());
    vm.push_callback_call(gc, callback, this_arg, vec![element, Value::smi(0), this]);
    Value::undefined()
}

/// Return a list of builtins to register in every new Vm.
/// Promise(value) or new Promise(executor) — creates a Promise.
pub fn promise_constructor(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let proto_ptr = vm.promise_prototype.heap_ptr();
    let promise_ptr = Promise::allocate(gc, proto_ptr);
    let promise_val = Value::from_heap_ptr(promise_ptr);
    let resolve_handle = vm.get_builtin("_promise_resolve").unwrap_or(Value::undefined());
    let reject_handle = vm.get_builtin("_promise_reject").unwrap_or(Value::undefined());
    let executor = args.first().copied().unwrap_or(Value::undefined());
    if executor.is_undefined() { return promise_val; }
    let resolve_func = vm.create_promise_bridge(gc, promise_val, resolve_handle);
    let reject_func = vm.create_promise_bridge(gc, promise_val, reject_handle);
    vm.pending_promise_ctor = Some(crate::vm::PendingPromiseCtor {
        source_frame_depth: 0,
        promise: promise_val,
        resolve_handle,
        reject_handle,
        resolve_with_result: false,
    });
    vm.push_callback_call(gc, executor, Value::undefined(), vec![resolve_func, reject_func]);
    Value::undefined()
}

/// Internal: resolve a promise. Promise is `this`.
pub fn promise_resolve_impl(_gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_PROMISE && unsafe { Promise::state(ptr) == PROMISE_PENDING } {
            let val = args.first().copied().unwrap_or(Value::undefined());
            unsafe { Promise::set_state(ptr, PROMISE_FULFILLED); Promise::set_result(ptr, val); }
            let reactions_ptr = unsafe { Promise::reactions(ptr) };
            if !reactions_ptr.is_null() {
                let arr = reactions_ptr as *mut RuneArray;
                let len = unsafe { RuneArray::length(arr) };
                let mut idx = 0;
                while idx + 1 < len as usize {
                    let cb = unsafe { RuneArray::get_element(arr, idx) };
                    let chained = unsafe { RuneArray::get_element(arr, idx + 1) };
                    if cb.is_heap_object() {
                        let ppc = crate::vm::PendingPromiseCtor {
                            source_frame_depth: 0, promise: chained,
                            resolve_handle: Value::undefined(), reject_handle: Value::undefined(),
                            resolve_with_result: true,
                        };
                        vm.enqueue_microtask(cb, vec![val], Some(ppc));
                    }
                    idx += 2;
                }
            }
        }
    }
    Value::undefined()
}

/// Internal: reject a promise. Promise is `this`.
pub fn promise_reject_impl(_gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_PROMISE && unsafe { Promise::state(ptr) == PROMISE_PENDING } {
            let reason = args.first().copied().unwrap_or(Value::undefined());
            unsafe { Promise::set_state(ptr, PROMISE_REJECTED); Promise::set_result(ptr, reason); }
            let reactions_ptr = unsafe { Promise::reactions(ptr) };
            if !reactions_ptr.is_null() {
                let arr = reactions_ptr as *mut RuneArray;
                let len = unsafe { RuneArray::length(arr) };
                let mut idx = 0;
                while idx + 1 < len as usize {
                    let cb = unsafe { RuneArray::get_element(arr, idx) };
                    let chained = unsafe { RuneArray::get_element(arr, idx + 1) };
                    if cb.is_heap_object() {
                        let ppc = crate::vm::PendingPromiseCtor {
                            source_frame_depth: 0, promise: chained,
                            resolve_handle: Value::undefined(), reject_handle: Value::undefined(),
                            resolve_with_result: true,
                        };
                        vm.enqueue_microtask(cb, vec![reason], Some(ppc));
                    }
                    idx += 2;
                }
            }
        }
    }
    Value::undefined()
}

/// Promise.prototype.then(onFulfilled, onRejected)
pub fn promise_prototype_then(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let ptr = match this.heap_ptr() { Some(p) => p, None => return Value::undefined() };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag != TAG_PROMISE { return Value::undefined(); }
    let state = unsafe { Promise::state(ptr) };
    let result = unsafe { Promise::result(ptr) };
    let on_fulfilled = args.first().copied().unwrap_or(Value::undefined());
    let on_rejected = args.get(1).copied().unwrap_or(Value::undefined());
    let proto = vm.promise_prototype.heap_ptr();
    let new_promise_ptr = Promise::allocate(gc, proto);
    let new_promise = Value::from_heap_ptr(new_promise_ptr);
    if state == PROMISE_FULFILLED {
        if let Some(op) = on_fulfilled.heap_ptr() && unsafe { (*(op as *const GcHeader)).tag() == TAG_FUNC } {
            let ppc = crate::vm::PendingPromiseCtor {
                source_frame_depth: 0, promise: new_promise,
                resolve_handle: Value::undefined(), reject_handle: Value::undefined(),
                resolve_with_result: true,
            };
            vm.enqueue_microtask(on_fulfilled, vec![result], Some(ppc));
            return new_promise;
        }
        unsafe { Promise::set_state(new_promise_ptr, PROMISE_FULFILLED); Promise::set_result(new_promise_ptr, result); }
        return new_promise;
    }
    if state == PROMISE_REJECTED {
        if let Some(op) = on_rejected.heap_ptr() && unsafe { (*(op as *const GcHeader)).tag() == TAG_FUNC } {
            let ppc = crate::vm::PendingPromiseCtor {
                source_frame_depth: 0, promise: new_promise,
                resolve_handle: Value::undefined(), reject_handle: Value::undefined(),
                resolve_with_result: true,
            };
            vm.enqueue_microtask(on_rejected, vec![result], Some(ppc));
            return new_promise;
        }
        unsafe { Promise::set_state(new_promise_ptr, PROMISE_REJECTED); Promise::set_result(new_promise_ptr, result); }
        return new_promise;
    }
    // Pending — store reaction in the promise's reactions array
    let reactions_ptr = unsafe { Promise::reactions(ptr) };
    if !reactions_ptr.is_null() {
        unsafe { RuneArray::push(gc, reactions_ptr as *mut RuneArray, on_fulfilled); }
        unsafe { RuneArray::push(gc, reactions_ptr as *mut RuneArray, new_promise); }
    }
    new_promise
}

/// Promise.prototype.catch(onRejected)
pub fn promise_prototype_catch(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    promise_prototype_then(gc, this, &[Value::undefined(), args.first().copied().unwrap_or(Value::undefined())], vm)
}

/// Promise.prototype.finally(onFinally) — calls onFinally when settled, passes through original result.
pub fn promise_prototype_finally(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let on_finally = args.first().copied().unwrap_or(Value::undefined());
    let ptr = match this.heap_ptr() { Some(p) => p, None => return Value::undefined() };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag != TAG_PROMISE { return Value::undefined(); }
    let state = unsafe { Promise::state(ptr) };
    let result = unsafe { Promise::result(ptr) };
    let proto = vm.promise_prototype.heap_ptr();
    let new_promise_ptr = Promise::allocate(gc, proto);
    let new_promise = Value::from_heap_ptr(new_promise_ptr);

    // If on_finally is not callable, propagate the original result directly
    if !on_finally.is_heap_object() || unsafe { (*(on_finally.heap_ptr().unwrap() as *const GcHeader)).tag() != TAG_FUNC } {
        if state == PROMISE_FULFILLED || state == PROMISE_REJECTED {
            unsafe { Promise::set_state(new_promise_ptr, state); Promise::set_result(new_promise_ptr, result); }
        }
        return new_promise;
    }

    if state == PROMISE_FULFILLED {
        vm.pending_finally_op = Some(crate::vm::PendingFinallyOp {
            promise: new_promise,
            orig_value: result,
            is_reject: false,
            source_frame_depth: 0,
        });
        vm.push_callback_call(gc, on_finally, Value::undefined(), vec![]);
        return Value::undefined();
    }

    if state == PROMISE_REJECTED {
        vm.pending_finally_op = Some(crate::vm::PendingFinallyOp {
            promise: new_promise,
            orig_value: result,
            is_reject: true,
            source_frame_depth: 0,
        });
        vm.push_callback_call(gc, on_finally, Value::undefined(), vec![]);
        return Value::undefined();
    }

    // Pending case: fall back to .then(on_finally, on_finally) behaviour
    // (doesn't passthrough correctly for pending promises — known limitation)
    promise_prototype_then(gc, this, &[on_finally, on_finally], vm)
}

/// Promise.resolve(value) — returns a fulfilled promise. If value is a promise, returns it.
pub fn promise_static_resolve(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let val = args.first().copied().unwrap_or(Value::undefined());
    if let Some(ptr) = val.heap_ptr() && unsafe { (*(ptr as *const GcHeader)).tag() == TAG_PROMISE } {
        return val;
    }
    let ptr = Promise::allocate(gc, vm.promise_prototype.heap_ptr());
    unsafe { Promise::set_state(ptr, PROMISE_FULFILLED); Promise::set_result(ptr, val); }
    Value::from_heap_ptr(ptr)
}

/// Promise.reject(reason) — returns a rejected promise.
pub fn promise_static_reject(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let val = args.first().copied().unwrap_or(Value::undefined());
    let ptr = Promise::allocate(gc, vm.promise_prototype.heap_ptr());
    unsafe { Promise::set_state(ptr, PROMISE_REJECTED); Promise::set_result(ptr, val); }
    Value::from_heap_ptr(ptr)
}

/// Async generator continuation: resumes an async generator with a resolved value.
/// Called via bridge function: async_continue(this=gen_id_smi, args=[value])
pub fn async_continue(_gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let gen_id = this.as_smi().unwrap_or(0) as usize;
    let value = args.first().copied().unwrap_or(Value::undefined());
    vm.pending_async_gen = Some(crate::vm::PendingAsyncGen { gen_id, arg: value, is_throw: false });
    Value::undefined()
}

/// Async generator rejection: resumes an async generator with a thrown error.
/// Called via bridge function: async_reject(this=gen_id_smi, args=[reason])
pub fn async_reject(_gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let gen_id = this.as_smi().unwrap_or(0) as usize;
    let reason = args.first().copied().unwrap_or(Value::undefined());
    vm.pending_async_gen = Some(crate::vm::PendingAsyncGen { gen_id, arg: reason, is_throw: true });
    Value::undefined()
}

/// Promise.all(iterable) — returns a promise that fulfills when all items fulfill,
/// or rejects on the first rejection.
pub fn promise_static_all(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let iterable = args.first().copied().unwrap_or(Value::undefined());
    let proto = vm.promise_prototype.heap_ptr();
    let result_ptr = Promise::allocate(gc, proto);
    let result_val = Value::from_heap_ptr(result_ptr);
    let len = if let Some(l) = crate::vm::array_like_length(iterable) { l } else {
        unsafe { Promise::set_state(result_ptr, PROMISE_FULFILLED); }
        return result_val;
    };
    if len == 0 {
        let arr = RuneArray::allocate(gc, &[]);
        unsafe { Promise::set_state(result_ptr, PROMISE_FULFILLED); Promise::set_result(result_ptr, Value::from_heap_ptr(arr as *mut u8)); }
        return result_val;
    }
    let mut arr_ptr = RuneArray::allocate(gc, &[]);
    let mut remaining: u32 = len;
    for i in 0..len {
        let item = crate::vm::array_like_index(iterable, i).unwrap_or(Value::undefined());
        let is_promise = if let Some(ptr) = item.heap_ptr() { unsafe { (*(ptr as *const GcHeader)).tag() == TAG_PROMISE } } else { false };
        if is_promise {
            let ptr = item.heap_ptr().unwrap();
            let state = unsafe { Promise::state(ptr) };
            if state == PROMISE_FULFILLED {
                let r = unsafe { Promise::result(ptr) };
                arr_ptr = unsafe { RuneArray::push(gc, arr_ptr, r) };
                remaining -= 1;
            } else if state == PROMISE_REJECTED {
                let r = unsafe { Promise::result(ptr) };
                unsafe { Promise::set_state(result_ptr, PROMISE_REJECTED); Promise::set_result(result_ptr, r); }
                return result_val;
            }
        } else {
            arr_ptr = unsafe { RuneArray::push(gc, arr_ptr, item) };
            remaining -= 1;
        }
    }
    if remaining == 0 {
        unsafe { Promise::set_state(result_ptr, PROMISE_FULFILLED); Promise::set_result(result_ptr, Value::from_heap_ptr(arr_ptr as *mut u8)); }
    }
    result_val
}

/// Promise.race(iterable) — settles with the first settled promise or value.
pub fn promise_static_race(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let iterable = args.first().copied().unwrap_or(Value::undefined());
    let proto = vm.promise_prototype.heap_ptr();
    let result_ptr = Promise::allocate(gc, proto);
    let result_val = Value::from_heap_ptr(result_ptr);
    let len = if let Some(l) = crate::vm::array_like_length(iterable) { l } else { return result_val; };
    if len == 0 { return result_val; }
    for i in 0..len {
        let item = crate::vm::array_like_index(iterable, i).unwrap_or(Value::undefined());
        let is_promise = if let Some(ptr) = item.heap_ptr() { unsafe { (*(ptr as *const GcHeader)).tag() == TAG_PROMISE } } else { false };
        if is_promise {
            let ptr = item.heap_ptr().unwrap();
            let state = unsafe { Promise::state(ptr) };
            if state == PROMISE_FULFILLED {
                let r = unsafe { Promise::result(ptr) };
                unsafe { Promise::set_state(result_ptr, PROMISE_FULFILLED); Promise::set_result(result_ptr, r); }
                return result_val;
            }
            if state == PROMISE_REJECTED {
                let r = unsafe { Promise::result(ptr) };
                unsafe { Promise::set_state(result_ptr, PROMISE_REJECTED); Promise::set_result(result_ptr, r); }
                return result_val;
            }
        } else {
            unsafe { Promise::set_state(result_ptr, PROMISE_FULFILLED); Promise::set_result(result_ptr, item); }
            return result_val;
        }
    }
    result_val
}

pub fn default_builtins() -> Vec<Builtin> {
    vec![
        Builtin {
            length: 0,
            name: "print",
            func: print_builtin,
        },
        Builtin {
            length: 1,
            name: "String",
            func: string_builtin,
        },
        Builtin {
            length: 1,
            name: "Number",
            func: number_builtin,
        },
        Builtin {
            length: 1,
            name: "_promise_resolve",
            func: promise_resolve_impl,
        },
        Builtin {
            length: 1,
            name: "_promise_reject",
            func: promise_reject_impl,
        },
        Builtin {
            length: 1,
            name: "Promise",
            func: promise_constructor,
        },
        Builtin {
            length: 2,
            name: "Promise_prototype_then",
            func: promise_prototype_then,
        },
        Builtin {
            length: 1,
            name: "Promise_prototype_catch",
            func: promise_prototype_catch,
        },
        Builtin { length: 1, name: "Promise_prototype_finally", func: promise_prototype_finally },
        Builtin { length: 1, name: "Promise_resolve", func: promise_static_resolve },
        Builtin { length: 1, name: "Promise_reject", func: promise_static_reject },
        Builtin { length: 1, name: "Promise_all", func: promise_static_all },
        Builtin { length: 1, name: "Promise_race", func: promise_static_race },
        Builtin { length: 1, name: "async_continue", func: async_continue },
        Builtin { length: 1, name: "async_reject", func: async_reject },
        Builtin {
            length: 1,
            name: "Object",
            func: object_builtin,
        },
        Builtin {
            length: 1,
            name: "Error",
            func: error_builtin,
        },
        Builtin {
            length: 1,
            name: "Test262Error",
            func: test262_error_builtin,
        },
        Builtin {
            length: 0,
            name: "$DONOTEVALUATE",
            func: donot_evaluate_builtin,
        },
        Builtin {
            length: 1,
            name: "eval",
            func: eval_builtin,
        },
        Builtin {
            length: 2,
            name: "Object_create",
            func: object_create_builtin,
        }, // accessible only via Object.create
        Builtin {
            length: 1,
            name: "Object_keys",
            func: object_keys,
        },
        Builtin {
            length: 1,
            name: "Object_values",
            func: object_values,
        },
        Builtin {
            length: 1,
            name: "Object_entries",
            func: object_entries,
        },
        Builtin {
            length: 1,
            name: "Array_isArray",
            func: array_is_array,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_push",
            func: array_push,
        },
        Builtin {
            length: 0,
            name: "Array_prototype_pop",
            func: array_pop,
        },
        Builtin {
            length: 1,
            name: "String_fromCharCode",
            func: string_from_char_code,
        },
        Builtin {
            length: 1,
            name: "String_prototype_charAt",
            func: string_char_at,
        },
        Builtin {
            length: 2,
            name: "String_prototype_slice",
            func: string_slice,
        },
        Builtin {
            length: 2,
            name: "String_prototype_split",
            func: string_split,
        },
        Builtin {
            length: 1,
            name: "String_prototype_indexOf",
            func: string_index_of,
        },
        Builtin {
            length: 1,
            name: "String_prototype_includes",
            func: string_includes,
        },
        Builtin {
            length: 1,
            name: "String_prototype_startsWith",
            func: string_starts_with,
        },
        Builtin {
            length: 1,
            name: "String_prototype_endsWith",
            func: string_ends_with,
        },
        Builtin {
            length: 1,
            name: "String_prototype_charCodeAt",
            func: string_char_code_at,
        },
        Builtin {
            length: 1,
            name: "String_prototype_codePointAt",
            func: string_code_point_at,
        },
        Builtin {
            length: 2,
            name: "String_prototype_substring",
            func: string_substring,
        },
        Builtin {
            length: 2,
            name: "String_prototype_substr",
            func: string_substr,
        },
        Builtin {
            length: 0,
            name: "String_prototype_trim",
            func: string_trim,
        },
        Builtin {
            length: 0,
            name: "String_prototype_trimStart",
            func: string_trim_start,
        },
        Builtin {
            length: 0,
            name: "String_prototype_trimEnd",
            func: string_trim_end,
        },
        Builtin {
            length: 0,
            name: "String_prototype_toLowerCase",
            func: string_to_lower_case,
        },
        Builtin {
            length: 0,
            name: "String_prototype_toUpperCase",
            func: string_to_upper_case,
        },
        Builtin {
            length: 1,
            name: "String_prototype_repeat",
            func: string_repeat,
        },
        Builtin {
            length: 1,
            name: "String_prototype_padStart",
            func: string_pad_start,
        },
        Builtin {
            length: 1,
            name: "String_prototype_padEnd",
            func: string_pad_end,
        },
        Builtin {
            length: 1,
            name: "String_prototype_concat",
            func: string_concat,
        },
        Builtin {
            length: 0,
            name: "String_prototype_toString",
            func: string_to_string,
        },
        Builtin {
            length: 0,
            name: "String_prototype_valueOf",
            func: string_value_of,
        },
        Builtin {
            length: 2,
            name: "String_prototype_replace",
            func: string_replace,
        },
        Builtin {
            length: 2,
            name: "String_prototype_replaceAll",
            func: string_replace_all,
        },
        Builtin {
            length: 1,
            name: "Math_floor",
            func: math_floor,
        },
        Builtin {
            length: 1,
            name: "Math_ceil",
            func: math_ceil,
        },
        Builtin {
            length: 1,
            name: "Math_abs",
            func: math_abs,
        },
        Builtin {
            length: 2,
            name: "Math_min",
            func: math_min,
        },
        Builtin {
            length: 2,
            name: "Math_max",
            func: math_max,
        },
        Builtin {
            length: 2,
            name: "Math_pow",
            func: math_pow,
        },
        Builtin {
            length: 1,
            name: "Math_sqrt",
            func: math_sqrt,
        },
        // Global functions
        Builtin {
            length: 2,
            name: "parseInt",
            func: parse_int_builtin,
        },
        Builtin {
            length: 1,
            name: "parseFloat",
            func: parse_float_builtin,
        },
        // JSON
        Builtin {
            length: 2,
            name: "JSON_parse",
            func: json_parse,
        },
        Builtin {
            length: 3,
            name: "JSON_stringify",
            func: json_stringify,
        },
        // Array.prototype methods
        Builtin {
            length: 1,
            name: "Array_prototype_filter",
            func: array_filter,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_map",
            func: array_map,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_reduce",
            func: array_reduce,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_forEach",
            func: array_for_each,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_slice",
            func: array_slice,
        },
        Builtin {
            length: 2,
            name: "Array_prototype_includes",
            func: array_includes,
        },
        Builtin {
            length: 2,
            name: "Array_prototype_indexOf",
            func: array_index_of,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_find",
            func: array_find,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_findIndex",
            func: array_find_index,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_some",
            func: array_some,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_every",
            func: array_every,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_flat",
            func: array_flat,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_flatMap",
            func: array_flat_map,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_sort",
            func: array_sort,
        },
        Builtin {
            length: 1,
            name: "Function_prototype_call",
            func: call_builtin,
        },
        // Test262 assert builtins
        Builtin {
            length: 2,
            name: "assert_sameValue",
            func: assert_same_value,
        },
        Builtin {
            length: 2,
            name: "assert_notSameValue",
            func: assert_not_same_value,
        },
        Builtin {
            length: 2,
            name: "assert_throws",
            func: assert_throws,
        },
        Builtin {
            length: 1,
            name: "assert",
            func: assert_plain,
        },
        Builtin {
            length: 2,
            name: "assert__isSameValue",
            func: assert_is_same_value,
        },
    ]
}

// ---- Test262 assert builtins ----

/// SameValue comparison per ECMAScript §7.2.11.
/// NaN === NaN, +0 !== -0.
fn same_value(a: Value, b: Value) -> bool {
    // Both undefined or both null
    if a.is_undefined() && b.is_undefined() { return true; }
    if a.is_null() && b.is_null() { return true; }
    // Both booleans
    if let (Some(ab), Some(bb)) = (a.to_boolean(), b.to_boolean()) {
        return ab == bb;
    }
    // Both heap pointers (strings, objects)
    if let (Some(ap), Some(bp)) = (a.heap_ptr(), b.heap_ptr()) {
        // Compare strings by content, objects by identity
        unsafe {
            let ta = (*(ap as *const GcHeader)).tag();
            let tb = (*(bp as *const GcHeader)).tag();
            if ta == TAG_STRING && tb == TAG_STRING {
                return HeapString::to_string(ap as *mut HeapString)
                    == HeapString::to_string(bp as *mut HeapString);
            }
        }
        return ap == bp;
    }
    // Numeric comparison (accept both Smi and Float64)
    let a_num = a.as_smi().map(|v| v as f64).or_else(|| a.as_float64());
    let b_num = b.as_smi().map(|v| v as f64).or_else(|| b.as_float64());
    match (a_num, b_num) {
        (Some(av), Some(bv)) => {
            // SameValue: NaN === NaN
            if av.is_nan() && bv.is_nan() { return true; }
            // SameValue: +0 !== -0
            if av == 0.0 && bv == 0.0 {
                return av.to_bits() == bv.to_bits();
            }
            av == bv
        }
        _ => false,
    }
}

fn value_to_debug(v: Value) -> String {
    if v.is_undefined() {
        "undefined".to_string()
    } else if v.is_null() {
        "null".to_string()
    } else if let Some(b) = v.to_boolean() {
        b.to_string()
    } else if let Some(n) = v.as_smi() {
        n.to_string()
    } else if let Some(f) = v.as_float64() {
        if f.is_nan() {
            "NaN".to_string()
        } else if f.is_infinite() {
            if f.is_sign_negative() { "-Infinity".to_string() } else { "Infinity".to_string() }
        } else if f.fract() == 0.0 && (-(1 << 30) as f64..(1 << 30) as f64).contains(&f) {
            format!("{}", f as i64)
        } else {
            f.to_string()
        }
    } else if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            unsafe { HeapString::to_string(ptr as *mut HeapString) }
        } else if tag == TAG_STRING_OBJ {
            let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
            format!("String {{ [[StringData]]: \"{}\" }}", unsafe { HeapString::to_string(str_ptr as *mut HeapString) })
        } else {
            format!("{:p}", ptr)
        }
    } else {
        format!("{:?}", v)
    }
}

pub(crate) fn make_error(gc: &mut SemiSpace, msg: &str) -> Value {
    let s = HeapString::allocate(gc, msg);
    make_simple_object(gc, "message", Value::from_heap_ptr(s as *mut u8))
}

/// Extract a human-readable error message from an exception Value.
/// Returns `None` if the value is not an object with a "message" string property.
pub fn read_error_message(val: Value) -> Option<String> {
    let ptr = val.heap_ptr()?;
    unsafe {
        let tag = (*(ptr as *const GcHeader)).tag();
        if tag == TAG_STRING {
            return Some(HeapString::to_string(ptr as *mut HeapString));
        }
        if tag != TAG_OBJECT {
            return None;
        }
        let shape = JSObject::shape_ptr(ptr as *mut JSObject);
        let key = PropertyKey::from_string("message");
        let slot = shape.lookup(&key)?;
        let msg_val = JSObject::get_slot(ptr as *mut JSObject, slot);
        let msg_ptr = msg_val.heap_ptr()?;
        let tag2 = (*(msg_ptr as *const GcHeader)).tag();
        if tag2 != TAG_STRING {
            return None;
        }
        Some(HeapString::to_string(msg_ptr as *mut HeapString))
    }
}

/// assert.sameValue(actual, expected, description) — uses SameValue semantics.
pub fn assert_same_value(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    _vm.assert_called = true;
    let actual = args.first().copied().unwrap_or(Value::undefined());
    let expected = args.get(1).copied().unwrap_or(Value::undefined());
    let desc = args.get(2).map(|v| value_to_debug(*v)).unwrap_or_default();
    if !same_value(actual, expected) {
        let msg = if desc.is_empty() {
            format!(
                "assert.sameValue: expected {} but got {}",
                value_to_debug(expected),
                value_to_debug(actual)
            )
        } else {
            format!(
                "{}: assert.sameValue: expected {} but got {}",
                desc,
                value_to_debug(expected),
                value_to_debug(actual)
            )
        };
        let err = make_error(gc, &msg);
        _vm.set_pending_exception(err);
    }
    Value::undefined()
}

/// assert.notSameValue(actual, expected, description) — uses SameValue semantics.
pub fn assert_not_same_value(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    _vm: &mut Vm,
) -> Value {
    _vm.assert_called = true;
    let actual = args.first().copied().unwrap_or(Value::undefined());
    let expected = args.get(1).copied().unwrap_or(Value::undefined());
    let desc = args.get(2).map(|v| value_to_debug(*v)).unwrap_or_default();
    if same_value(actual, expected) {
        let msg = if desc.is_empty() {
            format!(
                "assert.notSameValue: expected different value but got {}",
                value_to_debug(actual)
            )
        } else {
            format!(
                "{}: assert.notSameValue: expected different value but got {}",
                desc,
                value_to_debug(actual)
            )
        };
        let err = make_error(gc, &msg);
        _vm.set_pending_exception(err);
    }
    Value::undefined()
}

/// assert() — plain assert function that throws Test262Error if condition is falsy.
pub fn assert_plain(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    _vm.assert_called = true;
    let cond = args.first().copied().unwrap_or(Value::undefined());
    if !cond.to_bool() {
        let msg = args.get(1).map(|v| value_to_debug(*v)).unwrap_or_default();
        let full_msg = if msg.is_empty() {
            "assert: expected truthy value".to_string()
        } else {
            format!("assert: {msg}")
        };
        let err = make_error(gc, &full_msg);
        _vm.set_pending_exception(err);
    }
    Value::undefined()
}

/// assert._isSameValue(a, b) — internal helper for test262 assert.js.
pub fn assert_is_same_value(_gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    vm.assert_called = true;
    let a = args.first().copied().unwrap_or(Value::undefined());
    let b = args.get(1).copied().unwrap_or(Value::undefined());
    if same_value(a, b) {
        Value::boolean(true)
    } else {
        Value::boolean(false)
    }
}

/// assert.throws(errorConstructor, func, message) — rewritten to use callback state machine.
pub fn assert_throws(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    vm.assert_called = true;
    if args.len() < 2 {
        let err = make_error(
            gc,
            "assert.throws: expected errorConstructor and func arguments",
        );
        vm.set_pending_exception(err);
        return Value::undefined();
    }
    let error_ctor = args[0];
    let func = args[1];

    // Set up pending assert state for the Return/Throw handlers
    vm.pending_assert = Some(crate::vm::PendingAssert {
        expected_error: error_ctor,
        source_frame_depth: 0, // will be set by push_callback_call
    });

    // Push the function call — the Return handler will catch the result
    vm.push_callback_call(gc, func, Value::undefined(), vec![]);

    Value::undefined()
}

/// Build a wrapper object for the Object constructor, exposing methods like .create().
/// Returns (object_value, create_builtin_smi_index).
pub fn build_object_constructor(gc: &mut SemiSpace) -> Value {
    let shape = Shape::empty();
    let ptr = JSObject::allocate(gc, shape, &[]);
    Value::from_heap_ptr(ptr as *mut u8)
}
