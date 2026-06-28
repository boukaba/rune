use crate::vm::Vm;
use rune_core::array::RuneArray;
use rune_core::gc::{GcHeader, SemiSpace, TAG_ARRAY, TAG_OBJECT, TAG_STRING, TAG_STRING_OBJ};
use rune_core::object::JSObject;
use rune_core::shape::{DENSE_ARRAY_SHAPE, PropertyKey, Shape};
use rune_core::string::HeapString;
use rune_core::string_object::StringObject;
use rune_core::value::Value;

/// A registered built-in function.
pub struct Builtin {
    pub name: &'static str,
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
fn to_primitive_string(
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

/// String.prototype.indexOf(searchString, position) — returns the index of the first occurrence.
pub fn string_index_of(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let search_str = args.first().map(|&v| value_to_js_string(v)).unwrap_or_default();
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
    let search_str = args.first().map(|&v| value_to_js_string(v)).unwrap_or_default();
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
    let search_str = args.first().map(|&v| value_to_js_string(v)).unwrap_or_default();
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
    let search_str = args.first().map(|&v| value_to_js_string(v)).unwrap_or_default();
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
        Some(v) if !v.is_undefined() => value_to_js_string(*v),
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
        Some(v) if !v.is_undefined() => value_to_js_string(*v),
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
        result.push_str(&value_to_js_string(arg));
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
        let sep = value_to_js_string(separator);
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

/// Return a list of builtins to register in every new Vm.
pub fn default_builtins() -> Vec<Builtin> {
    vec![
        Builtin {
            name: "print",
            func: print_builtin,
        },
        Builtin {
            name: "String",
            func: string_builtin,
        },
        Builtin {
            name: "Object",
            func: object_builtin,
        },
        Builtin {
            name: "Error",
            func: error_builtin,
        },
        Builtin {
            name: "Test262Error",
            func: test262_error_builtin,
        },
        Builtin {
            name: "$DONOTEVALUATE",
            func: donot_evaluate_builtin,
        },
        Builtin {
            name: "eval",
            func: eval_builtin,
        },
        Builtin {
            name: "Object_create",
            func: object_create_builtin,
        }, // accessible only via Object.create
        Builtin {
            name: "Array_isArray",
            func: array_is_array,
        },
        Builtin {
            name: "Array_prototype_push",
            func: array_push,
        },
        Builtin {
            name: "Array_prototype_pop",
            func: array_pop,
        },
        Builtin {
            name: "String_fromCharCode",
            func: string_from_char_code,
        },
        Builtin {
            name: "String_prototype_charAt",
            func: string_char_at,
        },
        Builtin {
            name: "String_prototype_slice",
            func: string_slice,
        },
        Builtin {
            name: "String_prototype_split",
            func: string_split,
        },
        Builtin {
            name: "String_prototype_indexOf",
            func: string_index_of,
        },
        Builtin {
            name: "String_prototype_includes",
            func: string_includes,
        },
        Builtin {
            name: "String_prototype_startsWith",
            func: string_starts_with,
        },
        Builtin {
            name: "String_prototype_endsWith",
            func: string_ends_with,
        },
        Builtin {
            name: "String_prototype_charCodeAt",
            func: string_char_code_at,
        },
        Builtin {
            name: "String_prototype_codePointAt",
            func: string_code_point_at,
        },
        Builtin {
            name: "String_prototype_substring",
            func: string_substring,
        },
        Builtin {
            name: "String_prototype_substr",
            func: string_substr,
        },
        Builtin {
            name: "String_prototype_trim",
            func: string_trim,
        },
        Builtin {
            name: "String_prototype_trimStart",
            func: string_trim_start,
        },
        Builtin {
            name: "String_prototype_trimEnd",
            func: string_trim_end,
        },
        Builtin {
            name: "String_prototype_toLowerCase",
            func: string_to_lower_case,
        },
        Builtin {
            name: "String_prototype_toUpperCase",
            func: string_to_upper_case,
        },
        Builtin {
            name: "String_prototype_repeat",
            func: string_repeat,
        },
        Builtin {
            name: "String_prototype_padStart",
            func: string_pad_start,
        },
        Builtin {
            name: "String_prototype_padEnd",
            func: string_pad_end,
        },
        Builtin {
            name: "String_prototype_concat",
            func: string_concat,
        },
        Builtin {
            name: "String_prototype_toString",
            func: string_to_string,
        },
        Builtin {
            name: "String_prototype_valueOf",
            func: string_value_of,
        },
        Builtin {
            name: "Math_floor",
            func: math_floor,
        },
        Builtin {
            name: "Math_ceil",
            func: math_ceil,
        },
        Builtin {
            name: "Math_abs",
            func: math_abs,
        },
        Builtin {
            name: "Math_min",
            func: math_min,
        },
        Builtin {
            name: "Math_max",
            func: math_max,
        },
        Builtin {
            name: "Math_pow",
            func: math_pow,
        },
        Builtin {
            name: "Math_sqrt",
            func: math_sqrt,
        },
        // Global functions
        Builtin {
            name: "parseInt",
            func: parse_int_builtin,
        },
        Builtin {
            name: "parseFloat",
            func: parse_float_builtin,
        },
        // JSON
        Builtin {
            name: "JSON_parse",
            func: json_parse,
        },
        Builtin {
            name: "JSON_stringify",
            func: json_stringify,
        },
        // Array.prototype methods
        Builtin {
            name: "Array_prototype_filter",
            func: array_filter,
        },
        Builtin {
            name: "Array_prototype_map",
            func: array_map,
        },
        Builtin {
            name: "Array_prototype_reduce",
            func: array_reduce,
        },
        Builtin {
            name: "Array_prototype_forEach",
            func: array_for_each,
        },
        Builtin {
            name: "Array_prototype_slice",
            func: array_slice,
        },
        Builtin {
            name: "Function_prototype_call",
            func: call_builtin,
        },
        // Test262 assert builtins
        Builtin {
            name: "assert_sameValue",
            func: assert_same_value,
        },
        Builtin {
            name: "assert_notSameValue",
            func: assert_not_same_value,
        },
        Builtin {
            name: "assert_throws",
            func: assert_throws,
        },
        Builtin {
            name: "assert",
            func: assert_plain,
        },
        Builtin {
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
