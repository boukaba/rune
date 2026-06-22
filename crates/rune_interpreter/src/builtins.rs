use crate::vm::Vm;
use rune_core::array::RuneArray;
use rune_core::float::HeapFloat64;
use rune_core::gc::{GcHeader, SemiSpace, TAG_ARRAY, TAG_STRING};
use rune_core::object::JSObject;
use rune_core::shape::{PropertyKey, Shape};
use rune_core::string::HeapString;
use rune_core::value::Value;

/// A registered built-in function.
pub struct Builtin {
    pub name: &'static str,
    pub func: BuiltinFn,
}

/// Signature for a built-in function: receives GC access, `this` value, args, and VM reference.
pub type BuiltinFn = fn(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value;

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
pub fn print_builtin(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let s = args
        .iter()
        .map(|v| format!("{v:?}"))
        .collect::<Vec<_>>()
        .join(" ");
    println!("{s}");
    Value::undefined()
}

/// String(value) — converts a value to its string representation.
pub fn string_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let arg = args.first().copied().unwrap_or(Value::undefined());
    let s = value_to_js_string(arg);
    let ptr = HeapString::allocate(gc, &s);
    Value::from_heap_ptr(ptr as *mut u8)
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
            return Value::smi(1);
        }
    }
    Value::smi(0)
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
                    vm.update_heap_reference(old_ptr, new_arr as *mut u8);
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

/// String.prototype.charAt(index) — returns the character at index as a string.
/// Per §22.1.3.1, OOB returns empty string, not undefined.
pub fn string_char_at(gc: &mut SemiSpace, this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let index = args.first().and_then(|v| v.as_smi()).unwrap_or(0) as usize;
    if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            if index >= s.chars().count() {
                let empty = HeapString::allocate(gc, "");
                return Value::from_heap_ptr(empty as *mut u8);
            }
            let ch = s.chars().nth(index).unwrap();
            let result = HeapString::allocate(gc, &ch.to_string());
            return Value::from_heap_ptr(result as *mut u8);
        }
    }
    Value::undefined()
}

/// String.prototype.slice(start, end) — returns a substring.
pub fn string_slice(gc: &mut SemiSpace, this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let start = args.first().and_then(|v| v.as_smi()).unwrap_or(0) as usize;
    let end = args.get(1).and_then(|v| v.as_smi()).map(|n| n as usize);
    if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            let end = end.unwrap_or(s.len());
            let start = start.min(s.len());
            let end = end.min(s.len());
            let result_s: String = s
                .chars()
                .skip(start)
                .take(end.saturating_sub(start))
                .collect();
            let result = HeapString::allocate(gc, &result_s);
            return Value::from_heap_ptr(result as *mut u8);
        }
    }
    Value::undefined()
}

/// Math.floor(x) — rounds down.
fn math_op_unary(gc: &mut SemiSpace, args: &[Value], op: fn(f64) -> f64) -> Value {
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
    let ptr = HeapFloat64::allocate(gc, result);
    Value::from_float64_ptr(ptr as *mut u8)
}

fn math_op_binary(gc: &mut SemiSpace, args: &[Value], op: fn(f64, f64) -> f64) -> Value {
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
    let ptr = HeapFloat64::allocate(gc, result);
    Value::from_float64_ptr(ptr as *mut u8)
}

pub fn math_floor(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    math_op_unary(gc, args, f64::floor)
}

pub fn math_ceil(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    math_op_unary(gc, args, f64::ceil)
}

pub fn math_abs(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    math_op_unary(gc, args, f64::abs)
}

pub fn math_min(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
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
    let ptr = HeapFloat64::allocate(gc, min);
    Value::from_float64_ptr(ptr as *mut u8)
}

pub fn math_max(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
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
    let ptr = HeapFloat64::allocate(gc, max);
    Value::from_float64_ptr(ptr as *mut u8)
}

pub fn math_pow(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    math_op_binary(gc, args, |a, b| a.powf(b))
}

pub fn math_sqrt(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    math_op_unary(gc, args, f64::sqrt)
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
    ]
}

// ---- Test262 assert builtins ----

/// `assert.sameValue(actual, expected, description)`
/// Performs StrictEqual comparison and throws Test262Error if mismatch.
fn value_eq_strict(a: Value, b: Value) -> bool {
    if let (Some(av), Some(bv)) = (a.as_smi(), b.as_smi()) {
        av == bv
    } else if (a.is_undefined() && b.is_undefined()) || (a.is_null() && b.is_null()) {
        true
    } else if let (Some(ap), Some(bp)) = (a.heap_ptr(), b.heap_ptr()) {
        ap == bp
    } else {
        false
    }
}

fn value_to_debug(v: Value) -> String {
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
            format!("{:p}", ptr)
        }
    } else {
        "undefined".to_string()
    }
}

fn make_error(gc: &mut SemiSpace, msg: &str) -> Value {
    let s = HeapString::allocate(gc, msg);
    make_simple_object(gc, "message", Value::from_heap_ptr(s as *mut u8))
}

/// assert.sameValue(actual, expected, description)
pub fn assert_same_value(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let actual = args.first().copied().unwrap_or(Value::undefined());
    let expected = args.get(1).copied().unwrap_or(Value::undefined());
    let desc = args.get(2).map(|v| value_to_debug(*v)).unwrap_or_default();
    if !value_eq_strict(actual, expected) {
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

/// assert.notSameValue(actual, expected, description)
pub fn assert_not_same_value(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    _vm: &mut Vm,
) -> Value {
    let actual = args.first().copied().unwrap_or(Value::undefined());
    let expected = args.get(1).copied().unwrap_or(Value::undefined());
    let desc = args.get(2).map(|v| value_to_debug(*v)).unwrap_or_default();
    if value_eq_strict(actual, expected) {
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

/// assert.throws(errorConstructor, func, message)
/// Calls func and checks that it throws an error of the expected type.
pub fn assert_throws(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    if args.len() < 2 {
        let err = make_error(
            gc,
            "assert.throws: expected errorConstructor and func arguments",
        );
        _vm.set_pending_exception(err);
        return Value::undefined();
    }
    let _error_ctor = args[0];
    let _func = args[1];
    let _msg = args.get(2).map(|v| value_to_debug(*v)).unwrap_or_default();

    // For now, we can't easily call a JS function from a builtin, so we'll
    // implement a simplified check that expects the pending_exception
    // mechanism. Full implementation deferred to Sprint 14+.
    let err = make_error(
        gc,
        "assert.throws: not yet fully implemented (see Sprint 14)",
    );
    _vm.set_pending_exception(err);
    Value::undefined()
}

/// Build a wrapper object for the Object constructor, exposing methods like .create().
/// Returns (object_value, create_builtin_smi_index).
pub fn build_object_constructor(gc: &mut SemiSpace) -> Value {
    let shape = Shape::empty();
    let ptr = JSObject::allocate(gc, shape, &[]);
    Value::from_heap_ptr(ptr as *mut u8)
}
