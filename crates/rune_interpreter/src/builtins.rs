use crate::vm::SymbolMethodResult;
use crate::vm::Vm;
use crate::vm::get_iter_method;
use crate::vm::get_symbol_method;
use crate::vm::load_property_recursive;
use crate::vm::to_number;
use crate::vm::value_to_array_index;
use crate::vm::value_to_prop_key;
use crate::vm::{CollectionCtorState, PendingCollectionCtor, PendingCollectionForEach};
use rune_core::array::RuneArray;
use rune_core::date;
use rune_core::gc::{
    GcHeader, SemiSpace, TAG_ARRAY, TAG_ARRAY_BUFFER, TAG_DATE, TAG_FLOAT64, TAG_FORWARDED,
    TAG_FUNC, TAG_MAP, TAG_OBJECT, TAG_PROMISE, TAG_REGEXP, TAG_SET, TAG_STRING, TAG_STRING_OBJ,
    TAG_TYPED_ARRAY,
};
use rune_core::map::{RuneMap, RuneSet};
use rune_core::object::JSObject;
use rune_core::promise::{PROMISE_FULFILLED, PROMISE_PENDING, PROMISE_REJECTED, Promise};
use rune_core::regexp::RegExp;
use rune_core::shape::{DENSE_ARRAY_SHAPE, PropertyKey, Shape};
use rune_core::string::HeapString;
use rune_core::string_object::StringObject;
use rune_core::symbol::{
    SYM_MATCH, SYM_REPLACE, SYM_SEARCH, SYM_SPLIT, register_symbol, symbol_display, symbol_for,
    symbol_key_for,
};
use rune_core::typedarray;
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
    } else if let Some(id) = v.as_symbol_id() {
        symbol_display(id)
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
        } else if tag == TAG_DATE {
            date::to_date_string(unsafe { date::RuneDate::tv(ptr) })
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
pub(crate) fn to_primitive_string(gc: &mut SemiSpace, val: Value, vm: &mut Vm) -> Option<String> {
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
    if tag == TAG_DATE {
        // §7.1.1.1: Date's default hint is string → ToDateString
        return Some(date::to_date_string(unsafe { date::RuneDate::tv(ptr) }));
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
            if let Some(smi) = value_of_val.as_smi() {
                if smi < 0 {
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
        }
        // Neither toString nor valueOf returned a primitive
        return Some(value_to_js_string(val));
    }
    Some(value_to_js_string(val))
}

/// Synchronous version of to_primitive_string — never sets up callbacks.
/// User-defined toString/valueOf are skipped (fall through to [object Object]).
/// Use this for string method arguments where the callback pattern would leak.
pub(crate) fn to_primitive_string_sync(val: Value, gc: &mut SemiSpace, vm: &mut Vm) -> String {
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
    if tag == TAG_DATE {
        return date::to_date_string(unsafe { date::RuneDate::tv(ptr) });
    }
    if tag == TAG_OBJECT {
        let key = PropertyKey::from_string("toString");
        let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
        if let Some(slot) = shape.lookup(&key) {
            let to_string_val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
            if let Some(smi) = to_string_val.as_smi() {
                if smi < 0 {
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
                            } else {
                                false
                            }
                        } {
                            return value_to_js_string(result);
                        }
                    }
                }
            }
            // User-defined or non-callable toString — skip
        }
        let value_of_key = PropertyKey::from_string("valueOf");
        if let Some(slot) = shape.lookup(&value_of_key) {
            let value_of_val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
            if let Some(smi) = value_of_val.as_smi() {
                if smi < 0 {
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
                            } else {
                                false
                            }
                        } {
                            return value_to_js_string(result);
                        }
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
    if let Some(rest) = upper.strip_prefix("0X") {
        if let Ok(n) = u64::from_str_radix(rest, 16) {
            return n as f64;
        }
    }
    if trimmed.eq_ignore_ascii_case("infinity") || trimmed == "+Infinity" {
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
        let n = if val.is_null() || val.to_boolean() == Some(false) {
            0.0
        } else {
            1.0
        };
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
    // §7.1.12.1 ToString(Symbol) throws TypeError.
    if arg.is_symbol() {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Cannot convert a Symbol value to a string",
        )));
        return Value::undefined();
    }
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

/// §20.4.1.1 Symbol(description) — returns a new unique symbol. Throws if
/// called with `new` (see Opcode::New).
pub fn symbol_ctor_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let desc = args.first().copied();
    match desc {
        None => Value::symbol(register_symbol(None)),
        Some(v) if v.is_undefined() => Value::symbol(register_symbol(None)),
        Some(v) if v.is_symbol() => {
            // §7.1.12.1 ToString(Symbol) throws TypeError.
            vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                gc,
                "TypeError: Cannot convert a Symbol value to a string",
            )));
            Value::undefined()
        }
        Some(v) => match to_primitive_string(gc, v, vm) {
            Some(s) => Value::symbol(register_symbol(Some(s))),
            None => {
                // Pending ToString callback — the Return handler wraps the
                // toString result into the symbol (see PendingSymbolCoercion).
                vm.pending_symbol_coercion = Some(crate::vm::PendingSymbolCoercion {
                    source_frame_depth: vm.frame_depth(),
                    is_for: false,
                });
                Value::undefined()
            }
        },
    }
}

/// §20.4.2.2 Symbol.for(key) — returns the registered symbol for `key`.
pub fn symbol_for_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let key = args.first().copied().unwrap_or(Value::undefined());
    if key.is_symbol() {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Cannot convert a Symbol value to a string",
        )));
        return Value::undefined();
    }
    match to_primitive_string(gc, key, vm) {
        Some(s) => Value::symbol(symbol_for(&s)),
        None => {
            vm.pending_symbol_coercion = Some(crate::vm::PendingSymbolCoercion {
                source_frame_depth: vm.frame_depth(),
                is_for: true,
            });
            Value::undefined()
        }
    }
}

/// §20.4.2.3 Symbol.keyFor(sym) — the registry key for `sym`, or undefined.
pub fn symbol_key_for_builtin(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let arg = args.first().copied().unwrap_or(Value::undefined());
    match arg.as_symbol_id() {
        Some(id) => match symbol_key_for(id) {
            Some(k) => {
                let ptr = HeapString::allocate(gc, &k);
                Value::from_heap_ptr(ptr as *mut u8)
            }
            None => Value::undefined(),
        },
        None => {
            vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                gc,
                "TypeError: Symbol.keyFor requires that the argument be a symbol",
            )));
            Value::undefined()
        }
    }
}

/// §20.4.3.2 Symbol.prototype.toString() — "Symbol(desc)".
pub fn symbol_prototype_to_string(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    match this.as_symbol_id() {
        Some(id) => {
            let s = symbol_display(id);
            let ptr = HeapString::allocate(gc, &s);
            Value::from_heap_ptr(ptr as *mut u8)
        }
        None => {
            vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                gc,
                "TypeError: Symbol.prototype.toString requires that 'this' be a Symbol",
            )));
            Value::undefined()
        }
    }
}

/// §20.4.3.4 Symbol.prototype.valueOf() — the symbol itself.
pub fn symbol_prototype_value_of(
    _gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    if this.is_symbol() {
        this
    } else {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            _gc,
            "TypeError: Symbol.prototype.valueOf requires that 'this' be a Symbol",
        )));
        Value::undefined()
    }
}

/// §20.4.3.5 Symbol.prototype[@@toPrimitive](hint) — returns the symbol.
pub fn symbol_prototype_to_primitive(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    if this.is_symbol() {
        this
    } else {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Symbol.prototype[Symbol.toPrimitive] requires that 'this' be a Symbol",
        )));
        Value::undefined()
    }
}

// ── Iteration protocol builtins ──────────────────────────────────────────

/// Resolve a builtin handle by name (linear scan of the registry).
fn find_handle(builtins: &[Builtin], name: &str) -> Option<Value> {
    builtins
        .iter()
        .position(|b| b.name == name)
        .map(|id| Value::smi(-(id as i32) - 1))
}

/// Create an iterator object: own `next` property, hidden per-instance state
/// stored under the rune-internal state symbol (excluded from enumeration),
/// plus @@toStringTag and @@iterator (returns the iterator itself).
fn make_iterator_object(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    next_handle_name: &str,
    state_elems: &[Value],
    tag: &str,
) -> Value {
    let next_h = find_handle(&vm.builtins, next_handle_name).unwrap();
    let iter_h = find_handle(&vm.builtins, "Iterator_prototype_symbol_iterator").unwrap();
    let state = RuneArray::allocate(gc, state_elems);
    let keys = vec![
        (PropertyKey::from_string("next"), 0),
        (PropertyKey::from_symbol(vm.iter_state_symbol), 1),
        (
            PropertyKey::from_symbol(rune_core::symbol::SYM_TO_STRING_TAG),
            2,
        ),
        (PropertyKey::from_symbol(rune_core::symbol::SYM_ITERATOR), 3),
    ];
    let key_names = vec![
        "next".to_string(),
        "\u{0}".to_string(),
        tag.to_string(),
        "\u{0}".to_string(),
    ];
    let shape = Shape::intern(keys, key_names);
    let tag_str = HeapString::allocate(gc, tag) as *mut u8;
    let vals = vec![
        next_h,
        Value::from_heap_ptr(state as *mut u8),
        Value::from_heap_ptr(tag_str),
        iter_h,
    ];
    let obj_ptr = JSObject::allocate(gc, shape, &vals);
    if vm.object_prototype.is_heap_object() {
        if let Some(pp) = vm.object_prototype.heap_ptr() {
            unsafe { JSObject::set_prototype(obj_ptr, pp) };
        }
    }
    Value::from_heap_ptr(obj_ptr as *mut u8)
}

/// Iterator result object: `{ value, done }` (§7.4.7 CreateIterResultObject).
fn make_iter_result(gc: &mut SemiSpace, value: Value, done: bool) -> Value {
    let keys = vec![
        (PropertyKey::from_string("value"), 0),
        (PropertyKey::from_string("done"), 1),
    ];
    let key_names = vec!["value".to_string(), "done".to_string()];
    let shape = Shape::intern(keys, key_names);
    let vals = vec![value, Value::boolean(done)];
    Value::from_heap_ptr(JSObject::allocate(gc, shape, &vals) as *mut u8)
}

/// %IteratorPrototype%[@@iterator] — returns the iterator itself (§7.4.1.2.1).
pub fn iterator_prototype_symbol_iterator(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    _vm: &mut Vm,
) -> Value {
    let _ = gc;
    this
}

/// Array.prototype.values — iterator over element values (kind 2).
pub fn array_values_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let ok = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_ARRAY });
    if !ok {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Array.prototype.values requires an array receiver",
        )));
        return Value::undefined();
    }
    make_iterator_object(
        gc,
        vm,
        "Array_iterator_next",
        &[this, Value::smi(0), Value::smi(2)],
        "Array Iterator",
    )
}

/// Array.prototype.keys — iterator over indices (kind 1).
pub fn array_keys_builtin(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    let ok = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_ARRAY });
    if !ok {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Array.prototype.keys requires an array receiver",
        )));
        return Value::undefined();
    }
    make_iterator_object(
        gc,
        vm,
        "Array_iterator_next",
        &[this, Value::smi(0), Value::smi(1)],
        "Array Iterator",
    )
}

/// Array.prototype.entries — iterator over [index, value] pairs (kind 0).
pub fn array_entries_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let ok = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_ARRAY });
    if !ok {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Array.prototype.entries requires an array receiver",
        )));
        return Value::undefined();
    }
    make_iterator_object(
        gc,
        vm,
        "Array_iterator_next",
        &[this, Value::smi(0), Value::smi(0)],
        "Array Iterator",
    )
}

/// The shared next() for array iterators — reads the hidden state
/// [iterated array, index, kind] stored on the iterator object.
pub fn array_iterator_next(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_OBJECT {
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            if let Some(slot) = shape.lookup(&PropertyKey::from_symbol(vm.iter_state_symbol)) {
                let state_val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                if let Some(state_ptr) = state_val.heap_ptr() {
                    let state = state_ptr as *mut RuneArray;
                    let arr_val = unsafe { RuneArray::get_element(state, 0) };
                    let index = unsafe { RuneArray::get_element(state, 1) }
                        .as_smi()
                        .unwrap_or(0) as usize;
                    let kind = unsafe { RuneArray::get_element(state, 2) }
                        .as_smi()
                        .unwrap_or(2) as usize;
                    if let Some(arr_ptr) = arr_val.heap_ptr() {
                        let arr_tag = unsafe { (*(arr_ptr as *const GcHeader)).tag() };
                        if arr_tag == TAG_ARRAY || arr_tag == TAG_TYPED_ARRAY {
                            let len = if arr_tag == TAG_ARRAY {
                                (unsafe { RuneArray::length(arr_ptr as *mut RuneArray) }) as usize
                            } else {
                                unsafe { typedarray::RuneTypedArray::length(arr_ptr) }
                            };
                            if index >= len {
                                unsafe {
                                    RuneArray::set_element(state, 0, Value::undefined());
                                }
                                return make_iter_result(gc, Value::undefined(), true);
                            }
                            let value = if arr_tag == TAG_ARRAY {
                                unsafe { RuneArray::get_element(arr_ptr as *mut RuneArray, index) }
                            } else {
                                unsafe { typedarray::read_element(arr_ptr, index) }
                            };
                            unsafe {
                                RuneArray::set_element(state, 1, Value::smi((index + 1) as i32));
                            }
                            let out = match kind {
                                0 => {
                                    let pair = crate::vm::new_dense_array(vm, gc);
                                    let pair2 = unsafe {
                                        RuneArray::push(
                                            gc,
                                            pair as *mut RuneArray,
                                            Value::smi(index as i32),
                                        )
                                    };
                                    let pair3 = unsafe { RuneArray::push(gc, pair2, value) };
                                    Value::from_heap_ptr(pair3 as *mut u8)
                                }
                                1 => Value::smi(index as i32),
                                _ => value,
                            };
                            return make_iter_result(gc, out, false);
                        }
                    }
                }
            }
        }
    }
    make_iter_result(gc, Value::undefined(), true)
}

/// String.prototype[Symbol.iterator] — code point iterator over `this`.
pub fn string_iterator_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let str_ptr = if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            ptr
        } else if tag == TAG_STRING_OBJ {
            unsafe { StringObject::string_ptr(ptr as *mut StringObject) }
        } else {
            vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                gc,
                "TypeError: String.prototype[Symbol.iterator] requires a string receiver",
            )));
            return Value::undefined();
        }
    } else {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: String.prototype[Symbol.iterator] requires a string receiver",
        )));
        return Value::undefined();
    };
    make_iterator_object(
        gc,
        vm,
        "String_iterator_next",
        &[Value::from_heap_ptr(str_ptr), Value::smi(0)],
        "String Iterator",
    )
}

/// The next() for string iterators — yields one code point per step,
/// advancing by UTF-16 code units (surrogate pairs count as 2).
pub fn string_iterator_next(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_OBJECT {
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            if let Some(slot) = shape.lookup(&PropertyKey::from_symbol(vm.iter_state_symbol)) {
                let state_val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                if let Some(state_ptr) = state_val.heap_ptr() {
                    let state = state_ptr as *mut RuneArray;
                    let str_val = unsafe { RuneArray::get_element(state, 0) };
                    let index = unsafe { RuneArray::get_element(state, 1) }
                        .as_smi()
                        .unwrap_or(0) as usize;
                    if let Some(str_ptr) = str_val.heap_ptr() {
                        if unsafe { (*(str_ptr as *const GcHeader)).tag() } == TAG_STRING {
                            let s = unsafe { HeapString::to_string(str_ptr as *mut HeapString) };
                            let mut pos = 0usize;
                            for ch in s.chars() {
                                let width = if (ch as u32) > 0xFFFF { 2 } else { 1 };
                                if pos == index {
                                    let sp = HeapString::allocate(gc, &ch.to_string());
                                    unsafe {
                                        RuneArray::set_element(
                                            state,
                                            1,
                                            Value::smi((index + width) as i32),
                                        );
                                    }
                                    return make_iter_result(
                                        gc,
                                        Value::from_heap_ptr(sp as *mut u8),
                                        false,
                                    );
                                }
                                pos += width;
                            }
                            // Past the end — mark the iterator done.
                            unsafe {
                                RuneArray::set_element(state, 0, Value::undefined());
                            }
                            return make_iter_result(gc, Value::undefined(), true);
                        }
                    }
                }
            }
        }
    }
    make_iter_result(gc, Value::undefined(), true)
}

// ── Map / Set builtins ────────────────────────────────────────────────

/// SameValueZero comparison used for Map/Set keys (§7.2.12 SameValueZero):
/// - NaN matches NaN; +0 and -0 are equal
/// - Smi and float64 encodings of the same number are equal
/// - Heap strings compare by content, not pointer identity
/// - Symbols and objects compare by identity
pub(crate) fn map_key_equal(a: Value, b: Value) -> bool {
    let a_num = a.is_smi() || a.is_float64();
    let b_num = b.is_smi() || b.is_float64();
    if a_num || b_num {
        if !(a_num && b_num) {
            return false;
        }
        let fa = if a.is_smi() {
            a.as_smi().unwrap() as f64
        } else {
            f64::from_bits(a.raw())
        };
        let fb = if b.is_smi() {
            b.as_smi().unwrap() as f64
        } else {
            f64::from_bits(b.raw())
        };
        if fa.is_nan() || fb.is_nan() {
            return fa.is_nan() && fb.is_nan();
        }
        return fa == fb;
    }
    if a.raw() == b.raw() {
        return true;
    }
    if let (Some(pa), Some(pb)) = (a.heap_ptr(), b.heap_ptr()) {
        let ta = unsafe { (*(pa as *const GcHeader)).tag() };
        let tb = unsafe { (*(pb as *const GcHeader)).tag() };
        if ta == TAG_STRING && tb == TAG_STRING {
            return unsafe { HeapString::to_string(pa as *mut HeapString) }
                == unsafe { HeapString::to_string(pb as *mut HeapString) };
        }
    }
    false
}

/// §7.2.14 IsObject: true for everything except primitives. The GC-tagged
/// string and legacy float64 boxes are primitives, not Objects.
pub(crate) fn is_object_value(v: Value) -> bool {
    if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        tag != TAG_STRING && tag != TAG_FLOAT64 && tag != TAG_FORWARDED
    } else {
        false
    }
}

/// Index of the entry whose key equals `key`, or None. Non-allocating.
/// Map entry lists are flat [k0, v0, k1, v1, ...]; a deleted entry has its
/// key slot set to `Value::empty_sentinel()`. Set lists are flat values with
/// the same sentinel marking deletion.
pub(crate) fn key_index(entries_ptr: *mut u8, key: Value, is_map: bool) -> Option<usize> {
    if entries_ptr.is_null() {
        return None;
    }
    let entries = entries_ptr as *mut RuneArray;
    let len = unsafe { RuneArray::length(entries) } as usize;
    let empty = Value::empty_sentinel().raw();
    if is_map {
        let mut i = 0;
        while i < len {
            let k = unsafe { RuneArray::get_element(entries, i) };
            if k.raw() != empty && map_key_equal(k, key) {
                return Some(i);
            }
            i += 2;
        }
    } else {
        for i in 0..len {
            let k = unsafe { RuneArray::get_element(entries, i) };
            if k.raw() != empty && map_key_equal(k, key) {
                return Some(i);
            }
        }
    }
    None
}

/// Set `map[key] = value` (§27.1.3.15 Map.prototype.set).
/// `map_slot` must reference a GC-rooted slot (the VM stack or a pending
/// state field) — the map pointer is re-read after every allocation.
/// Returns true if a new entry was appended (size grew).
pub(crate) fn map_set_internal(
    gc: &mut SemiSpace,
    map_slot: &mut Value,
    key: Value,
    value: Value,
) -> bool {
    let mut map_ptr = map_slot.heap_ptr().unwrap();
    if let Some(i) = key_index(unsafe { RuneMap::entries(map_ptr) }, key, true) {
        let entries = unsafe { RuneMap::entries(map_ptr) } as *mut RuneArray;
        unsafe { RuneArray::set_element(entries, i + 1, value) };
        return false;
    }
    let mut entries_ptr = unsafe { RuneMap::entries(map_ptr) };
    if entries_ptr.is_null() {
        entries_ptr = RuneArray::allocate(gc, &[]) as *mut u8;
        map_ptr = map_slot.heap_ptr().unwrap();
        unsafe { RuneMap::set_entries(map_ptr, entries_ptr) };
    }
    let entries = unsafe { RuneArray::push(gc, entries_ptr as *mut RuneArray, key) };
    let entries = unsafe { RuneArray::push(gc, entries, value) };
    map_ptr = map_slot.heap_ptr().unwrap();
    unsafe { RuneMap::set_entries(map_ptr, entries as *mut u8) };
    unsafe { RuneMap::set_size(map_ptr, RuneMap::size(map_ptr) + 1) };
    true
}

/// Add `value` to a Set (§27.2.3.1 Set.prototype.add). Slot rules as above.
/// Returns true if a new element was appended (size grew).
pub(crate) fn set_add_internal(gc: &mut SemiSpace, set_slot: &mut Value, value: Value) -> bool {
    let mut set_ptr = set_slot.heap_ptr().unwrap();
    if key_index(unsafe { RuneSet::entries(set_ptr) }, value, false).is_some() {
        return false;
    }
    let mut entries_ptr = unsafe { RuneSet::entries(set_ptr) };
    if entries_ptr.is_null() {
        entries_ptr = RuneArray::allocate(gc, &[]) as *mut u8;
        set_ptr = set_slot.heap_ptr().unwrap();
        unsafe { RuneSet::set_entries(set_ptr, entries_ptr) };
    }
    let entries = unsafe { RuneArray::push(gc, entries_ptr as *mut RuneArray, value) };
    set_ptr = set_slot.heap_ptr().unwrap();
    unsafe { RuneSet::set_entries(set_ptr, entries as *mut u8) };
    unsafe { RuneSet::set_size(set_ptr, RuneSet::size(set_ptr) + 1) };
    true
}

fn map_receiver(gc: &mut SemiSpace, this: Value, vm: &mut Vm) -> Option<*mut u8> {
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_MAP {
            return Some(ptr);
        }
    }
    vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
        gc,
        "TypeError: Map.prototype method called on incompatible receiver",
    )));
    None
}

fn set_receiver(gc: &mut SemiSpace, this: Value, vm: &mut Vm) -> Option<*mut u8> {
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_SET {
            return Some(ptr);
        }
    }
    vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
        gc,
        "TypeError: Set.prototype method called on incompatible receiver",
    )));
    None
}

fn is_callable_value(v: Value) -> bool {
    v.as_smi().is_some_and(|s| s < 0)
        || v.heap_ptr()
            .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_FUNC })
}

/// §27.1.3.15 Map.prototype.set
pub fn map_set_builtin(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some(_map_ptr) = map_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let key = args.first().copied().unwrap_or(Value::undefined());
    let value = args.get(1).copied().unwrap_or(Value::undefined());
    let mut slot = this;
    map_set_internal(gc, &mut slot, key, value);
    this
}

/// §27.1.3.8 Map.prototype.get
pub fn map_get_builtin(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some(map_ptr) = map_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let key = args.first().copied().unwrap_or(Value::undefined());
    if let Some(i) = key_index(unsafe { RuneMap::entries(map_ptr) }, key, true) {
        let entries = unsafe { RuneMap::entries(map_ptr) } as *mut RuneArray;
        return unsafe { RuneArray::get_element(entries, i + 1) };
    }
    Value::undefined()
}

/// §27.1.3.10 Map.prototype.has
pub fn map_has_builtin(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some(map_ptr) = map_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let key = args.first().copied().unwrap_or(Value::undefined());
    Value::boolean(key_index(unsafe { RuneMap::entries(map_ptr) }, key, true).is_some())
}

/// §27.1.3.4 Map.prototype.delete — removes the entry, returns true if present.
pub fn map_delete_builtin(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some(map_ptr) = map_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let key = args.first().copied().unwrap_or(Value::undefined());
    let entries_ptr = unsafe { RuneMap::entries(map_ptr) };
    if let Some(i) = key_index(entries_ptr, key, true) {
        let entries = entries_ptr as *mut RuneArray;
        unsafe {
            RuneArray::set_element(entries, i, Value::empty_sentinel());
            RuneArray::set_element(entries, i + 1, Value::undefined());
        }
        unsafe { RuneMap::set_size(map_ptr, RuneMap::size(map_ptr) - 1) };
        return Value::boolean(true);
    }
    Value::boolean(false)
}

/// §27.1.3.2 Map.prototype.clear — empties the map (the entry list itself is
/// retained so suspended iterators keep their snapshot semantics).
pub fn map_clear_builtin(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    let Some(map_ptr) = map_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let entries_ptr = unsafe { RuneMap::entries(map_ptr) };
    if !entries_ptr.is_null() {
        let entries = entries_ptr as *mut RuneArray;
        let len = unsafe { RuneArray::length(entries) } as usize;
        for i in 0..len {
            unsafe { RuneArray::set_element(entries, i, Value::empty_sentinel()) };
        }
    }
    unsafe { RuneMap::set_size(map_ptr, 0) };
    Value::undefined()
}

/// §27.2.3.1 Set.prototype.add
pub fn set_add_builtin(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some(_set_ptr) = set_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let value = args.first().copied().unwrap_or(Value::undefined());
    let mut slot = this;
    set_add_internal(gc, &mut slot, value);
    this
}

/// §27.2.3.9 Set.prototype.has
pub fn set_has_builtin(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some(set_ptr) = set_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let value = args.first().copied().unwrap_or(Value::undefined());
    Value::boolean(key_index(unsafe { RuneSet::entries(set_ptr) }, value, false).is_some())
}

/// §27.2.3.3 Set.prototype.delete — removes the element, returns true if present.
pub fn set_delete_builtin(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some(set_ptr) = set_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let value = args.first().copied().unwrap_or(Value::undefined());
    let entries_ptr = unsafe { RuneSet::entries(set_ptr) };
    if let Some(i) = key_index(entries_ptr, value, false) {
        let entries = entries_ptr as *mut RuneArray;
        unsafe { RuneArray::set_element(entries, i, Value::empty_sentinel()) };
        unsafe { RuneSet::set_size(set_ptr, RuneSet::size(set_ptr) - 1) };
        return Value::boolean(true);
    }
    Value::boolean(false)
}

/// §27.2.3.2 Set.prototype.clear
pub fn set_clear_builtin(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    let Some(set_ptr) = set_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let entries_ptr = unsafe { RuneSet::entries(set_ptr) };
    if !entries_ptr.is_null() {
        let entries = entries_ptr as *mut RuneArray;
        let len = unsafe { RuneArray::length(entries) } as usize;
        for i in 0..len {
            unsafe { RuneArray::set_element(entries, i, Value::empty_sentinel()) };
        }
    }
    unsafe { RuneSet::set_size(set_ptr, 0) };
    Value::undefined()
}

/// Create a [key, value] (kind 0), key (kind 1) or value (kind 2) iterator.
fn make_collection_iterator(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    collection: Value,
    kind: i32,
    next_handle: &str,
    tag: &str,
) -> Value {
    make_iterator_object(
        gc,
        vm,
        next_handle,
        &[collection, Value::smi(0), Value::smi(kind)],
        tag,
    )
}

/// §27.1.3.5 Map.prototype.entries / keys / values
pub fn map_entries_builtin(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    let Some(_map_ptr) = map_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    make_collection_iterator(gc, vm, this, 0, "Map_iterator_next", "Map Iterator")
}

pub fn map_keys_builtin(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    let Some(_map_ptr) = map_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    make_collection_iterator(gc, vm, this, 1, "Map_iterator_next", "Map Iterator")
}

pub fn map_values_builtin(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    let Some(_map_ptr) = map_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    make_collection_iterator(gc, vm, this, 2, "Map_iterator_next", "Map Iterator")
}

/// §27.2.3.5 Set.prototype.entries (yields [v, v]) / keys / values
pub fn set_entries_builtin(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    let Some(_set_ptr) = set_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    make_collection_iterator(gc, vm, this, 0, "Set_iterator_next", "Set Iterator")
}

pub fn set_keys_builtin(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    let Some(_set_ptr) = set_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    make_collection_iterator(gc, vm, this, 1, "Set_iterator_next", "Set Iterator")
}

pub fn set_values_builtin(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    let Some(_set_ptr) = set_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    make_collection_iterator(gc, vm, this, 2, "Set_iterator_next", "Set Iterator")
}

/// Shared next() for map iterators — state [map, raw index, kind].
/// Skips deleted (sentinel) entries; done once the raw list is exhausted.
pub fn map_iterator_next(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_OBJECT {
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            if let Some(slot) = shape.lookup(&PropertyKey::from_symbol(vm.iter_state_symbol)) {
                let state_val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                if let Some(state_ptr) = state_val.heap_ptr() {
                    let state = state_ptr as *mut RuneArray;
                    let map_val = unsafe { RuneArray::get_element(state, 0) };
                    let index = unsafe { RuneArray::get_element(state, 1) }
                        .as_smi()
                        .unwrap_or(0) as usize;
                    let kind = unsafe { RuneArray::get_element(state, 2) }
                        .as_smi()
                        .unwrap_or(0) as usize;
                    if let Some(map_ptr) = map_val.heap_ptr() {
                        if unsafe { (*(map_ptr as *const GcHeader)).tag() } == TAG_MAP {
                            let entries_ptr = unsafe { RuneMap::entries(map_ptr) };
                            let len = if entries_ptr.is_null() {
                                0
                            } else {
                                (unsafe { RuneArray::length(entries_ptr as *mut RuneArray) })
                                    as usize
                            };
                            let mut i = index;
                            while i < len {
                                let entries = entries_ptr as *mut RuneArray;
                                let k = unsafe { RuneArray::get_element(entries, i) };
                                if k.raw() != Value::empty_sentinel().raw() {
                                    let v = unsafe { RuneArray::get_element(entries, i + 1) };
                                    unsafe {
                                        RuneArray::set_element(state, 1, Value::smi((i + 2) as i32))
                                    };
                                    let out = match kind {
                                        1 => k,
                                        2 => v,
                                        _ => {
                                            let pair = crate::vm::new_dense_array(vm, gc);
                                            let pair2 = unsafe {
                                                RuneArray::push(gc, pair as *mut RuneArray, k)
                                            };
                                            let pair3 = unsafe { RuneArray::push(gc, pair2, v) };
                                            Value::from_heap_ptr(pair3 as *mut u8)
                                        }
                                    };
                                    return make_iter_result(gc, out, false);
                                }
                                i += 2;
                            }
                            unsafe { RuneArray::set_element(state, 0, Value::undefined()) };
                            return make_iter_result(gc, Value::undefined(), true);
                        }
                    }
                }
            }
        }
    }
    make_iter_result(gc, Value::undefined(), true)
}

/// Shared next() for set iterators — state [set, raw index, kind].
pub fn set_iterator_next(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_OBJECT {
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            if let Some(slot) = shape.lookup(&PropertyKey::from_symbol(vm.iter_state_symbol)) {
                let state_val = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                if let Some(state_ptr) = state_val.heap_ptr() {
                    let state = state_ptr as *mut RuneArray;
                    let set_val = unsafe { RuneArray::get_element(state, 0) };
                    let index = unsafe { RuneArray::get_element(state, 1) }
                        .as_smi()
                        .unwrap_or(0) as usize;
                    let kind = unsafe { RuneArray::get_element(state, 2) }
                        .as_smi()
                        .unwrap_or(0) as usize;
                    if let Some(set_ptr) = set_val.heap_ptr() {
                        if unsafe { (*(set_ptr as *const GcHeader)).tag() } == TAG_SET {
                            let entries_ptr = unsafe { RuneSet::entries(set_ptr) };
                            let len = if entries_ptr.is_null() {
                                0
                            } else {
                                (unsafe { RuneArray::length(entries_ptr as *mut RuneArray) })
                                    as usize
                            };
                            let mut i = index;
                            while i < len {
                                let entries = entries_ptr as *mut RuneArray;
                                let v = unsafe { RuneArray::get_element(entries, i) };
                                if v.raw() != Value::empty_sentinel().raw() {
                                    unsafe {
                                        RuneArray::set_element(state, 1, Value::smi((i + 1) as i32))
                                    };
                                    let out = match kind {
                                        0 => {
                                            let pair = crate::vm::new_dense_array(vm, gc);
                                            let pair2 = unsafe {
                                                RuneArray::push(gc, pair as *mut RuneArray, v)
                                            };
                                            let pair3 = unsafe { RuneArray::push(gc, pair2, v) };
                                            Value::from_heap_ptr(pair3 as *mut u8)
                                        }
                                        _ => v,
                                    };
                                    return make_iter_result(gc, out, false);
                                }
                                i += 1;
                            }
                            unsafe { RuneArray::set_element(state, 0, Value::undefined()) };
                            return make_iter_result(gc, Value::undefined(), true);
                        }
                    }
                }
            }
        }
    }
    make_iter_result(gc, Value::undefined(), true)
}

// ---------------------------------------------------------------------------
// Date — §21.4. UTC-only time zone (spec-conformant default: local time
// equals UTC). The engine's ToPrimitive does not dispatch @@toPrimitive, so
// Date "default" hint → string is handled by special-casing TAG_DATE in
// to_primitive_string / to_number / value_to_js_string.
// ---------------------------------------------------------------------------

/// §21.4.2.1 Date ( ...values ) — constructor called with `new`.
/// The freshly allocated RuneDate is passed as `this`; computes the time value
/// and stores it. Synchronous only (no pending state machine): object args
/// use the sync ToPrimitive path, matching the engine's existing simplifications.
pub fn date_constructor(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let tv = match args.len() {
        0 => date::now_ms(),
        1 => {
            let v = args[0];
            if let Some(ptr) = v.heap_ptr() {
                let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                if tag == TAG_DATE {
                    // Copy the [[DateValue]] of another Date.
                    unsafe { date::RuneDate::tv(ptr) }
                } else if tag == TAG_STRING {
                    let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
                    date::time_clip(date::parse_date_string(&s))
                } else if tag == TAG_STRING_OBJ {
                    let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
                    let s = unsafe { HeapString::to_string(str_ptr as *mut HeapString) };
                    date::time_clip(date::parse_date_string(&s))
                } else if tag == TAG_ARRAY {
                    let s = array_to_string(ptr as *mut RuneArray);
                    date::time_clip(date::parse_date_string(&s))
                } else if tag == TAG_OBJECT {
                    // ToPrimitive (default hint) via the sync path, then parse.
                    let s = to_primitive_string_sync(v, gc, vm);
                    date::time_clip(date::parse_date_string(&s))
                } else {
                    date::time_clip(to_number(v))
                }
            } else if v.is_symbol() {
                // ToNumber(symbol) should throw TypeError; known gap (NaN).
                f64::NAN
            } else {
                date::time_clip(to_number(v))
            }
        }
        _ => {
            let y = to_number(args[0]);
            let m = to_number(args.get(1).copied().unwrap_or(Value::smi(0)));
            let dt = match args.get(2) {
                Some(x) => to_number(*x),
                None => 1.0,
            };
            let h = match args.get(3) {
                Some(x) => to_number(*x),
                None => 0.0,
            };
            let min = match args.get(4) {
                Some(x) => to_number(*x),
                None => 0.0,
            };
            let sec = match args.get(5) {
                Some(x) => to_number(*x),
                None => 0.0,
            };
            let ms = match args.get(6) {
                Some(x) => to_number(*x),
                None => 0.0,
            };
            let yr = date::make_full_year(y);
            let final_date =
                date::make_date(date::make_day(yr, m, dt), date::make_time(h, min, sec, ms));
            // UTC(t) = t in the UTC-only implementation.
            date::time_clip(final_date)
        }
    };
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_DATE {
            unsafe { date::RuneDate::set_tv(ptr, tv) };
        }
    }
    this
}

/// §21.4.3.1 Date.now ( )
pub fn date_now_builtin(_gc: &mut SemiSpace, _this: Value, _args: &[Value], _vm: &mut Vm) -> Value {
    date_number(date::now_ms())
}

/// §21.4.3.2 Date.parse ( string )
pub fn date_parse_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let s = match args.first().copied() {
        Some(v) => to_primitive_string_sync(v, gc, _vm),
        None => String::new(),
    };
    date_number(date::parse_date_string(&s))
}

/// §21.4.3.4 Date.UTC ( year [ , month [ , date [ , hours [ , minutes [ , seconds [ , ms ] ] ] ] ] ] )
pub fn date_utc_builtin(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    if args.is_empty() {
        return date_number(f64::NAN);
    }
    let y = to_number(args[0]);
    let m = args.get(1).map_or(0.0, |x| to_number(*x));
    let dt = args.get(2).map_or(1.0, |x| to_number(*x));
    let h = args.get(3).map_or(0.0, |x| to_number(*x));
    let min = args.get(4).map_or(0.0, |x| to_number(*x));
    let sec = args.get(5).map_or(0.0, |x| to_number(*x));
    let ms = args.get(6).map_or(0.0, |x| to_number(*x));
    let yr = date::make_full_year(y);
    date_number(date::time_clip(date::make_date(
        date::make_day(yr, m, dt),
        date::make_time(h, min, sec, ms),
    )))
}

fn date_receiver(gc: &mut SemiSpace, this: Value, vm: &mut Vm) -> Option<*mut u8> {
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_DATE {
            return Some(ptr);
        }
    }
    vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
        gc,
        "TypeError: Date.prototype method called on incompatible receiver",
    )));
    None
}

/// Number result helper: Smi when integral and in range, else NaN-boxed f64.
fn date_number(v: f64) -> Value {
    if v.is_nan() || v.is_infinite() {
        return Value::from_float64(v);
    }
    if v.fract() == 0.0 {
        if v == 0.0 && v.is_sign_negative() {
            return Value::from_float64(v);
        }
        let i = v as i64;
        if i32::try_from(i).is_ok() {
            return Value::smi(i as i32);
        }
    }
    Value::from_float64(v)
}

macro_rules! date_getter {
    ($name:ident, $doc:expr, $expr:expr) => {
        /// $doc
        pub fn $name(gc: &mut SemiSpace, this: Value, _args: &[Value], vm: &mut Vm) -> Value {
            let Some(ptr) = date_receiver(gc, this, vm) else {
                return Value::undefined();
            };
            let tv = unsafe { date::RuneDate::tv(ptr) };
            if tv.is_nan() {
                return Value::from_float64(f64::NAN);
            }
            date_number($expr(tv) as f64)
        }
    };
}

date_getter!(
    date_get_date_builtin,
    "§21.4.4.2 Date.prototype.getDate",
    date::date_from_time
);
date_getter!(
    date_get_day_builtin,
    "§21.4.4.3 Date.prototype.getDay",
    date::week_day
);
date_getter!(
    date_get_full_year_builtin,
    "§21.4.4.4 Date.prototype.getFullYear",
    |tv| date::year_from_time(tv) as f64
);
date_getter!(
    date_get_hours_builtin,
    "§21.4.4.5 Date.prototype.getHours",
    |tv| date::hour_from_time(tv) as f64
);
date_getter!(
    date_get_milliseconds_builtin,
    "§21.4.4.6 Date.prototype.getMilliseconds",
    |tv| date::millisec_from_time(tv) as f64
);
date_getter!(
    date_get_minutes_builtin,
    "§21.4.4.7 Date.prototype.getMinutes",
    |tv| date::min_from_time(tv) as f64
);
date_getter!(
    date_get_month_builtin,
    "§21.4.4.8 Date.prototype.getMonth",
    |tv| date::month_from_time(tv) as f64
);
date_getter!(
    date_get_seconds_builtin,
    "§21.4.4.9 Date.prototype.getSeconds",
    |tv| date::sec_from_time(tv) as f64
);
date_getter!(
    date_get_utc_date_builtin,
    "§21.4.4.12 Date.prototype.getUTCDate",
    date::date_from_time
);
date_getter!(
    date_get_utc_day_builtin,
    "§21.4.4.13 Date.prototype.getUTCDay",
    date::week_day
);
date_getter!(
    date_get_utc_full_year_builtin,
    "§21.4.4.14 Date.prototype.getUTCFullYear",
    |tv| date::year_from_time(tv) as f64
);
date_getter!(
    date_get_utc_hours_builtin,
    "§21.4.4.15 Date.prototype.getUTCHours",
    |tv| date::hour_from_time(tv) as f64
);
date_getter!(
    date_get_utc_milliseconds_builtin,
    "§21.4.4.16 Date.prototype.getUTCMilliseconds",
    |tv| date::millisec_from_time(tv) as f64
);
date_getter!(
    date_get_utc_minutes_builtin,
    "§21.4.4.17 Date.prototype.getUTCMinutes",
    |tv| date::min_from_time(tv) as f64
);
date_getter!(
    date_get_utc_month_builtin,
    "§21.4.4.18 Date.prototype.getUTCMonth",
    |tv| date::month_from_time(tv) as f64
);
date_getter!(
    date_get_utc_seconds_builtin,
    "§21.4.4.19 Date.prototype.getUTCSeconds",
    |tv| date::sec_from_time(tv) as f64
);

/// §21.4.4.10 Date.prototype.getTime
pub fn date_get_time_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    date_number(unsafe { date::RuneDate::tv(ptr) })
}

/// §21.4.4.11 Date.prototype.getTimezoneOffset — 0 in the UTC-only implementation.
pub fn date_get_timezone_offset_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    Value::smi(0)
}

/// §21.4.4.44 Date.prototype.valueOf
pub fn date_value_of_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    date_number(unsafe { date::RuneDate::tv(ptr) })
}

/// §21.4.4.41 Date.prototype.toString
pub fn date_to_string_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let s = date::to_date_string(unsafe { date::RuneDate::tv(ptr) });
    Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8)
}

/// §21.4.4.35 Date.prototype.toDateString
pub fn date_to_date_string_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    if tv.is_nan() {
        return Value::from_heap_ptr(HeapString::allocate(gc, "Invalid Date") as *mut u8);
    }
    let s = date::date_string(tv);
    Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8)
}

/// §21.4.4.42 Date.prototype.toTimeString
pub fn date_to_time_string_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    if tv.is_nan() {
        return Value::from_heap_ptr(HeapString::allocate(gc, "Invalid Date") as *mut u8);
    }
    let s = format!("{}{}", date::time_string(tv), date::time_zone_string(tv));
    Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8)
}

/// §21.4.4.43 Date.prototype.toUTCString
pub fn date_to_utc_string_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    if tv.is_nan() {
        return Value::from_heap_ptr(HeapString::allocate(gc, "Invalid Date") as *mut u8);
    }
    let yv = date::year_from_time(tv);
    let year_sign = if yv >= 0 { "" } else { "-" };
    let s = format!(
        "{}, {} {} {}{} {}",
        date::weekday_name(date::week_day(tv)),
        date::zero_padded(date::date_from_time(tv), 2),
        date::month_name(date::month_from_time(tv)),
        year_sign,
        date::zero_padded(yv, 4),
        date::time_string(tv)
    );
    Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8)
}

/// §21.4.4.36 Date.prototype.toISOString
pub fn date_to_iso_string_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    match date::to_iso_string(tv) {
        Some(s) => Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8),
        None => {
            // §21.4.4.36: throw a RangeError for NaN or unrepresentable years.
            vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                gc,
                "RangeError: Invalid time value",
            )));
            Value::undefined()
        }
    }
}

/// §21.4.4.37 Date.prototype.toJSON ( key )
pub fn date_to_json_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    // Generic per spec; this implementation handles Date receivers and
    // non-finite primitive coercions (objects without a toISOString are a gap).
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_DATE {
            let tv = unsafe { date::RuneDate::tv(ptr) };
            if tv.is_nan() || tv.is_infinite() {
                return Value::null();
            }
            return date_to_iso_string_builtin(gc, this, _args, vm);
        }
    }
    Value::null()
}

/// §21.4.4.38-40 locale methods — implementation-defined without ECMA-402.
pub fn date_to_locale_string_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    date_to_string_builtin(gc, this, _args, vm)
}

pub fn date_to_locale_date_string_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    date_to_date_string_builtin(gc, this, _args, vm)
}

pub fn date_to_locale_time_string_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    date_to_time_string_builtin(gc, this, _args, vm)
}

/// §21.4.4.20 Date.prototype.setDate ( date )
pub fn date_set_date_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let dt = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let new_date = date::make_date(
        date::make_day(
            date::year_from_time(tv) as f64,
            date::month_from_time(tv) as f64,
            dt,
        ),
        date::time_within_day(tv),
    );
    let u = date::time_clip(new_date);
    unsafe { date::RuneDate::set_tv(ptr, u) };
    date_number(u)
}

/// §21.4.4.21 Date.prototype.setFullYear ( year [ , month [ , date ] ] )
pub fn date_set_full_year_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let y = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    let base = if tv.is_nan() { 0.0 } else { tv };
    let m = args
        .get(1)
        .map_or(date::month_from_time(base) as f64, |x| to_number(*x));
    let dt = args
        .get(2)
        .map_or(date::date_from_time(base) as f64, |x| to_number(*x));
    let yr = date::make_full_year(y);
    let new_date = date::make_date(date::make_day(yr, m, dt), date::time_within_day(base));
    let u = date::time_clip(new_date);
    unsafe { date::RuneDate::set_tv(ptr, u) };
    date_number(u)
}

/// §21.4.4.22 Date.prototype.setHours ( hour [ , min [ , sec [ , ms ] ] ] )
pub fn date_set_hours_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let h = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let m = args
        .get(1)
        .map_or(date::min_from_time(tv) as f64, |x| to_number(*x));
    let s = args
        .get(2)
        .map_or(date::sec_from_time(tv) as f64, |x| to_number(*x));
    let ms = args
        .get(3)
        .map_or(date::millisec_from_time(tv) as f64, |x| to_number(*x));
    let u = date::time_clip(date::make_date(
        date::day(tv) as f64,
        date::make_time(h, m, s, ms),
    ));
    unsafe { date::RuneDate::set_tv(ptr, u) };
    date_number(u)
}

/// §21.4.4.23 Date.prototype.setMilliseconds ( ms )
pub fn date_set_milliseconds_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let ms = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let u = date::time_clip(date::make_date(
        date::day(tv) as f64,
        date::make_time(
            date::hour_from_time(tv) as f64,
            date::min_from_time(tv) as f64,
            date::sec_from_time(tv) as f64,
            ms,
        ),
    ));
    unsafe { date::RuneDate::set_tv(ptr, u) };
    date_number(u)
}

/// §21.4.4.24 Date.prototype.setMinutes ( min [ , sec [ , ms ] ] )
pub fn date_set_minutes_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let m = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let s = args
        .get(1)
        .map_or(date::sec_from_time(tv) as f64, |x| to_number(*x));
    let ms = args
        .get(2)
        .map_or(date::millisec_from_time(tv) as f64, |x| to_number(*x));
    let u = date::time_clip(date::make_date(
        date::day(tv) as f64,
        date::make_time(date::hour_from_time(tv) as f64, m, s, ms),
    ));
    unsafe { date::RuneDate::set_tv(ptr, u) };
    date_number(u)
}

/// §21.4.4.25 Date.prototype.setMonth ( month [ , date ] )
pub fn date_set_month_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let m = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let dt = args
        .get(1)
        .map_or(date::date_from_time(tv) as f64, |x| to_number(*x));
    let new_date = date::make_date(
        date::make_day(date::year_from_time(tv) as f64, m, dt),
        date::time_within_day(tv),
    );
    let u = date::time_clip(new_date);
    unsafe { date::RuneDate::set_tv(ptr, u) };
    date_number(u)
}

/// §21.4.4.26 Date.prototype.setSeconds ( sec [ , ms ] )
pub fn date_set_seconds_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let s = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let ms = args
        .get(1)
        .map_or(date::millisec_from_time(tv) as f64, |x| to_number(*x));
    let u = date::time_clip(date::make_date(
        date::day(tv) as f64,
        date::make_time(
            date::hour_from_time(tv) as f64,
            date::min_from_time(tv) as f64,
            s,
            ms,
        ),
    ));
    unsafe { date::RuneDate::set_tv(ptr, u) };
    date_number(u)
}

/// §21.4.4.27 Date.prototype.setTime ( time )
pub fn date_set_time_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let t = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    let v = date::time_clip(t);
    unsafe { date::RuneDate::set_tv(ptr, v) };
    date_number(v)
}

/// §21.4.4.28 Date.prototype.setUTCDate ( date )
pub fn date_set_utc_date_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let dt = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let v = date::time_clip(date::make_date(
        date::make_day(
            date::year_from_time(tv) as f64,
            date::month_from_time(tv) as f64,
            dt,
        ),
        date::time_within_day(tv),
    ));
    unsafe { date::RuneDate::set_tv(ptr, v) };
    date_number(v)
}

/// §21.4.4.29 Date.prototype.setUTCFullYear ( year [ , month [ , date ] ] )
pub fn date_set_utc_full_year_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let base = if tv.is_nan() { 0.0 } else { tv };
    let y = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    let m = args
        .get(1)
        .map_or(date::month_from_time(base) as f64, |x| to_number(*x));
    let dt = args
        .get(2)
        .map_or(date::date_from_time(base) as f64, |x| to_number(*x));
    let yr = date::make_full_year(y);
    let v = date::time_clip(date::make_date(
        date::make_day(yr, m, dt),
        date::time_within_day(base),
    ));
    unsafe { date::RuneDate::set_tv(ptr, v) };
    date_number(v)
}

/// §21.4.4.30 Date.prototype.setUTCHours ( hour [ , min [ , sec [ , ms ] ] ] )
pub fn date_set_utc_hours_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let h = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let m = args
        .get(1)
        .map_or(date::min_from_time(tv) as f64, |x| to_number(*x));
    let s = args
        .get(2)
        .map_or(date::sec_from_time(tv) as f64, |x| to_number(*x));
    let ms = args
        .get(3)
        .map_or(date::millisec_from_time(tv) as f64, |x| to_number(*x));
    let v = date::time_clip(date::make_date(
        date::day(tv) as f64,
        date::make_time(h, m, s, ms),
    ));
    unsafe { date::RuneDate::set_tv(ptr, v) };
    date_number(v)
}

/// §21.4.4.31 Date.prototype.setUTCMilliseconds ( ms )
pub fn date_set_utc_milliseconds_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let ms = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let v = date::time_clip(date::make_date(
        date::day(tv) as f64,
        date::make_time(
            date::hour_from_time(tv) as f64,
            date::min_from_time(tv) as f64,
            date::sec_from_time(tv) as f64,
            ms,
        ),
    ));
    unsafe { date::RuneDate::set_tv(ptr, v) };
    date_number(v)
}

/// §21.4.4.32 Date.prototype.setUTCMinutes ( min [ , sec [ , ms ] ] )
pub fn date_set_utc_minutes_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let m = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let s = args
        .get(1)
        .map_or(date::sec_from_time(tv) as f64, |x| to_number(*x));
    let ms = args
        .get(2)
        .map_or(date::millisec_from_time(tv) as f64, |x| to_number(*x));
    let v = date::time_clip(date::make_date(
        date::day(tv) as f64,
        date::make_time(date::hour_from_time(tv) as f64, m, s, ms),
    ));
    unsafe { date::RuneDate::set_tv(ptr, v) };
    date_number(v)
}

/// §21.4.4.33 Date.prototype.setUTCMonth ( month [ , date ] )
pub fn date_set_utc_month_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let m = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let dt = args
        .get(1)
        .map_or(date::date_from_time(tv) as f64, |x| to_number(*x));
    let v = date::time_clip(date::make_date(
        date::make_day(date::year_from_time(tv) as f64, m, dt),
        date::time_within_day(tv),
    ));
    unsafe { date::RuneDate::set_tv(ptr, v) };
    date_number(v)
}

/// §21.4.4.34 Date.prototype.setUTCSeconds ( sec [ , ms ] )
pub fn date_set_utc_seconds_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = date_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let tv = unsafe { date::RuneDate::tv(ptr) };
    let s = to_number(
        args.first()
            .copied()
            .unwrap_or(Value::from_float64(f64::NAN)),
    );
    if tv.is_nan() {
        return Value::from_float64(f64::NAN);
    }
    let ms = args
        .get(1)
        .map_or(date::millisec_from_time(tv) as f64, |x| to_number(*x));
    let v = date::time_clip(date::make_date(
        date::day(tv) as f64,
        date::make_time(
            date::hour_from_time(tv) as f64,
            date::min_from_time(tv) as f64,
            s,
            ms,
        ),
    ));
    unsafe { date::RuneDate::set_tv(ptr, v) };
    date_number(v)
}

/// §7.1.23 ToIndex — non-negative integer in [0, 2^53-1] or RangeError.
fn to_index_typed(gc: &mut SemiSpace, vm: &mut Vm, v: Value) -> Result<usize, ()> {
    let n = to_number(v);
    let i = if n.is_nan() || n == 0.0 {
        0.0
    } else if n.is_infinite() {
        f64::INFINITY
    } else {
        n.trunc()
    };
    if !(0.0..=9007199254740991.0).contains(&i) {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "RangeError: Invalid typed array length",
        )));
        return Err(());
    }
    Ok(i as usize)
}

/// §7.1.25 ToClampedIndex — negative relative to length, clamped to [0, length].
fn to_clamped_index(v: Value, length: usize) -> usize {
    let n = to_number(v);
    let i = if n.is_nan() || n == 0.0 {
        0
    } else if n.is_infinite() {
        if n > 0.0 { i64::MAX } else { i64::MIN }
    } else {
        n.trunc() as i64
    };
    let idx = if i < 0 { length as i64 + i } else { i };
    if idx < 0 {
        0
    } else if idx as usize > length {
        length
    } else {
        idx as usize
    }
}

/// §7.1.24 ToAbsoluteIndex — negative relative to length, unclamped.
fn to_absolute_index(v: Value, length: usize) -> i64 {
    let n = to_number(v);
    let i = if n.is_nan() || n == 0.0 {
        0
    } else if n.is_infinite() {
        if n > 0.0 { i64::MAX } else { i64::MIN }
    } else {
        n.trunc() as i64
    };
    if i < 0 { length as i64 + i } else { i }
}

/// Receiver check for TypedArray builtins — returns the RuneTypedArray ptr.
fn typed_array_receiver(gc: &mut SemiSpace, this: Value, vm: &mut Vm) -> Option<*mut u8> {
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_TYPED_ARRAY {
            return Some(ptr);
        }
    }
    vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
        gc,
        "TypeError: Method called on incompatible receiver",
    )));
    None
}

/// Receiver check for ArrayBuffer builtins.
fn array_buffer_receiver(gc: &mut SemiSpace, this: Value, vm: &mut Vm) -> Option<*mut u8> {
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_ARRAY_BUFFER {
            return Some(ptr);
        }
    }
    vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
        gc,
        "TypeError: Method called on incompatible receiver",
    )));
    None
}

/// Allocate a fresh ArrayBuffer for a typed array of `length` elements.
fn typed_alloc_buffer(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    kind: typedarray::TypedArrayKind,
    length: usize,
) -> Option<*mut u8> {
    let byte_len = length * kind.element_size();
    let proto = vm
        .array_buffer_prototype
        .heap_ptr()
        .unwrap_or(std::ptr::null_mut());
    Some(typedarray::RuneArrayBuffer::allocate(gc, byte_len, proto))
}

/// §23.2.5.1 TypedArray ( ...args ) — shared ctor body; `this` is the
/// pre-allocated RuneTypedArray (proto already set by the New arm).
fn typed_array_ctor_impl(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
    kind: typedarray::TypedArrayKind,
) -> Value {
    let ptr = match this.heap_ptr() {
        Some(p) => p,
        None => return Value::undefined(),
    };
    let argc = args.len();
    if argc == 0 {
        // AllocateTypedArrayBuffer(obj, 0)
        if let Some(buf) = typed_alloc_buffer(gc, vm, kind, 0) {
            unsafe {
                typedarray::RuneTypedArray::set_buffer(ptr, buf);
                typedarray::RuneTypedArray::set_kind(ptr, kind);
                typedarray::RuneTypedArray::set_length(ptr, 0);
                typedarray::RuneTypedArray::set_byte_offset(ptr, 0);
            }
        }
        return this;
    }
    let first = args[0];
    // Object argument?
    if let Some(fp) = first.heap_ptr() {
        let ftag = unsafe { (*(fp as *const GcHeader)).tag() };
        if ftag == TAG_ARRAY_BUFFER {
            // §23.2.5.1.3 InitializeTypedArrayFromArrayBuffer
            let byte_offset = if argc > 1 {
                args[1]
            } else {
                Value::undefined()
            };
            let length_arg = if argc > 2 {
                args[2]
            } else {
                Value::undefined()
            };
            let size = kind.element_size();
            let offset = match to_index_typed(gc, vm, byte_offset) {
                Ok(o) => o,
                Err(()) => return Value::undefined(),
            };
            if offset % size != 0 {
                vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                    gc,
                    "RangeError: Start offset of Uint8Array should be a multiple of 1",
                )));
                return Value::undefined();
            }
            let buf_len = unsafe { typedarray::RuneArrayBuffer::byte_length(fp) };
            let (new_byte_len, new_len) = if length_arg.is_undefined() {
                if buf_len % size != 0 {
                    vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                        gc,
                        "RangeError: Attempting to construct an invalid TypedArray",
                    )));
                    return Value::undefined();
                }
                if buf_len < offset {
                    vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                        gc,
                        "RangeError: Start offset is outside the bounds of the buffer",
                    )));
                    return Value::undefined();
                }
                (buf_len - offset, (buf_len - offset) / size)
            } else {
                let new_len = match to_index_typed(gc, vm, length_arg) {
                    Ok(l) => l,
                    Err(()) => return Value::undefined(),
                };
                let nb = new_len * size;
                if offset + nb > buf_len {
                    vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                        gc,
                        "RangeError: Invalid typed array length",
                    )));
                    return Value::undefined();
                }
                (nb, new_len)
            };
            unsafe {
                typedarray::RuneTypedArray::set_buffer(ptr, fp);
                typedarray::RuneTypedArray::set_kind(ptr, kind);
                typedarray::RuneTypedArray::set_byte_offset(ptr, offset);
                typedarray::RuneTypedArray::set_length(ptr, new_len);
            }
            let _ = new_byte_len;
            return this;
        }
        if ftag == TAG_TYPED_ARRAY {
            // §23.2.5.1.2 InitializeTypedArrayFromTypedArray — elementwise
            // (snapshots the source so overlapping conversion is safe).
            let src_len = unsafe { typedarray::RuneTypedArray::length(fp) };
            let mut vals = Vec::with_capacity(src_len);
            for i in 0..src_len {
                vals.push(to_number(unsafe { typedarray::read_element(fp, i) }));
            }
            if let Some(buf) = typed_alloc_buffer(gc, vm, kind, src_len) {
                unsafe {
                    typedarray::RuneTypedArray::set_buffer(ptr, buf);
                    typedarray::RuneTypedArray::set_kind(ptr, kind);
                    typedarray::RuneTypedArray::set_length(ptr, src_len);
                    typedarray::RuneTypedArray::set_byte_offset(ptr, 0);
                    for (i, v) in vals.iter().enumerate() {
                        typedarray::write_element(ptr, i, typedarray::convert_number(kind, *v));
                    }
                }
            }
            return this;
        }
        if ftag == TAG_ARRAY || ftag == TAG_STRING {
            // §23.2.5.1.5 InitializeTypedArrayFromArrayLike
            let len = if ftag == TAG_ARRAY {
                unsafe { rune_core::array::RuneArray::length(fp as *mut RuneArray) as usize }
            } else {
                unsafe { rune_core::string::HeapString::to_string(fp as *mut HeapString) }
                    .encode_utf16()
                    .count()
            };
            if let Some(buf) = typed_alloc_buffer(gc, vm, kind, len) {
                unsafe {
                    typedarray::RuneTypedArray::set_buffer(ptr, buf);
                    typedarray::RuneTypedArray::set_kind(ptr, kind);
                    typedarray::RuneTypedArray::set_length(ptr, len);
                    typedarray::RuneTypedArray::set_byte_offset(ptr, 0);
                }
            }
            for i in 0..len {
                let v = if ftag == TAG_ARRAY {
                    unsafe { rune_core::array::RuneArray::get_element(fp as *mut RuneArray, i) }
                } else {
                    let s =
                        unsafe { rune_core::string::HeapString::to_string(fp as *mut HeapString) };
                    Value::smi(s.encode_utf16().nth(i).unwrap_or(0) as i32)
                };
                let n = to_number(v);
                unsafe {
                    typedarray::write_element(ptr, i, typedarray::convert_number(kind, n));
                }
            }
            return this;
        }
        // Generic array-like object (plain objects): length + index gets.
        let len_val = load_property_recursive(
            first,
            Value::from_heap_ptr(crate::vm::heap_string(gc, "length")),
            Some(vm.function_prototype),
            gc,
        );
        let len = to_number(len_val);
        let len = if len.is_nan() || len <= 0.0 {
            0
        } else {
            len.trunc() as usize
        };
        if let Some(buf) = typed_alloc_buffer(gc, vm, kind, len) {
            unsafe {
                typedarray::RuneTypedArray::set_buffer(ptr, buf);
                typedarray::RuneTypedArray::set_kind(ptr, kind);
                typedarray::RuneTypedArray::set_length(ptr, len);
                typedarray::RuneTypedArray::set_byte_offset(ptr, 0);
            }
        }
        for i in 0..len {
            let v = load_property_recursive(
                first,
                Value::smi(i as i32),
                Some(vm.function_prototype),
                gc,
            );
            let n = to_number(v);
            unsafe {
                typedarray::write_element(ptr, i, typedarray::convert_number(kind, n));
            }
        }
        return this;
    }
    // §23.2.5.1 step 9: ToIndex(firstArg) → AllocateTypedArrayBuffer
    let element_length = match to_index_typed(gc, vm, first) {
        Ok(l) => l,
        Err(()) => return Value::undefined(),
    };
    if let Some(buf) = typed_alloc_buffer(gc, vm, kind, element_length) {
        unsafe {
            typedarray::RuneTypedArray::set_buffer(ptr, buf);
            typedarray::RuneTypedArray::set_kind(ptr, kind);
            typedarray::RuneTypedArray::set_length(ptr, element_length);
            typedarray::RuneTypedArray::set_byte_offset(ptr, 0);
        }
    }
    this
}

macro_rules! typed_array_ctor {
    ($name:ident, $kind:expr) => {
        pub fn $name(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
            typed_array_ctor_impl(gc, this, args, vm, $kind)
        }
    };
}

typed_array_ctor!(int8array_constructor, typedarray::TypedArrayKind::Int8);
typed_array_ctor!(uint8array_constructor, typedarray::TypedArrayKind::Uint8);
typed_array_ctor!(
    uint8clampedarray_constructor,
    typedarray::TypedArrayKind::Uint8Clamped
);
typed_array_ctor!(int16array_constructor, typedarray::TypedArrayKind::Int16);
typed_array_ctor!(uint16array_constructor, typedarray::TypedArrayKind::Uint16);
typed_array_ctor!(int32array_constructor, typedarray::TypedArrayKind::Int32);
typed_array_ctor!(uint32array_constructor, typedarray::TypedArrayKind::Uint32);
typed_array_ctor!(
    float32array_constructor,
    typedarray::TypedArrayKind::Float32
);
typed_array_ctor!(
    float64array_constructor,
    typedarray::TypedArrayKind::Float64
);

/// §25.1.4.1 ArrayBuffer ( length [ , options ] ) — `this` is a pre-allocated
/// zero-length RuneArrayBuffer; sets the real byte length.
pub fn array_buffer_constructor(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = this.heap_ptr() else {
        return Value::undefined();
    };
    let byte_length =
        match to_index_typed(gc, vm, args.first().copied().unwrap_or(Value::undefined())) {
            Ok(l) => l,
            Err(()) => return Value::undefined(),
        };
    if byte_length > 0 {
        let data = vec![0u8; byte_length].into_boxed_slice();
        unsafe {
            typedarray::RuneArrayBuffer::set_data_and_length(
                ptr,
                Box::into_raw(data) as *mut u8,
                byte_length,
            );
        }
    }
    this
}

/// §25.1.5.1 ArrayBuffer.isView ( arg )
pub fn array_buffer_is_view_builtin(
    _gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    _vm: &mut Vm,
) -> Value {
    let _ = this;
    let ok = args
        .first()
        .and_then(|v| v.heap_ptr())
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_TYPED_ARRAY });
    Value::boolean(ok)
}

/// §25.1.6.7 ArrayBuffer.prototype.slice ( start, end )
pub fn array_buffer_slice_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = array_buffer_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let len = unsafe { typedarray::RuneArrayBuffer::byte_length(ptr) };
    let first = to_clamped_index(args.first().copied().unwrap_or(Value::smi(0)), len);
    let final_ = if args.get(1).is_none_or(|v| v.is_undefined()) {
        len
    } else {
        to_clamped_index(args[1], len)
    };
    let new_len = final_.saturating_sub(first);
    let proto = vm
        .array_buffer_prototype
        .heap_ptr()
        .unwrap_or(std::ptr::null_mut());
    let new_ptr = typedarray::RuneArrayBuffer::allocate(gc, new_len, proto);
    if new_len > 0 {
        unsafe {
            typedarray::RuneArrayBuffer::copy_from(
                new_ptr,
                0,
                typedarray::RuneArrayBuffer::data(ptr),
                first,
                new_len,
            );
        }
    }
    Value::from_heap_ptr(new_ptr)
}

/// §23.2.3.30 TypedArray.prototype.subarray ( start, end ) — shares the buffer.
pub fn typed_array_subarray_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = typed_array_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let kind = unsafe { typedarray::RuneTypedArray::kind(ptr) };
    let length = unsafe { typedarray::RuneTypedArray::length(ptr) };
    let start = to_clamped_index(args.first().copied().unwrap_or(Value::smi(0)), length);
    let end = if args.get(1).is_none_or(|v| v.is_undefined()) {
        length
    } else {
        to_clamped_index(args[1], length)
    };
    let new_len = end.saturating_sub(start);
    let size = kind.element_size();
    let new_off = unsafe { typedarray::RuneTypedArray::byte_offset(ptr) } + start * size;
    let proto = vm
        .typed_array_protos
        .get(kind as usize)
        .and_then(|v| v.heap_ptr())
        .unwrap_or(std::ptr::null_mut());
    let new_ptr = typedarray::RuneTypedArray::allocate(gc, proto);
    unsafe {
        typedarray::RuneTypedArray::set_buffer(new_ptr, typedarray::RuneTypedArray::buffer(ptr));
        typedarray::RuneTypedArray::set_kind(new_ptr, kind);
        typedarray::RuneTypedArray::set_byte_offset(new_ptr, new_off);
        typedarray::RuneTypedArray::set_length(new_ptr, new_len);
    }
    Value::from_heap_ptr(new_ptr)
}

/// §23.2.3.9 TypedArray.prototype.fill ( value [ , start [ , end ] ] )
pub fn typed_array_fill_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = typed_array_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let kind = unsafe { typedarray::RuneTypedArray::kind(ptr) };
    let length = unsafe { typedarray::RuneTypedArray::length(ptr) };
    let value = typedarray::convert_number(
        kind,
        to_number(args.first().copied().unwrap_or(Value::undefined())),
    );
    let start = to_clamped_index(args.get(1).copied().unwrap_or(Value::smi(0)), length);
    let end = if args.get(2).is_none_or(|v| v.is_undefined()) {
        length
    } else {
        to_clamped_index(args[2], length)
    };
    for i in start..end.min(length) {
        unsafe {
            typedarray::write_element(ptr, i, value);
        }
    }
    this
}

/// §23.2.3.1 TypedArray.prototype.at ( index )
pub fn typed_array_at_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = typed_array_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let length = unsafe { typedarray::RuneTypedArray::length(ptr) };
    let k = to_absolute_index(args.first().copied().unwrap_or(Value::smi(0)), length);
    if k < 0 || k as usize >= length {
        return Value::undefined();
    }
    unsafe { typedarray::read_element(ptr, k as usize) }
}

/// §23.2.3.17 TypedArray.prototype.indexOf ( searchElement [ , fromIndex ] )
pub fn typed_array_index_of_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = typed_array_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let length = unsafe { typedarray::RuneTypedArray::length(ptr) };
    let search = args.first().copied().unwrap_or(Value::undefined());
    let k = if length == 0 {
        0
    } else {
        to_clamped_index(args.get(1).copied().unwrap_or(Value::smi(0)), length)
    };
    let mut i = k;
    while i < length {
        let el = unsafe { typedarray::read_element(ptr, i) };
        if el == search {
            return Value::smi(i as i32);
        }
        i += 1;
    }
    Value::smi(-1)
}

/// §23.2.3.16 TypedArray.prototype.includes ( searchElement [ , fromIndex ] )
pub fn typed_array_includes_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = typed_array_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let length = unsafe { typedarray::RuneTypedArray::length(ptr) };
    if length == 0 {
        return Value::boolean(false);
    }
    let search = args.first().copied().unwrap_or(Value::undefined());
    let k = to_clamped_index(args.get(1).copied().unwrap_or(Value::smi(0)), length);
    let mut i = k;
    while i < length {
        let el = unsafe { typedarray::read_element(ptr, i) };
        if same_value_zero(el, search) {
            return Value::boolean(true);
        }
        i += 1;
    }
    Value::boolean(false)
}

/// §23.2.3.26 TypedArray.prototype.set ( source [ , offset ] )
pub fn typed_array_set_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(target) = typed_array_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let Some(source) = args.first().copied() else {
        return Value::undefined();
    };
    let target_offset = {
        let n = to_number(args.get(1).copied().unwrap_or(Value::smi(0)));
        if n.is_nan() || n == 0.0 {
            0.0
        } else {
            n.trunc()
        }
    };
    if target_offset < 0.0 {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "RangeError: Offset is out of bounds",
        )));
        return Value::undefined();
    }
    let target_offset = target_offset as usize;
    let target_len = unsafe { typedarray::RuneTypedArray::length(target) };
    let target_kind = unsafe { typedarray::RuneTypedArray::kind(target) };

    enum ReadSource {
        Typed(*mut u8),
        Array(*mut u8),
        Object,
    }
    let (src_len, read_src) = if let Some(sp) = source.heap_ptr() {
        let stag = unsafe { (*(sp as *const GcHeader)).tag() };
        if stag == TAG_TYPED_ARRAY {
            (
                unsafe { typedarray::RuneTypedArray::length(sp) },
                ReadSource::Typed(sp),
            )
        } else if stag == TAG_ARRAY {
            (
                unsafe { rune_core::array::RuneArray::length(sp as *mut RuneArray) as usize },
                ReadSource::Array(sp),
            )
        } else {
            // Array-like: length + indexed gets via the load path.
            let len_val = load_property_recursive(
                source,
                Value::from_heap_ptr(crate::vm::heap_string(gc, "length")),
                Some(vm.function_prototype),
                gc,
            );
            let ln = to_number(len_val);
            let ln = if ln.is_nan() || ln <= 0.0 {
                0
            } else {
                ln.trunc() as usize
            };
            (ln, ReadSource::Object)
        }
    } else {
        // Primitives are not array-like.
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Cannot convert undefined or null to object",
        )));
        return Value::undefined();
    };

    if src_len + target_offset > target_len {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "RangeError: Offset is out of bounds",
        )));
        return Value::undefined();
    }
    // Snapshot the source values first (spec §23.2.3.26.2 clones the buffer
    // when source and target share one — a value snapshot is equivalent).
    let mut vals = Vec::with_capacity(src_len);
    for i in 0..src_len {
        let v = match read_src {
            ReadSource::Typed(sp) => unsafe { typedarray::read_element(sp, i) },
            ReadSource::Array(sp) => unsafe {
                rune_core::array::RuneArray::get_element(sp as *mut RuneArray, i)
            },
            ReadSource::Object => load_property_recursive(
                source,
                Value::smi(i as i32),
                Some(vm.function_prototype),
                gc,
            ),
        };
        vals.push(to_number(v));
    }
    for (i, v) in vals.iter().enumerate() {
        unsafe {
            typedarray::write_element(
                target,
                target_offset + i,
                typedarray::convert_number(target_kind, *v),
            );
        }
    }
    Value::undefined()
}

/// §23.2.3.32 TypedArray.prototype.slice ( start, end )
pub fn typed_array_slice_builtin(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(ptr) = typed_array_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let kind = unsafe { typedarray::RuneTypedArray::kind(ptr) };
    let length = unsafe { typedarray::RuneTypedArray::length(ptr) };
    let start = to_clamped_index(args.first().copied().unwrap_or(Value::smi(0)), length);
    let end = if args.get(1).is_none_or(|v| v.is_undefined()) {
        length
    } else {
        to_clamped_index(args[1], length)
    };
    let new_len = end.saturating_sub(start);
    let proto = vm
        .typed_array_protos
        .get(kind as usize)
        .and_then(|v| v.heap_ptr())
        .unwrap_or(std::ptr::null_mut());
    let new_ptr = typedarray::RuneTypedArray::allocate(gc, proto);
    if let Some(buf) = typed_alloc_buffer(gc, vm, kind, new_len) {
        unsafe {
            typedarray::RuneTypedArray::set_buffer(new_ptr, buf);
            typedarray::RuneTypedArray::set_kind(new_ptr, kind);
            typedarray::RuneTypedArray::set_length(new_ptr, new_len);
            typedarray::RuneTypedArray::set_byte_offset(new_ptr, 0);
            for i in 0..new_len {
                let v = typedarray::read_element(ptr, start + i);
                let n = to_number(v);
                typedarray::write_element(new_ptr, i, typedarray::convert_number(kind, n));
            }
        }
    }
    Value::from_heap_ptr(new_ptr)
}

/// TypedArray.prototype.values — iterator over element values (kind 2).
pub fn typed_array_values_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let ok = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_TYPED_ARRAY });
    if !ok {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: requires a typed array receiver",
        )));
        return Value::undefined();
    }
    make_iterator_object(
        gc,
        vm,
        "Array_iterator_next",
        &[this, Value::smi(0), Value::smi(2)],
        "Array Iterator",
    )
}

/// TypedArray.prototype.keys — iterator over indices (kind 1).
pub fn typed_array_keys_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let ok = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_TYPED_ARRAY });
    if !ok {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: requires a typed array receiver",
        )));
        return Value::undefined();
    }
    make_iterator_object(
        gc,
        vm,
        "Array_iterator_next",
        &[this, Value::smi(0), Value::smi(1)],
        "Array Iterator",
    )
}

/// TypedArray.prototype.entries — iterator over [index, value] pairs (kind 0).
pub fn typed_array_entries_builtin(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let ok = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_TYPED_ARRAY });
    if !ok {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: requires a typed array receiver",
        )));
        return Value::undefined();
    }
    make_iterator_object(
        gc,
        vm,
        "Array_iterator_next",
        &[this, Value::smi(0), Value::smi(0)],
        "Array Iterator",
    )
}

/// §27.1.3.6 Map.prototype.forEach(callback, thisArg)
pub fn map_foreach_builtin(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some(map_ptr) = map_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let callback = args.first().copied().unwrap_or(Value::undefined());
    if !is_callable_value(callback) {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: callback is not a function",
        )));
        return Value::undefined();
    }
    let this_arg = args.get(1).copied().unwrap_or(Value::undefined());
    let entries_ptr = unsafe { RuneMap::entries(map_ptr) };
    let len = if entries_ptr.is_null() {
        0
    } else {
        (unsafe { RuneArray::length(entries_ptr as *mut RuneArray) }) as usize
    };
    let size = unsafe { RuneMap::size(map_ptr) } as usize;
    // §23.1.3.25: entries deleted before being visited are not visited, so
    // snapshot the KEYS only and re-check liveness (and re-read the value)
    // at dispatch time. Mutations during callbacks don't reorder.
    let mut elems: Vec<Value> = Vec::with_capacity(len / 2);
    if !entries_ptr.is_null() {
        let entries = entries_ptr as *mut RuneArray;
        for i in (0..len).step_by(2) {
            let k = unsafe { RuneArray::get_element(entries, i) };
            if k.raw() != Value::empty_sentinel().raw() {
                elems.push(k);
            }
        }
    }
    let snapshot = RuneArray::allocate(gc, &elems) as *mut u8;
    let mut idx = 0usize;
    let mut found = 0usize;
    while idx < elems.len() && found < size {
        let k = unsafe { RuneArray::get_element(snapshot as *mut RuneArray, idx) };
        let live_entries = unsafe { RuneMap::entries(map_ptr) };
        if let Some(live) = key_index(live_entries, k, true) {
            found += 1;
            let v = unsafe { RuneArray::get_element(live_entries as *mut RuneArray, live + 1) };
            if callback.as_smi().is_some_and(|s| s < 0) {
                let id = (-callback.as_smi().unwrap() as usize) - 1;
                if id < vm.builtins.len() {
                    (vm.builtins[id].func)(gc, this_arg, &[v, k, this], vm);
                    if vm.pending_exception.is_some() {
                        return Value::undefined();
                    }
                }
            } else {
                vm.pending_collection_foreach = Some(PendingCollectionForEach {
                    source_frame_depth: vm.frame_depth() - 1,
                    snapshot,
                    idx: idx + 1,
                    found,
                    size,
                    is_map: true,
                    callback,
                    this_arg,
                    collection: this,
                });
                vm.push_callback_call(gc, callback, this_arg, vec![v, k, this]);
                return Value::undefined();
            }
        }
        idx += 1;
    }
    Value::undefined()
}

/// §27.2.3.8 Set.prototype.forEach(callback, thisArg)
pub fn set_foreach_builtin(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some(set_ptr) = set_receiver(gc, this, vm) else {
        return Value::undefined();
    };
    let callback = args.first().copied().unwrap_or(Value::undefined());
    if !is_callable_value(callback) {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: callback is not a function",
        )));
        return Value::undefined();
    }
    let this_arg = args.get(1).copied().unwrap_or(Value::undefined());
    let entries_ptr = unsafe { RuneSet::entries(set_ptr) };
    let len = if entries_ptr.is_null() {
        0
    } else {
        (unsafe { RuneArray::length(entries_ptr as *mut RuneArray) }) as usize
    };
    let size = unsafe { RuneSet::size(set_ptr) } as usize;
    // §23.2.3.11: elements deleted before being visited are not visited.
    let mut elems: Vec<Value> = Vec::with_capacity(len);
    if !entries_ptr.is_null() {
        let entries = entries_ptr as *mut RuneArray;
        for i in 0..len {
            let v = unsafe { RuneArray::get_element(entries, i) };
            if v.raw() != Value::empty_sentinel().raw() {
                elems.push(v);
            }
        }
    }
    let snapshot = RuneArray::allocate(gc, &elems) as *mut u8;
    let mut idx = 0usize;
    let mut found = 0usize;
    while idx < elems.len() && found < size {
        let v = unsafe { RuneArray::get_element(snapshot as *mut RuneArray, idx) };
        let live_entries = unsafe { RuneSet::entries(set_ptr) };
        if key_index(live_entries, v, false).is_some() {
            found += 1;
            if callback.as_smi().is_some_and(|s| s < 0) {
                let id = (-callback.as_smi().unwrap() as usize) - 1;
                if id < vm.builtins.len() {
                    (vm.builtins[id].func)(gc, this_arg, &[v, v, this], vm);
                    if vm.pending_exception.is_some() {
                        return Value::undefined();
                    }
                }
            } else {
                vm.pending_collection_foreach = Some(PendingCollectionForEach {
                    source_frame_depth: vm.frame_depth() - 1,
                    snapshot,
                    idx: idx + 1,
                    found,
                    size,
                    is_map: false,
                    callback,
                    this_arg,
                    collection: this,
                });
                vm.push_callback_call(gc, callback, this_arg, vec![v, v, this]);
                return Value::undefined();
            }
        }
        idx += 1;
    }
    Value::undefined()
}

/// Outcome of filling a collection from an iterable.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FillOutcome {
    Done,
    /// A callback was pushed; the caller must not advance pc.
    Pending,
    /// An exception is set on `vm.pending_exception`.
    Threw,
}

/// Process one iterator result during collection construction.
/// Ok(true) = iterator done; Ok(false) = entry added, continue; Err = threw.
fn process_collection_result(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    collection: &mut Value,
    is_map: bool,
    result: Value,
) -> Result<bool, ()> {
    if !result.is_heap_object() {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Iterator result is not an object",
        )));
        return Err(());
    }
    let done = load_property_recursive(result, vm.done_key, None, gc).to_bool();
    if done {
        return Ok(true);
    }
    let value = load_property_recursive(result, vm.value_key, None, gc);
    if is_map {
        // §27.1.1.1 step 10.b: each iterator value must be an Object (the
        // [key, value] pair); a Set adds the raw value instead.
        if !is_object_value(value) {
            vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                gc,
                "TypeError: Iterator value is not an object",
            )));
            return Err(());
        }
        let k = load_property_recursive(value, Value::smi(0), None, gc);
        let v = load_property_recursive(value, Value::smi(1), None, gc);
        map_set_internal(gc, collection, k, v);
    } else {
        set_add_internal(gc, collection, value);
    }
    Ok(false)
}

/// Fill `collection` from `iterator` (obtained from the @@iterator factory).
/// `collection` and `iterator` must be rooted by the caller (VM stack or
/// pending state fields); they are re-read from the rooted slots via the
/// provided slot indices whenever an allocation may run the GC.
pub(crate) fn fill_collection_from_iterator(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    collection_idx: usize,
    iterator_idx: usize,
    is_map: bool,
) -> FillOutcome {
    let mut collection = vm.stack[collection_idx];
    let mut iterator = vm.stack[iterator_idx];
    if !iterator.is_heap_object() {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: value is not iterable",
        )));
        return FillOutcome::Threw;
    }
    let next = load_property_recursive(iterator, vm.next_key, Some(vm.function_prototype), gc);
    if next.as_smi().is_some_and(|s| s < 0) {
        loop {
            let id = (-next.as_smi().unwrap() as usize) - 1;
            let result = if id < vm.builtins.len() {
                let r = (vm.builtins[id].func)(gc, iterator, &[], vm);
                if vm.pending_exception.is_some() {
                    return FillOutcome::Threw;
                }
                r
            } else {
                vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                    gc,
                    "TypeError: iterator.next is not a function",
                )));
                return FillOutcome::Threw;
            };
            collection = vm.stack[collection_idx];
            iterator = vm.stack[iterator_idx];
            match process_collection_result(vm, gc, &mut collection, is_map, result) {
                Ok(true) => return FillOutcome::Done,
                Ok(false) => continue,
                Err(()) => return FillOutcome::Threw,
            }
        }
    } else if next.is_heap_object()
        && unsafe { (*(next.heap_ptr().unwrap() as *const GcHeader)).tag() } == TAG_FUNC
    {
        vm.pending_collection_ctor = Some(PendingCollectionCtor {
            source_frame_depth: vm.frame_depth() - 1,
            root_base: collection_idx,
            state: CollectionCtorState::AwaitNext,
            iter: iterator,
            next,
            collection,
            is_map,
        });
        vm.push_callback_call(gc, next, iterator, vec![]);
        FillOutcome::Pending
    } else {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: iterator.next is not a function",
        )));
        FillOutcome::Threw
    }
}

/// §27.1.1.1 Map constructor — AddEntriesFromIterable.
/// The map must be rooted on the VM stack at `collection_idx`.
pub fn map_constructor(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let root_base = vm.stack.len();
    vm.push(this);
    let iterable = args.first().copied().unwrap_or(Value::undefined());
    let outcome = if iterable.is_undefined() || iterable.is_null() {
        FillOutcome::Done
    } else {
        fill_collection_from_iterable(vm, gc, root_base, iterable, true)
    };
    match outcome {
        FillOutcome::Done => {
            vm.stack.truncate(root_base);
            this
        }
        // Pending: a callback frame sits on the stack (rooted at root_base);
        // truncating would steal the root below its base.
        FillOutcome::Pending => Value::undefined(),
        FillOutcome::Threw => {
            vm.stack.truncate(root_base);
            Value::undefined()
        }
    }
}

/// §27.2.1.1 Set constructor.
pub fn set_constructor(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let root_base = vm.stack.len();
    vm.push(this);
    let iterable = args.first().copied().unwrap_or(Value::undefined());
    let outcome = if iterable.is_undefined() || iterable.is_null() {
        FillOutcome::Done
    } else {
        fill_collection_from_iterable(vm, gc, root_base, iterable, false)
    };
    match outcome {
        FillOutcome::Done => {
            vm.stack.truncate(root_base);
            this
        }
        FillOutcome::Pending => Value::undefined(),
        FillOutcome::Threw => {
            vm.stack.truncate(root_base);
            Value::undefined()
        }
    }
}

/// Resolve the @@iterator method for `iterable` and fill `collection`
/// (rooted at `collection_idx` on the VM stack) from it.
fn fill_collection_from_iterable(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    collection_idx: usize,
    iterable: Value,
    is_map: bool,
) -> FillOutcome {
    let method = match get_iter_method(vm, gc, iterable) {
        SymbolMethodResult::Found(m) => m,
        SymbolMethodResult::NotCallable => {
            vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                gc,
                "TypeError: value[Symbol.iterator] is not callable",
            )));
            return FillOutcome::Threw;
        }
        SymbolMethodResult::NotFound => {
            vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                gc,
                "TypeError: value is not iterable",
            )));
            return FillOutcome::Threw;
        }
    };
    if method.as_smi().is_some_and(|s| s < 0) {
        let id = (-method.as_smi().unwrap() as usize) - 1;
        let iterator = if id < vm.builtins.len() {
            let r = (vm.builtins[id].func)(gc, iterable, &[], vm);
            if vm.pending_exception.is_some() {
                return FillOutcome::Threw;
            }
            r
        } else {
            vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                gc,
                "TypeError: value is not iterable",
            )));
            return FillOutcome::Threw;
        };
        vm.stack.push(iterator);
        let outcome =
            fill_collection_from_iterator(vm, gc, collection_idx, vm.stack.len() - 1, is_map);
        vm.stack.truncate(vm.stack.len() - 1);
        outcome
    } else if method.is_heap_object()
        && unsafe { (*(method.heap_ptr().unwrap() as *const GcHeader)).tag() } == TAG_FUNC
    {
        vm.pending_collection_ctor = Some(PendingCollectionCtor {
            source_frame_depth: vm.frame_depth() - 1,
            root_base: collection_idx,
            state: CollectionCtorState::AwaitFactory,
            iter: Value::undefined(),
            next: Value::undefined(),
            collection: vm.stack[collection_idx],
            is_map,
        });
        vm.push_callback_call(gc, method, iterable, vec![]);
        FillOutcome::Pending
    } else {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: value[Symbol.iterator] is not callable",
        )));
        FillOutcome::Threw
    }
}

/// §7.3.11 GetMethod + dispatch of a well-known-symbol method from the
/// String.prototype match/search/split/replace family.
///
/// When `pattern` is an object with a callable @@method, pushes a callback
/// frame and returns Ok(None) — the builtin must return undefined immediately
/// and the Return handler routes the method's result back to the caller.
/// Err(()) means the caller should throw (non-callable @@method).
/// Ok(Some(())) means fall back to the legacy algorithm.
fn dispatch_symbol_method(
    gc: &mut SemiSpace,
    pattern: Value,
    symbol_id: u32,
    this: Value,
    extra_args: &[Value],
    vm: &mut Vm,
) -> Result<Option<()>, ()> {
    if let Some(ptr) = pattern.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_OBJECT {
            match get_symbol_method(gc, pattern, symbol_id, Some(vm.function_prototype)) {
                SymbolMethodResult::Found(m) => {
                    let mut args = Vec::with_capacity(1 + extra_args.len());
                    args.push(this);
                    args.extend_from_slice(extra_args);
                    vm.pending_symbol_dispatch = Some(crate::vm::PendingSymbolDispatch {
                        source_frame_depth: vm.frame_depth(),
                    });
                    vm.push_callback_call(gc, m, pattern, args);
                    Ok(None)
                }
                SymbolMethodResult::NotCallable => {
                    let name = match symbol_id {
                        SYM_MATCH => "@@match",
                        SYM_REPLACE => "@@replace",
                        SYM_SEARCH => "@@search",
                        SYM_SPLIT => "@@split",
                        _ => "@@method",
                    };
                    vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                        gc,
                        &format!("TypeError: {name} method called on an object with a non-callable @@method property"),
                    )));
                    Err(())
                }
                SymbolMethodResult::NotFound => Ok(Some(())),
            }
        } else {
            Ok(Some(()))
        }
    } else {
        Ok(Some(()))
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

/// NativeError type names, indexed consistently with `Vm::error_ctors` and
/// `Vm::error_protos` (Error first, then the six native errors).
pub const ERROR_TYPE_NAMES: [&str; 7] = [
    "Error",
    "EvalError",
    "RangeError",
    "ReferenceError",
    "SyntaxError",
    "TypeError",
    "URIError",
];

/// Result of converting an Error constructor message argument to a string.
enum ErrorMessageToString {
    Done(String),
    /// ToString threw — `vm.pending_exception` is set.
    Throw,
    /// The message object has a user-defined toString/valueOf; the callback
    /// machinery is deferred (documented gap) — treat as "no message".
    Pending,
}

/// §7.1.18 ToString for an Error constructor message. Symbols throw a
/// TypeError; objects without a usable toString/valueOf throw a TypeError
/// (§7.1.1 ToPrimitive with string hint).
fn to_string_for_error(val: Value, gc: &mut SemiSpace, vm: &mut Vm) -> ErrorMessageToString {
    if val.is_symbol() {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Cannot convert a Symbol value to a string",
        )));
        return ErrorMessageToString::Throw;
    }
    if !val.is_heap_object() {
        return ErrorMessageToString::Done(value_to_js_string(val));
    }
    let ptr = val.heap_ptr().unwrap();
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag == TAG_STRING || tag == TAG_STRING_OBJ || tag == TAG_DATE {
        return ErrorMessageToString::Done(value_to_js_string(val));
    }
    if tag == TAG_OBJECT {
        // ToPrimitive with string hint: try toString(), then valueOf().
        // The value is pushed onto the operand stack so it survives any GC
        // triggered by a builtin call below; re-read it each iteration.
        let depth = vm.stack.len();
        vm.push(val);
        let mut outcome = ErrorMessageToString::Throw;
        'outer: for method in ["toString", "valueOf"] {
            let cur = vm.stack[depth];
            let ptr = cur.heap_ptr().unwrap();
            let key = PropertyKey::from_string(method);
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            if let Some(slot) = shape.lookup(&key) {
                let m = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                if let Some(smi) = m.as_smi() {
                    if smi < 0 {
                        let id = ((-smi) as usize) - 1;
                        if id < vm.builtins.len() {
                            let r = (vm.builtins[id].func)(gc, cur, &[], vm);
                            if let Some(exc) = vm.pending_exception.take() {
                                vm.pending_exception = Some(exc);
                                outcome = ErrorMessageToString::Throw;
                                break 'outer;
                            }
                            if !r.is_heap_object() {
                                outcome = ErrorMessageToString::Done(value_to_js_string(r));
                                break 'outer;
                            }
                        }
                    }
                } else if let Some(func_ptr) = m.heap_ptr() {
                    let ft = unsafe { (*(func_ptr as *const GcHeader)).tag() };
                    if ft == rune_core::gc::TAG_FUNC {
                        // User-defined toString/valueOf — the pending-callback
                        // continuation is not wired for Error ctors (gap).
                        outcome = ErrorMessageToString::Pending;
                        break 'outer;
                    }
                }
            }
        }
        vm.stack.truncate(depth);
        if matches!(outcome, ErrorMessageToString::Throw) && vm.pending_exception.is_none() {
            vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
                gc,
                "TypeError: Cannot convert object to primitive value",
            )));
        }
        return outcome;
    }
    ErrorMessageToString::Done(value_to_js_string(val))
}

/// §20.5.1.1 Error(message[, options]) / §20.5.6.1.1 NativeError(message[, options]).
/// Creates an object whose [[Prototype]] is the given type's prototype, with
/// an own `message` property (when message is not undefined) and an own
/// `cause` property (when options is an object with a data "cause" property).
pub fn error_constructor(
    gc: &mut SemiSpace,
    type_idx: usize,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    // Push the args onto the operand stack: register_roots re-forwards the
    // stack on every collection, so values are safe to re-read from there
    // after any allocation below. Raw copies in locals go stale across a GC.
    let base = vm.stack.len();
    let nargs = args.len();
    for a in args {
        vm.push(*a);
    }

    let mut has_message = false;
    let mut msg = String::new();
    if let Some(m) = args.first() {
        if !m.is_undefined() {
            match to_string_for_error(*m, gc, vm) {
                ErrorMessageToString::Done(s) => {
                    has_message = true;
                    msg = s;
                }
                ErrorMessageToString::Throw => {
                    vm.stack.truncate(base);
                    return Value::undefined();
                }
                ErrorMessageToString::Pending => {}
            }
        }
    }
    // §20.5.8.1 InstallErrorCause: only object options with a "cause" data
    // property. Accessor (getter) values are skipped — no accessor dispatch
    // here (documented gap). `has_cause` is decided here; the cause VALUE is
    // re-read after the allocations below (GC may move it).
    let mut has_cause = false;
    let opts_val = if nargs >= 2 {
        vm.stack[base + 1]
    } else {
        Value::undefined()
    };
    if let Some(ptr) = opts_val.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_OBJECT {
            let key = PropertyKey::from_string("cause");
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            if let Some(slot) = shape.lookup(&key) {
                let cv = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                let is_accessor = cv.heap_ptr().is_some_and(|cp| unsafe {
                    (*(cp as *const GcHeader)).tag() == rune_core::gc::TAG_ACCESSOR
                });
                if !is_accessor {
                    has_cause = true;
                }
            }
        }
    }
    let (shape, _slots) = match (has_message, has_cause) {
        (true, true) => {
            let entries = vec![
                (PropertyKey::from_string("message"), 0usize),
                (PropertyKey::from_string("cause"), 1usize),
            ];
            let key_names = vec!["message".to_string(), "cause".to_string()];
            (Shape::intern(entries, key_names), 2usize)
        }
        (true, false) => {
            let entries = vec![(PropertyKey::from_string("message"), 0usize)];
            let key_names = vec!["message".to_string()];
            (Shape::intern(entries, key_names), 1usize)
        }
        (false, true) => {
            let entries = vec![(PropertyKey::from_string("cause"), 0usize)];
            let key_names = vec!["cause".to_string()];
            let shape = Shape::intern(entries, key_names);
            (shape, 1usize)
        }
        (false, false) => (Shape::empty(), 0usize),
    };
    // Allocate the message string FIRST and root it on the operand stack,
    // then allocate the object LAST. The object allocate may trigger a GC
    // that moves the message string and error prototypes, so those are
    // re-read from the rooted stack / vm fields after it.
    let msg_string_slot = if has_message {
        let slot = vm.stack.len();
        vm.push(Value::from_heap_ptr(crate::vm::heap_string(gc, &msg)));
        slot
    } else {
        0
    };
    let obj = JSObject::allocate(gc, shape, &[]);
    let mut slot = 0;
    if has_message {
        let m = vm.stack[msg_string_slot];
        unsafe {
            JSObject::set_slot(obj, slot, m);
        }
        slot += 1;
    }
    let proto_ptr = vm.error_protos.get(type_idx).and_then(|v| v.heap_ptr());
    if let Some(p) = proto_ptr {
        unsafe {
            JSObject::set_prototype(obj, p);
        }
    }
    if has_cause {
        // stack[base + 1] is the options value — re-read after any GC.
        let opts2 = if nargs >= 2 {
            vm.stack[base + 1]
        } else {
            Value::undefined()
        };
        let mut cause = Value::undefined();
        if let Some(ptr) = opts2.heap_ptr() {
            if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_OBJECT {
                let key = PropertyKey::from_string("cause");
                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                if let Some(slot) = shape.lookup(&key) {
                    cause = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                }
            }
        }
        unsafe {
            JSObject::set_slot(obj, slot, cause);
        }
    }
    // The shape was created with the slots we just filled — record the count
    // so future add_property() transitions append past them instead of
    // overwriting slot 0 (clobbering `message`).
    unsafe {
        JSObject::set_slot_count(obj, slot);
    }
    vm.stack.truncate(base);
    let _ = nargs;
    Value::from_heap_ptr(obj as *mut u8)
}

/// Error(message) — creates a minimal error object with `name` and `message` properties.
pub fn error_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    error_constructor(gc, 0, args, vm)
}

/// EvalError(message)
pub fn eval_error_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    error_constructor(gc, 1, args, vm)
}

/// RangeError(message)
pub fn range_error_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    error_constructor(gc, 2, args, vm)
}

/// ReferenceError(message)
pub fn reference_error_builtin(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    error_constructor(gc, 3, args, vm)
}

/// SyntaxError(message)
pub fn syntax_error_builtin(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    error_constructor(gc, 4, args, vm)
}

/// TypeError(message)
pub fn type_error_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    error_constructor(gc, 5, args, vm)
}

/// URIError(message)
pub fn uri_error_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    error_constructor(gc, 6, args, vm)
}

/// §20.5.3.1 Error.prototype.toString()
pub fn error_prototype_to_string(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    if !this.is_heap_object() {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Error.prototype.toString requires that 'this' be an Object",
        )));
        return Value::undefined();
    }
    // Push `this` onto the operand stack: register_roots re-forwards it on
    // every collection, so re-reading it after any allocation is safe.
    let base = vm.stack.len();
    vm.push(this);
    // Allocate the key strings up front so no allocation happens between
    // the two property reads.
    let name_key = Value::from_heap_ptr(crate::vm::heap_string(gc, "name"));
    let msg_key = Value::from_heap_ptr(crate::vm::heap_string(gc, "message"));
    let this_val = vm.stack[base];
    // name: Get(O, "name"); undefined → "Error".
    let name = load_property_recursive(this_val, name_key, Some(vm.function_prototype), gc);
    let name_str = if name.is_undefined() {
        "Error".to_string()
    } else {
        value_to_js_string(name)
    };
    // message: Get(O, "message"); undefined → "".
    let this_val2 = vm.stack[base];
    let msg = load_property_recursive(this_val2, msg_key, Some(vm.function_prototype), gc);
    vm.stack.truncate(base);
    let msg_str = if msg.is_undefined() {
        String::new()
    } else {
        value_to_js_string(msg)
    };
    let result = if name_str.is_empty() {
        msg_str
    } else if msg_str.is_empty() {
        name_str
    } else {
        format!("{}: {}", name_str, msg_str)
    };
    Value::from_heap_ptr(crate::vm::heap_string(gc, &result))
}

/// §20.3.4.2 Object.prototype.toString() — returns "[object Tag]" where Tag
/// comes from the receiver's type (and, for Error instances, the prototype
/// chain reaching one of the seven error prototypes).
pub fn object_prototype_to_string(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    let tag = if this.is_undefined() {
        "Undefined".to_string()
    } else if this.is_null() {
        "Null".to_string()
    } else if let Some(b) = this.to_boolean() {
        if b {
            "Boolean".to_string()
        } else {
            "Boolean".to_string()
        }
    } else if this.is_symbol() {
        "Symbol".to_string()
    } else if this.is_smi() || this.as_float64().is_some() {
        "Number".to_string()
    } else if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        match tag {
            TAG_STRING => "String".to_string(),
            TAG_STRING_OBJ => "String".to_string(),
            TAG_ARRAY => "Array".to_string(),
            TAG_FUNC => "Function".to_string(),
            TAG_REGEXP => "RegExp".to_string(),
            TAG_PROMISE => "Promise".to_string(),
            TAG_MAP => "Map".to_string(),
            TAG_SET => "Set".to_string(),
            TAG_DATE => "Date".to_string(),
            TAG_ARRAY_BUFFER => "ArrayBuffer".to_string(),
            TAG_TYPED_ARRAY => "Object".to_string(),
            TAG_OBJECT => {
                // Callable wrappers (builtin constructors) are functions.
                if vm
                    .callable_wrappers
                    .iter()
                    .any(|w| w.heap_ptr() == Some(ptr))
                {
                    "Function".to_string()
                } else if vm.error_protos.iter().any(|ep| ep.heap_ptr() == Some(ptr)) {
                    // Error prototype objects themselves are ordinary objects
                    // (no [[ErrorData]] slot) — only *instances* whose chain
                    // reaches an error prototype get the "Error" tag.
                    "Object".to_string()
                } else if vm
                    .error_protos
                    .iter()
                    .any(|ep| ep.heap_ptr().is_some_and(|p| is_on_proto_chain(ptr, p)))
                {
                    "Error".to_string()
                } else {
                    "Object".to_string()
                }
            }
            _ => "Object".to_string(),
        }
    } else {
        "Object".to_string()
    };
    Value::from_heap_ptr(crate::vm::heap_string(gc, &format!("[object {}]", tag)))
}

/// True iff `obj` (exclusive) has `proto` somewhere in its prototype chain.
fn is_on_proto_chain(obj: *mut u8, proto: *mut u8) -> bool {
    let mut cur = unsafe { JSObject::prototype(obj as *mut JSObject) };
    for _ in 0..MAX_PROTOTYPE_DEPTH {
        if cur.is_null() {
            return false;
        }
        if cur == proto {
            return true;
        }
        cur = unsafe { JSObject::prototype(cur as *mut JSObject) };
    }
    false
}

/// §20.3.4.4 Object.prototype.hasOwnProperty(key) — own-property check only.
pub fn object_prototype_has_own_property(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    if this.is_undefined() || this.is_null() {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Cannot convert undefined or null to object",
        )));
        return Value::undefined();
    }
    let key = args.first().copied().unwrap_or(Value::undefined());
    let Some(ptr) = this.heap_ptr() else {
        // Primitives have no own properties (string exotic props unsupported).
        return Value::boolean(false);
    };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    let key_str: Option<String> = match key.heap_ptr() {
        Some(kp) if unsafe { (*(kp as *const GcHeader)).tag() } == TAG_STRING => Some(unsafe {
            rune_core::string::HeapString::to_string(kp as *mut rune_core::string::HeapString)
        }),
        _ => None,
    };
    let key_is_str = |s: &str| key_str.as_deref() == Some(s);
    let found = match tag {
        TAG_ARRAY => {
            if let Some(index) = value_to_array_index(key) {
                let len = unsafe {
                    rune_core::array::RuneArray::length(ptr as *mut rune_core::array::RuneArray)
                };
                index < len as usize
            } else {
                key_is_str("length")
            }
        }
        TAG_TYPED_ARRAY => {
            if let Some(index) = value_to_array_index(key) {
                let len = unsafe { rune_core::typedarray::RuneTypedArray::length(ptr) };
                index < len as usize
            } else {
                key_is_str("length")
                    || key_is_str("byteLength")
                    || key_is_str("byteOffset")
                    || key_is_str("buffer")
            }
        }
        TAG_OBJECT => {
            let Some(pk) = value_to_prop_key(key) else {
                return Value::boolean(false);
            };
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            shape.lookup(&pk).is_some()
        }
        _ => false,
    };
    Value::boolean(found)
}

/// §20.3.4.5 Object.prototype.propertyIsEnumerable(key) — true iff the key is
/// an own property AND enumerable. The engine has no per-property
/// enumerability flags, so the result equals hasOwnProperty (all own
/// properties are treated as enumerable).
pub fn object_prototype_property_is_enumerable(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    if this.is_undefined() || this.is_null() {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Cannot convert undefined or null to object",
        )));
        return Value::undefined();
    }
    let key = args.first().copied().unwrap_or(Value::undefined());
    let Some(ptr) = this.heap_ptr() else {
        return Value::boolean(false);
    };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    let key_str: Option<String> = match key.heap_ptr() {
        Some(kp) if unsafe { (*(kp as *const GcHeader)).tag() } == TAG_STRING => Some(unsafe {
            rune_core::string::HeapString::to_string(kp as *mut rune_core::string::HeapString)
        }),
        _ => None,
    };
    let key_is_str = |s: &str| key_str.as_deref() == Some(s);
    let found = match tag {
        TAG_ARRAY => {
            if let Some(index) = value_to_array_index(key) {
                let len = unsafe {
                    rune_core::array::RuneArray::length(ptr as *mut rune_core::array::RuneArray)
                };
                index < len as usize
            } else {
                key_is_str("length")
            }
        }
        TAG_TYPED_ARRAY => {
            if let Some(index) = value_to_array_index(key) {
                let len = unsafe { rune_core::typedarray::RuneTypedArray::length(ptr) };
                index < len as usize
            } else {
                key_is_str("length")
                    || key_is_str("byteLength")
                    || key_is_str("byteOffset")
                    || key_is_str("buffer")
            }
        }
        TAG_OBJECT => {
            let Some(pk) = value_to_prop_key(key) else {
                return Value::boolean(false);
            };
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            shape.lookup(&pk).is_some()
        }
        _ => false,
    };
    Value::boolean(found)
}

/// §20.3.4.5 Object.prototype.valueOf() — returns the receiver object.
pub fn object_prototype_value_of(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    if !this.is_heap_object() {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Object.prototype.valueOf called on non-object",
        )));
        return Value::undefined();
    }
    this
}

/// §20.1.2.10 Object.getPrototypeOf(obj) — returns the [[Prototype]].
pub fn object_get_prototype_of(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let obj = args.first().copied().unwrap_or(Value::undefined());
    let Some(ptr) = obj.heap_ptr() else {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Object.getPrototypeOf called on non-object",
        )));
        return Value::undefined();
    };
    let proto = unsafe { JSObject::prototype(ptr as *mut JSObject) };
    if proto.is_null() {
        return Value::null();
    }
    Value::from_heap_ptr(proto)
}

/// §20.3.4.5 Object.prototype.isPrototypeOf(obj) — true iff the receiver is
/// on `obj`'s prototype chain.
pub fn object_prototype_is_prototype_of(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some(this_ptr) = this.heap_ptr() else {
        vm.set_pending_exception(Value::from_heap_ptr(crate::vm::heap_string(
            gc,
            "TypeError: Object.prototype.isPrototypeOf called on non-object",
        )));
        return Value::undefined();
    };
    let target = args.first().copied().unwrap_or(Value::undefined());
    let Some(tgt_ptr) = target.heap_ptr() else {
        return Value::boolean(false);
    };
    if tgt_ptr == this_ptr {
        return Value::boolean(true);
    }
    Value::boolean(is_on_proto_chain(tgt_ptr, this_ptr))
}

/// §20.5.2.4 Error.isError(value) — true iff value has an [[ErrorData]]
/// internal slot (i.e. it is an Error or subclass instance). Our error
/// instances are ordinary objects whose prototype chain reaches one of the
/// seven error prototypes; walk the chain (fake errors that merely inherit
/// from Error.prototype without going through a constructor are not marked).
/// [[Construct]] is not implemented — the New arm rejects `new Error.isError`.
pub fn error_is_error(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let val = args.first().copied().unwrap_or(Value::undefined());
    let mut cur = match val.heap_ptr() {
        Some(ptr) => ptr,
        None => return Value::boolean(false),
    };
    let tag = unsafe { (*(cur as *const GcHeader)).tag() };
    if tag != TAG_OBJECT {
        return Value::boolean(false);
    }
    for _ in 0..MAX_PROTOTYPE_DEPTH {
        let proto = unsafe { JSObject::prototype(cur as *mut JSObject) };
        if proto.is_null() {
            return Value::boolean(false);
        }
        if _vm.error_protos.iter().any(|p| p.heap_ptr() == Some(proto)) {
            return Value::boolean(true);
        }
        cur = proto;
    }
    Value::boolean(false)
}

const MAX_PROTOTYPE_DEPTH: usize = 256;

/// Test262Error(message) — built-in replacement for sta.js Test262Error constructor.
pub fn test262_error_builtin(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    _vm: &mut Vm,
) -> Value {
    error_with_name(gc, args, "Test262Error")
}

/// Create an Error-shaped object with `name` and `message` properties.
fn error_with_name(gc: &mut SemiSpace, args: &[Value], name: &str) -> Value {
    let msg = if let Some(arg) = args.first() {
        value_to_js_string(*arg)
    } else {
        String::new()
    };
    let name_str: *mut u8 = HeapString::allocate(gc, name) as *mut u8;
    let msg_str: *mut u8 = HeapString::allocate(gc, &msg) as *mut u8;
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
    Value::from_heap_ptr(obj as *mut u8)
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
                    // §20.1.2.3: symbol-keyed properties are excluded from
                    // Object.keys/values/entries enumeration.
                    if shape.entries[i].0.is_symbol() {
                        continue;
                    }
                    let key = shape.key_name_at(i).unwrap_or("").to_string();
                    let value = unsafe { JSObject::get_slot(ptr as *mut JSObject, i) };
                    entries.push((key, value));
                }
                Ok(entries)
            }
            TAG_ARRAY => {
                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) } as usize;
                let mut entries = Vec::with_capacity(len + 4);
                for i in 0..len {
                    let value = unsafe { RuneArray::get_element(ptr as *mut RuneArray, i) };
                    entries.push((i.to_string(), value));
                }
                // Named properties (e.g. "index"/"input" on match-result arrays,
                // user assignments like a.foo) are own enumerable properties.
                let extra_ptr = unsafe { RuneArray::extra_props(ptr as *mut RuneArray) };
                if !extra_ptr.is_null() {
                    let shape = unsafe { JSObject::shape_ptr(extra_ptr as *mut JSObject) };
                    let count = unsafe { JSObject::slot_count(extra_ptr as *mut JSObject) };
                    for i in 0..count {
                        if shape.entries[i].0.is_symbol() {
                            continue;
                        }
                        let key = shape.key_name_at(i).unwrap_or("").to_string();
                        let value = unsafe { JSObject::get_slot(extra_ptr as *mut JSObject, i) };
                        entries.push((key, value));
                    }
                }
                Ok(entries)
            }
            TAG_STRING => {
                let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
                let mut entries = Vec::with_capacity(s.len());
                for (i, c) in s.chars().enumerate() {
                    let ch: String = c.to_string();
                    let ch_val = Value::from_heap_ptr(HeapString::allocate(gc, &ch) as *mut u8);
                    entries.push((i.to_string(), ch_val));
                }
                Ok(entries)
            }
            TAG_STRING_OBJ => {
                let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
                let s = unsafe { HeapString::to_string(str_ptr as *mut HeapString) };
                let mut entries = Vec::with_capacity(s.len());
                for (i, c) in s.chars().enumerate() {
                    let ch: String = c.to_string();
                    let ch_val = Value::from_heap_ptr(HeapString::allocate(gc, &ch) as *mut u8);
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
            let key_val = Value::from_heap_ptr(HeapString::allocate(gc, k) as *mut u8);
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
    // §22.1.2.2 String.fromCharCode(...codeUnits): each arg → ToNumber →
    // ToUint16 → UTF-16 code unit. (Lone surrogates are unrepresentable in
    // the engine's UTF-16 storage — they decode to U+FFFD like elsewhere.)
    let mut s = String::new();
    for arg in args {
        let n = to_number(*arg);
        let unit = if n.is_nan() || n.is_infinite() {
            0
        } else {
            (n.trunc() as i64).rem_euclid(0x1_0000) as u16
        };
        if let Some(c) = char::from_u32(unit as u32) {
            s.push(c);
        } else {
            s.push('\u{FFFD}');
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
    let index = args
        .first()
        .map(|&v| to_integer_or_infinity(v).max(0.0) as usize)
        .unwrap_or(0);
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
    let s = string_from_value(this);
    let units = s.encode_utf16().collect::<Vec<u16>>();
    let len = units.len() as f64;
    let raw_start = to_number(args.first().copied().unwrap_or(Value::undefined()));
    let raw_end = args.get(1).map(|&v| to_number(v));
    let int_start = if raw_start.is_nan() { 0.0 } else { raw_start };
    let int_end = match raw_end {
        Some(e) if e.is_nan() => 0.0,
        Some(e) => e,
        None => len,
    };
    let clamp = |v: f64| -> usize {
        let v = if v.is_infinite() {
            if v.is_sign_negative() { 0.0 } else { len }
        } else if v < 0.0 {
            (len + v).max(0.0)
        } else {
            v.min(len)
        };
        v as usize
    };
    let start = clamp(int_start);
    let end = clamp(int_end);
    if start >= end {
        let empty = HeapString::allocate(gc, "");
        return Value::from_heap_ptr(empty as *mut u8);
    }
    let result = String::from_utf16_lossy(&units[start..end]);
    let heap = HeapString::allocate(gc, &result);
    Value::from_heap_ptr(heap as *mut u8)
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
    let units = s.encode_utf16().collect::<Vec<u16>>();
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    let search_units = search_str.encode_utf16().collect::<Vec<u16>>();
    let pos = args.get(1).copied().unwrap_or(Value::undefined());
    let start = if pos.is_undefined() {
        0
    } else {
        let f = to_integer_or_infinity(pos);
        if f.is_nan() || f < 0.0 {
            0
        } else {
            (f as usize).min(units.len())
        }
    };
    if search_units.is_empty() {
        return Value::smi(start as i32);
    }
    if start + search_units.len() > units.len() {
        return Value::smi(-1);
    }
    if let Some(idx) = units[start..]
        .windows(search_units.len())
        .position(|w| w == search_units)
    {
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
    let units = s.encode_utf16().collect::<Vec<u16>>();
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    let search_units = search_str.encode_utf16().collect::<Vec<u16>>();
    let pos = args.get(1).copied().unwrap_or(Value::undefined());
    let start = if pos.is_undefined() {
        0
    } else {
        let f = to_integer_or_infinity(pos);
        if f.is_nan() || f < 0.0 {
            0
        } else {
            (f as usize).min(units.len())
        }
    };
    if search_units.is_empty() {
        return Value::boolean(true);
    }
    if start + search_units.len() > units.len() {
        return Value::boolean(false);
    }
    Value::boolean(
        units[start..]
            .windows(search_units.len())
            .any(|w| w == search_units),
    )
}

/// String.prototype.startsWith(searchString, position) — checks if string starts with searchString.
pub fn string_starts_with(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let units = s.encode_utf16().collect::<Vec<u16>>();
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    let search_units = search_str.encode_utf16().collect::<Vec<u16>>();
    let pos = args.get(1).copied().unwrap_or(Value::undefined());
    let start = if pos.is_undefined() {
        0
    } else {
        let f = to_integer_or_infinity(pos);
        if f.is_nan() || f < 0.0 {
            0
        } else {
            (f as usize).min(units.len())
        }
    };
    Value::boolean(units[start..].starts_with(&search_units))
}

/// String.prototype.endsWith(searchString, endPosition) — checks if string ends with searchString.
pub fn string_ends_with(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let units = s.encode_utf16().collect::<Vec<u16>>();
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    let search_units = search_str.encode_utf16().collect::<Vec<u16>>();
    let end_pos = args.get(1).copied().unwrap_or(Value::undefined());
    let end = if end_pos.is_undefined() {
        units.len()
    } else {
        let f = to_integer_or_infinity(end_pos);
        if f.is_nan() || f < 0.0 {
            0
        } else {
            (f as usize).min(units.len())
        }
    };
    Value::boolean(units[..end].ends_with(&search_units))
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
    // §22.1.3.4: pos = ToIntegerOrInfinity(index); NaN → 0 (to_integer_or_infinity).
    let idx = to_integer_or_infinity(pos) as isize;
    if idx < 0 {
        return Value::from_float64(f64::NAN);
    }
    let units = s.encode_utf16().collect::<Vec<u16>>();
    if (idx as usize) >= units.len() {
        return Value::from_float64(f64::NAN);
    }
    Value::smi(units[idx as usize] as i32)
}

/// String.prototype.codePointAt(index) — returns Unicode code point at position
/// (decodes surrogate pairs; an isolated low surrogate returns itself).
pub fn string_code_point_at(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let pos = args.first().copied().unwrap_or(Value::undefined());
    let idx = to_integer_or_infinity(pos) as isize;
    if idx < 0 {
        return Value::undefined();
    }
    let units = s.encode_utf16().collect::<Vec<u16>>();
    let idx = idx as usize;
    if idx >= units.len() {
        return Value::undefined();
    }
    let cp = units[idx];
    if (0xD800..=0xDBFF).contains(&cp) && idx + 1 < units.len() {
        let low = units[idx + 1];
        if (0xDC00..=0xDFFF).contains(&low) {
            let code_point = 0x10000 + ((cp as u32 - 0xD800) << 10) + (low as u32 - 0xDC00);
            return Value::smi(code_point as i32);
        }
    }
    Value::smi(cp as i32)
}

/// String.prototype.substring(start, end) — returns substring with args clamped/sorted.
pub fn string_substring(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let units = s.encode_utf16().collect::<Vec<u16>>();
    let len = units.len() as f64;
    let raw_start = to_integer_or_infinity(args.first().copied().unwrap_or(Value::undefined()));
    let raw_end = args.get(1).map(|&v| to_integer_or_infinity(v));
    let final_start = raw_start.max(0.0).min(len) as usize;
    let final_end = match raw_end {
        Some(e) => e.max(0.0).min(len) as usize,
        None => units.len(),
    };
    let (lo, hi) = if final_start <= final_end {
        (final_start, final_end)
    } else {
        (final_end, final_start)
    };
    let result = String::from_utf16_lossy(&units[lo..hi]);
    let heap = HeapString::allocate(gc, &result);
    Value::from_heap_ptr(heap as *mut u8)
}

/// String.prototype.substr(start, length) — legacy, negative start offset.
pub fn string_substr(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let units = s.encode_utf16().collect::<Vec<u16>>();
    let len = units.len();
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
    let result = String::from_utf16_lossy(&units[int_start..end]);
    let heap = HeapString::allocate(gc, &result);
    Value::from_heap_ptr(heap as *mut u8)
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
pub fn string_to_lower_case(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let result = HeapString::allocate(gc, &s.to_lowercase());
    Value::from_heap_ptr(result as *mut u8)
}

/// String.prototype.toUpperCase() — returns uppercased string.
pub fn string_to_upper_case(
    gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    vm: &mut Vm,
) -> Value {
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
    // §22.1.3.28 step 8: RangeError when the result exceeds 2^53-1 units
    // (also guards usize overflow on `s.len() * n`).
    let result_units = s.encode_utf16().count() as u64 * n as u64;
    if result_units > 9_007_199_254_740_991 {
        let err = make_error(gc, "RangeError: Invalid string length");
        vm.set_pending_exception(err);
        return Value::undefined();
    }
    let mut result = String::with_capacity(s.len().saturating_mul(n).min(1 << 24));
    for _ in 0..n {
        result.push_str(&s);
    }
    let heap = HeapString::allocate(gc, &result);
    Value::from_heap_ptr(heap as *mut u8)
}

/// String.prototype.padStart(maxLength, fillString) — pads string to maxLength with fillString.
/// Lengths are measured in UTF-16 code units (§22.1.3.21); a fill truncated in
/// the middle of a surrogate pair decodes to U+FFFD (engine string model).
fn string_pad(gc: &mut SemiSpace, vm: &mut Vm, this: Value, args: &[Value], at_end: bool) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let units = s.encode_utf16().collect::<Vec<u16>>();
    let max_len = args.first().copied().unwrap_or(Value::undefined());
    let target_f = to_integer_or_infinity(max_len);
    if target_f.is_nan() || !target_f.is_finite() || target_f > 9_007_199_254_740_991.0 {
        let err = make_error(gc, "RangeError: Invalid string length");
        vm.set_pending_exception(err);
        return Value::undefined();
    }
    let target_len = target_f.max(0.0) as usize;
    if target_len <= units.len() {
        let result = HeapString::allocate(gc, &s);
        return Value::from_heap_ptr(result as *mut u8);
    }
    let fill = match args.get(1) {
        Some(v) if !v.is_undefined() => arg_to_string(gc, Some(*v), vm),
        _ => " ".to_string(),
    };
    let fill = if fill.is_empty() {
        " ".to_string()
    } else {
        fill
    };
    let fill_units = fill.encode_utf16().collect::<Vec<u16>>();
    let pad_len = target_len - units.len();
    let mut pad = Vec::with_capacity(pad_len);
    while pad.len() < pad_len {
        pad.extend_from_slice(&fill_units);
    }
    pad.truncate(pad_len);
    let pad_string = String::from_utf16_lossy(&pad);
    let result_str = if at_end {
        s + &pad_string
    } else {
        pad_string + &s
    };
    let result = HeapString::allocate(gc, &result_str);
    Value::from_heap_ptr(result as *mut u8)
}

pub fn string_pad_start(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    string_pad(gc, vm, this, args, false)
}

pub fn string_pad_end(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    string_pad(gc, vm, this, args, true)
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
    let separator = args.first().copied().unwrap_or(Value::undefined());
    let limit = args.get(1).copied().unwrap_or(Value::undefined());

    // §22.1.3.17 step 3: if separator is an object with a callable @@split,
    // dispatch to it with (this, limit).
    if let Ok(Some(())) = dispatch_symbol_method(gc, separator, SYM_SPLIT, this, &[limit], vm) {
        // fall through to legacy
    } else {
        return Value::undefined();
    }

    let lim = if limit.is_undefined() {
        u32::MAX
    } else {
        to_u32(limit)
    };
    if lim == 0 {
        let arr = RuneArray::allocate(gc, &[]);
        unsafe {
            let ptr = arr as *mut u8;
            *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
                *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
            if let Some(proto) = vm.array_prototype.heap_ptr() {
                *(ptr.add(24) as *mut *mut u8) = proto;
            }
        }
        return Value::from_heap_ptr(arr as *mut u8);
    }

    // ---- RegExp separator ----
    if let Some(ptr) = separator.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_REGEXP {
            if s.is_empty() {
                let match_result = regexp_exec_internal(gc, ptr, &s, 0);
                if match_result.is_some() {
                    return alloc_empty_array_with_proto(gc, vm);
                }
                let s_val = Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8);
                let arr = RuneArray::allocate(gc, &[]);
                set_array_proto(arr, vm);
                let result_ptr = unsafe { RuneArray::push(gc, arr, s_val) };
                return Value::from_heap_ptr(result_ptr as *mut u8);
            }

            let size = s.len();
            let mut pieces: Vec<String> = Vec::new();
            let mut last_match_end = 0usize;
            let mut search_index = last_match_end;

            while search_index < size {
                let match_result = regexp_exec_internal(gc, ptr, &s, search_index);
                match match_result {
                    Some(groups) => {
                        let (match_start, match_end) = groups[0];
                        if match_end == last_match_end {
                            search_index += 1;
                            if search_index > size {
                                search_index = size;
                            }
                            continue;
                        }
                        let substring = s[last_match_end..match_start].to_string();
                        pieces.push(substring);
                        if pieces.len() as u32 >= lim {
                            return alloc_split_array(gc, vm, &pieces, lim);
                        }
                        last_match_end = match_end;
                        for g in &groups[1..] {
                            let (gs, ge) = *g;
                            let cap = s[gs..ge].to_string();
                            pieces.push(cap);
                            if pieces.len() as u32 >= lim {
                                return alloc_split_array(gc, vm, &pieces, lim);
                            }
                        }
                        search_index = last_match_end;
                    }
                    None => {
                        search_index += 1;
                        if search_index > size {
                            search_index = size;
                        }
                    }
                }
            }
            let trailing = s[last_match_end..].to_string();
            pieces.push(trailing);
            return alloc_split_array(gc, vm, &pieces, lim);
        }
    }

    // ---- String separator ----
    if separator.is_undefined() {
        let s_val = Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8);
        let arr = RuneArray::allocate(gc, &[]);
        set_array_proto(arr, vm);
        let result_ptr = unsafe { RuneArray::push(gc, arr, s_val) };
        Value::from_heap_ptr(result_ptr as *mut u8)
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
            *(arr_ptr.add(8) as *mut *const rune_core::shape::Shape) =
                *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
            if let Some(proto) = vm.array_prototype.heap_ptr() {
                *(arr_ptr.add(24) as *mut *mut u8) = proto;
            }
            for p in pieces.iter().take(elem_count) {
                let heap_str = HeapString::allocate(gc, p);
                let new_ptr = RuneArray::push(
                    gc,
                    arr_ptr as *mut RuneArray,
                    Value::from_heap_ptr(heap_str as *mut u8),
                );
                if new_ptr as *mut u8 != arr_ptr {
                    arr_ptr = new_ptr as *mut u8;
                }
            }
            Value::from_heap_ptr(arr_ptr)
        }
    }
}

/// String.prototype.replace(searchValue, replaceValue) — first match only.
/// Supports string and RegExp patterns, including function replacement.
pub fn string_replace(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let search = args.first().copied().unwrap_or(Value::undefined());
    let replacement_fn = args.get(1).copied();
    let is_fn_replacement = replacement_fn.is_some_and(|v| {
        v.heap_ptr()
            .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_FUNC })
    });

    // §21.1.3.18 step 4: @@replace dispatch on the searchValue.
    if let Ok(Some(())) = dispatch_symbol_method(gc, search, SYM_REPLACE, this, &[], vm) {
        // fall through to legacy
    } else {
        return Value::undefined();
    }

    // Check if search is a RegExp
    if let Some(ptr) = search.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_REGEXP {
            let pattern_ptr = unsafe { RegExp::pattern(ptr) };
            let pattern = unsafe { HeapString::to_string(pattern_ptr as *mut HeapString) };
            // Parse and execute regex
            match rune_regex::parse_regex(&pattern) {
                Ok(expr) => {
                    let nfa = rune_regex::nfa::compile(&expr);
                    let pike_vm = rune_regex::pikevm::PikeVm::new();
                    if let Some(m) = pike_vm.exec(&nfa, &s, 0) {
                        let (start, end) = m.groups[0];
                        if is_fn_replacement {
                            let fn_val = replacement_fn.unwrap();
                            let mut fn_args = Vec::with_capacity(m.groups.len() + 2);
                            // Full match
                            let match_str = HeapString::allocate(gc, &s[start..end]);
                            fn_args.push(Value::from_heap_ptr(match_str as *mut u8));
                            // Captures (groups[1..])
                            for i in 1..m.groups.len() {
                                let (gs, ge) = m.groups[i];
                                let cap_str = HeapString::allocate(gc, &s[gs..ge]);
                                fn_args.push(Value::from_heap_ptr(cap_str as *mut u8));
                            }
                            // Offset and input
                            fn_args.push(Value::smi(start as i32));
                            let input_str = HeapString::allocate(gc, &s);
                            fn_args.push(Value::from_heap_ptr(input_str as *mut u8));
                            vm.pending_replace_op = Some(crate::vm::PendingReplaceOp {
                                source_frame_depth: 0,
                                input: s,
                                groups: m.groups,
                            });
                            vm.push_callback_call(gc, fn_val, Value::undefined(), fn_args);
                            return Value::undefined();
                        }
                        // String replacement (original logic)
                        let replacement = arg_to_string(gc, replacement_fn, vm);
                        let expanded = expand_replacement(&s, &m.groups, &replacement);
                        let result = s[..start].to_string() + &expanded + &s[end..];
                        return Value::from_heap_ptr(HeapString::allocate(gc, &result) as *mut u8);
                    } else {
                        return Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8);
                    }
                }
                Err(_) => {
                    // Bad regex — return original string
                    return Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8);
                }
            }
        }
    }

    // String pattern
    let replacement_str = arg_to_string(gc, replacement_fn, vm);
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    if search_str.is_empty() {
        let result = replacement_str.clone() + &s;
        return Value::from_heap_ptr(HeapString::allocate(gc, &result) as *mut u8);
    }
    if let Some(pos) = s.find(&search_str) {
        let result = s[..pos].to_string() + &replacement_str + &s[pos + search_str.len()..];
        Value::from_heap_ptr(HeapString::allocate(gc, &result) as *mut u8)
    } else {
        Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8)
    }
}

/// Expand $&, $`, $', $1..$n in a replacement string for regex match.
fn expand_replacement(s: &str, groups: &[(usize, usize)], replacement: &str) -> String {
    let mut result = String::new();
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' {
            match chars.next() {
                Some('&') => result.push_str(&s[groups[0].0..groups[0].1]),
                Some('`') => result.push_str(&s[..groups[0].0]),
                Some('\'') => result.push_str(&s[groups[0].1..]),
                Some(d) if d.is_ascii_digit() => {
                    let mut n = (d as u8 - b'0') as usize;
                    // Check for two-digit
                    if let Some(&d2) = chars.peek() {
                        if d2.is_ascii_digit() {
                            let n2 = (d2 as u8 - b'0') as usize;
                            let combined = n * 10 + n2;
                            if combined < groups.len() {
                                n = combined;
                                chars.next();
                            }
                        }
                    }
                    if n < groups.len() {
                        let (gs, ge) = groups[n];
                        result.push_str(&s[gs..ge]);
                    } else {
                        result.push('$');
                        result.push(char::from_digit(n as u32, 10).unwrap());
                    }
                }
                Some(d) => {
                    result.push('$');
                    result.push(d);
                }
                None => result.push('$'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// String.prototype.replaceAll(searchValue, replaceValue) — replace all non-overlapping matches.
/// Supports string and RegExp patterns.
pub fn string_replace_all(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let search = args.first().copied().unwrap_or(Value::undefined());
    let replacement_fn = args.get(1).copied();
    let is_fn_replacement = replacement_fn.is_some_and(|v| {
        v.heap_ptr()
            .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_FUNC })
    });

    // Check if search is a RegExp
    if let Some(ptr) = search.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_REGEXP {
            let pattern_ptr = unsafe { RegExp::pattern(ptr) };
            let pattern = unsafe { HeapString::to_string(pattern_ptr as *mut HeapString) };
            match rune_regex::parse_regex(&pattern) {
                Ok(expr) => {
                    let nfa = rune_regex::nfa::compile(&expr);
                    let pike_vm = rune_regex::pikevm::PikeVm::new();
                    if is_fn_replacement {
                        // Callable replacement: state machine in the Return
                        // handler re-invokes fn per match (spec §22.1.3.20
                        // @@replace path: fn(match, ...captures, position, string)).
                        let fn_val = replacement_fn.unwrap();
                        match pike_vm.exec(&nfa, &s, 0) {
                            Some(m) => {
                                let (start, end) = m.groups[0];
                                let mut fn_args = Vec::with_capacity(m.groups.len() + 2);
                                let match_str = HeapString::allocate(gc, &s[start..end]);
                                fn_args.push(Value::from_heap_ptr(match_str as *mut u8));
                                for i in 1..m.groups.len() {
                                    let (gs, ge) = m.groups[i];
                                    let cap_str = HeapString::allocate(gc, &s[gs..ge]);
                                    fn_args.push(Value::from_heap_ptr(cap_str as *mut u8));
                                }
                                fn_args.push(Value::smi(start as i32));
                                let input_str = HeapString::allocate(gc, &s);
                                fn_args.push(Value::from_heap_ptr(input_str as *mut u8));
                                let empty = start == end;
                                vm.pending_replace_all_op = Some(crate::vm::PendingReplaceAllOp {
                                    source_frame_depth: 0,
                                    input: s.clone(),
                                    search_str: String::new(),
                                    regex_pattern: Some(pattern.clone()),
                                    fn_val,
                                    next_pos: if empty { start + 1 } else { end },
                                    accumulated: s[..start].to_string(),
                                    last_end: end,
                                });
                                vm.push_callback_call(gc, fn_val, Value::undefined(), fn_args);
                                return Value::undefined();
                            }
                            None => {
                                return Value::from_heap_ptr(
                                    HeapString::allocate(gc, &s) as *mut u8
                                );
                            }
                        }
                    }
                    let replacement = arg_to_string(gc, replacement_fn, vm);
                    let mut result = String::new();
                    let mut last_end = 0;
                    while let Some(m) = pike_vm.exec(&nfa, &s, last_end) {
                        let (start, end) = m.groups[0];
                        result.push_str(&s[last_end..start]);
                        result.push_str(&expand_replacement(&s, &m.groups, &replacement));
                        last_end = end;
                        if start == end {
                            // Avoid infinite loop for zero-length matches
                            result.push_str(&s[last_end..last_end + 1]);
                            last_end += 1;
                        }
                    }
                    result.push_str(&s[last_end..]);
                    return Value::from_heap_ptr(HeapString::allocate(gc, &result) as *mut u8);
                }
                Err(_) => {
                    return Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8);
                }
            }
        }
    }

    // String pattern (original logic)
    let search_str = arg_to_string(gc, args.first().copied(), vm);
    if is_fn_replacement {
        // Callable replacement for a string search:
        // fn(searchString, position, string) per occurrence.
        let fn_val = replacement_fn.unwrap();
        let find_pos = if search_str.is_empty() {
            Some(0)
        } else {
            s.find(&search_str)
        };
        if let Some(start) = find_pos {
            let end = start + search_str.len();
            let mut fn_args = Vec::with_capacity(3);
            let ss = HeapString::allocate(gc, &search_str);
            fn_args.push(Value::from_heap_ptr(ss as *mut u8));
            fn_args.push(Value::smi(start as i32));
            let input_str = HeapString::allocate(gc, &s);
            fn_args.push(Value::from_heap_ptr(input_str as *mut u8));
            let empty = search_str.is_empty();
            let advance = if empty {
                s[start..].chars().next().map(|c| c.len_utf8()).unwrap_or(1)
            } else {
                end - start
            };
            vm.pending_replace_all_op = Some(crate::vm::PendingReplaceAllOp {
                source_frame_depth: 0,
                input: s.clone(),
                search_str: search_str.clone(),
                regex_pattern: None,
                fn_val,
                next_pos: start + advance,
                accumulated: s[..start].to_string(),
                last_end: end,
            });
            vm.push_callback_call(gc, fn_val, Value::undefined(), fn_args);
            return Value::undefined();
        }
        return Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8);
    }
    let replacement = arg_to_string(gc, replacement_fn, vm);
    if search_str.is_empty() {
        let result = s
            .chars()
            .map(|c| replacement.clone() + &c.to_string())
            .collect::<String>()
            + &replacement;
        return Value::from_heap_ptr(HeapString::allocate(gc, &result) as *mut u8);
    }
    let result = s.replace(&search_str, &replacement);
    Value::from_heap_ptr(HeapString::allocate(gc, &result) as *mut u8)
}

fn has_regexp_flag(regexp_ptr: *mut u8, flag: u8) -> bool {
    unsafe { RegExp::has_flag(regexp_ptr, flag) }
}

fn regexp_exec_internal(
    _gc: &mut SemiSpace,
    regexp_ptr: *mut u8,
    input: &str,
    start_pos: usize,
) -> Option<Vec<(usize, usize)>> {
    let pattern = unsafe { HeapString::to_string(RegExp::pattern(regexp_ptr) as *mut HeapString) };
    match rune_regex::parse_regex(&pattern) {
        Ok(expr) => {
            let nfa = rune_regex::nfa::compile(&expr);
            let pike_vm = rune_regex::pikevm::PikeVm::new();
            pike_vm.exec(&nfa, input, start_pos).map(|m| m.groups)
        }
        Err(_) => None,
    }
}

fn alloc_regexp_from_string(
    gc: &mut SemiSpace,
    pattern: &str,
    flags: u32,
    regexp_proto: Value,
) -> Value {
    let pattern_str = HeapString::allocate(gc, pattern);
    let ptr = rune_core::regexp::RegExp::allocate(gc, pattern_str as *mut u8, flags);
    if let Some(proto_ptr) = regexp_proto.heap_ptr() {
        unsafe {
            rune_core::regexp::RegExp::set_prototype(ptr, proto_ptr);
        }
    }
    Value::from_heap_ptr(ptr)
}

fn make_match_result_array(
    gc: &mut SemiSpace,
    groups: &[(usize, usize)],
    input: &str,
    match_index: usize,
    array_proto: Value,
) -> Value {
    let mut elements = Vec::with_capacity(groups.len());
    for (gs, ge) in groups.iter() {
        let s = HeapString::allocate(gc, &input[*gs..*ge]);
        elements.push(Value::from_heap_ptr(s as *mut u8));
    }
    let arr = RuneArray::allocate(gc, &elements);
    unsafe {
        let ptr = arr as *mut u8;
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
            *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
        if let Some(proto) = array_proto.heap_ptr() {
            *(ptr.add(24) as *mut *mut u8) = proto;
        }
        // §22.2.7.2 steps 18-19: non-enumerable "index" and "input" data
        // properties. Stored in extra_props (never enumerated by
        // for-in/Object.keys — matches the spec's non-enumerability).
        let props = JSObject::allocate(gc, Shape::empty(), &[]);
        let input_str = HeapString::allocate(gc, input);
        JSObject::add_property(
            props,
            PropertyKey::from_string("index"),
            "index".to_string(),
            Value::smi(match_index as i32),
        );
        JSObject::add_property(
            props,
            PropertyKey::from_string("input"),
            "input".to_string(),
            Value::from_heap_ptr(input_str as *mut u8),
        );
        RuneArray::set_extra_props(ptr as *mut RuneArray, props as *mut u8);
    }
    Value::from_heap_ptr(arr as *mut u8)
}

fn set_array_proto(arr: *mut RuneArray, vm: &Vm) {
    unsafe {
        let ptr = arr as *mut u8;
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
            *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(ptr.add(24) as *mut *mut u8) = proto;
        }
    }
}

fn alloc_empty_array_with_proto(gc: &mut SemiSpace, vm: &Vm) -> Value {
    let arr = RuneArray::allocate(gc, &[]);
    set_array_proto(arr, vm);
    Value::from_heap_ptr(arr as *mut u8)
}

fn alloc_split_array(gc: &mut SemiSpace, vm: &Vm, pieces: &[String], lim: u32) -> Value {
    let elem_count = (pieces.len() as u32).min(lim) as usize;
    let arr = RuneArray::allocate(gc, &[]);
    unsafe {
        let mut arr_ptr = arr as *mut u8;
        *(arr_ptr.add(8) as *mut *const rune_core::shape::Shape) =
            *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(arr_ptr.add(24) as *mut *mut u8) = proto;
        }
        for p in pieces.iter().take(elem_count) {
            let heap_str = HeapString::allocate(gc, p);
            let new_ptr = RuneArray::push(
                gc,
                arr_ptr as *mut RuneArray,
                Value::from_heap_ptr(heap_str as *mut u8),
            );
            if new_ptr as *mut u8 != arr_ptr {
                arr_ptr = new_ptr as *mut u8;
            }
        }
        Value::from_heap_ptr(arr_ptr)
    }
}

fn value_to_pattern_string(v: Option<Value>, gc: &mut SemiSpace, vm: &mut Vm) -> String {
    match v {
        Some(val) if !val.is_undefined() && !val.is_null() => arg_to_string(gc, v, vm),
        _ => String::new(),
    }
}

pub fn string_match(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let regexp_val = args.first().copied();

    // §21.1.3.39 step 2: if regexp is an object with a callable @@match,
    // dispatch to it and return its result.
    if let Some(v) = regexp_val {
        if let Ok(Some(())) = dispatch_symbol_method(gc, v, SYM_MATCH, this, &[], vm) {
            // fall through to legacy
        } else {
            return Value::undefined();
        }
    }

    let regexp_ptr = regexp_val.and_then(|v| {
        v.heap_ptr().and_then(|ptr| {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_REGEXP { Some(ptr) } else { None }
        })
    });

    let (rx_ptr, _rx_owned) = if let Some(ptr) = regexp_ptr {
        (ptr, false)
    } else {
        let pattern_str = value_to_pattern_string(regexp_val, gc, vm);
        let rx = alloc_regexp_from_string(gc, &pattern_str, 0, vm.regexp_prototype);
        match rx.heap_ptr() {
            Some(p) => (p, true),
            None => return Value::null(),
        }
    };

    let is_global = has_regexp_flag(rx_ptr, 0u8);

    if !is_global {
        match regexp_exec_internal(gc, rx_ptr, &s, 0) {
            Some(groups) => {
                let match_index = groups[0].0;
                make_match_result_array(gc, &groups, &s, match_index, vm.array_prototype)
            }
            None => Value::null(),
        }
    } else {
        unsafe {
            RegExp::set_last_index(rx_ptr, 0);
        }
        let mut matched_strings: Vec<String> = Vec::new();
        loop {
            let last_index = unsafe { RegExp::last_index(rx_ptr) } as usize;
            match regexp_exec_internal(gc, rx_ptr, &s, last_index) {
                Some(groups) => {
                    let (gs, ge) = groups[0];
                    let match_str = &s[gs..ge];
                    matched_strings.push(match_str.to_string());
                    let next_start = if match_str.is_empty() {
                        if gs < s.len() { gs + 1 } else { s.len() }
                    } else {
                        ge
                    };
                    unsafe {
                        RegExp::set_last_index(rx_ptr, next_start as u32);
                    }
                }
                None => break,
            }
        }
        if matched_strings.is_empty() {
            return Value::null();
        }
        let mut elements = Vec::with_capacity(matched_strings.len());
        for ms in &matched_strings {
            let heap_str = HeapString::allocate(gc, ms);
            elements.push(Value::from_heap_ptr(heap_str as *mut u8));
        }
        let arr = RuneArray::allocate(gc, &elements);
        unsafe {
            let ptr = arr as *mut u8;
            *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
                *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
            if let Some(proto) = vm.array_prototype.heap_ptr() {
                *(ptr.add(24) as *mut *mut u8) = proto;
            }
        }
        Value::from_heap_ptr(arr as *mut u8)
    }
}

pub fn string_search(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let s = string_from_value(this);
    let regexp_val = args.first().copied();

    // §21.1.3.22 step 2: @@search dispatch.
    if let Some(v) = regexp_val {
        if let Ok(Some(())) = dispatch_symbol_method(gc, v, SYM_SEARCH, this, &[], vm) {
            // fall through to legacy
        } else {
            return Value::undefined();
        }
    }

    let regexp_ptr = regexp_val.and_then(|v| {
        v.heap_ptr().and_then(|ptr| {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_REGEXP { Some(ptr) } else { None }
        })
    });

    let (rx_ptr, _rx_owned) = if let Some(ptr) = regexp_ptr {
        (ptr, false)
    } else {
        let pattern_str = value_to_pattern_string(regexp_val, gc, vm);
        let rx = alloc_regexp_from_string(gc, &pattern_str, 0, vm.regexp_prototype);
        match rx.heap_ptr() {
            Some(p) => (p, true),
            None => return Value::smi(-1),
        }
    };

    // §22.2.6.12 steps 4-8: lastIndex is reset to 0 before the exec and
    // restored afterwards — search ignores the "lastIndex"/"global" state.
    let prev_li = unsafe { RegExp::last_index(rx_ptr) };
    if prev_li != 0 {
        unsafe { RegExp::set_last_index(rx_ptr, 0) };
    }
    let result = match regexp_exec_internal(gc, rx_ptr, &s, 0) {
        Some(groups) => Value::smi(groups[0].0 as i32),
        None => Value::smi(-1),
    };
    unsafe { RegExp::set_last_index(rx_ptr, prev_li) };
    result
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
    if chars[i] == '-' {
        sign = -1.0;
        i += 1;
    } else if chars[i] == '+' {
        i += 1;
    }
    if i >= chars.len() {
        return Value::from_float64(f64::NAN);
    }
    // Determine radix
    let radix = if args.len() > 1 {
        let r = args[1];
        if r.is_undefined() {
            0
        } else {
            r.as_smi()
                .or_else(|| r.as_float64().map(|f| f as i32))
                .unwrap_or(0)
        }
    } else {
        0
    };
    let radix = if radix == 0 {
        if i + 2 <= chars.len() && chars[i] == '0' && (chars[i + 1] == 'x' || chars[i + 1] == 'X') {
            16
        } else {
            10
        }
    } else {
        radix
    };
    if !(2..=36).contains(&radix) {
        return Value::from_float64(f64::NAN);
    }
    if radix == 16
        && i + 2 <= chars.len()
        && chars[i] == '0'
        && (chars[i + 1] == 'x' || chars[i + 1] == 'X')
    {
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
        if d >= radix {
            break;
        }
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
pub fn parse_float_builtin(
    _gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    _vm: &mut Vm,
) -> Value {
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
        let prefix = &s[end..end + 8];
        if prefix == "Infinity" {
            return Value::from_float64(f64::INFINITY);
        }
    }
    // Check for NaN (case-insensitive)
    if end + 3 <= chars.len() {
        let na: String = chars[end..end + 3].iter().collect();
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
                                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                                        if let Some(ch) = char::from_u32(code) {
                                            s.push(ch);
                                        }
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
                        let val = parse_value(gc, chars, pos, array_proto, object_proto)?;
                        elements.push(val);
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
                                            if let Ok(code) = u32::from_str_radix(&hex, 16) {
                                                if let Some(ch) = char::from_u32(code) {
                                                    key.push(ch);
                                                }
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
                        let val = parse_value(gc, chars, pos, array_proto, object_proto)?;
                        keys.push(key);
                        values.push(val);
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
    fn stringify_val(
        gc: &mut SemiSpace,
        val: Value,
        stack: &mut Vec<*mut u8>,
        vm: &mut Vm,
    ) -> Result<String, ()> {
        if val.is_undefined() {
            return Err(());
        }
        if val.is_null() {
            return Ok("null".to_string());
        }
        if val.is_boolean() {
            return Ok(if val.to_boolean().unwrap() {
                "true"
            } else {
                "false"
            }
            .to_string());
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
                    parts.push(
                        stringify_val(gc, elem, stack, vm).unwrap_or_else(|_| "null".to_string()),
                    );
                }
                stack.pop();
                return Ok(format!("[{}]", parts.join(",")));
            }
            if tag == TAG_OBJECT {
                if stack.contains(&ptr) {
                    let msg = HeapString::allocate(
                        gc,
                        "TypeError: Converting circular structure to JSON",
                    );
                    vm.set_pending_exception(Value::from_heap_ptr(msg as *mut u8));
                    return Err(());
                }
                stack.push(ptr);
                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                let count = unsafe { JSObject::slot_count(ptr as *mut JSObject) };
                let mut pairs: Vec<String> = Vec::new();
                for i in 0..count {
                    // §25.5: symbol-keyed properties are not serialized.
                    if shape.entries[i].0.is_symbol() {
                        continue;
                    }
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
    if let Some(smi) = target.as_smi() {
        if smi < 0 {
            let id = ((-smi) as usize) - 1;
            if id < vm.builtins.len() {
                return (vm.builtins[id].func)(_gc, new_this, &call_args, vm);
            }
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
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
            *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
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
        return if n < 0 {
            length.saturating_sub(n.unsigned_abs())
        } else {
            (n as u32).min(length)
        };
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
            if n.is_nan() || n < 0.0 {
                0
            } else {
                (n as u32).min(length)
            }
        } else {
            0
        }
    } else {
        0
    }
}

/// Array.prototype.indexOf(searchElement, fromIndex) — returns index of first match, -1 if not found.
pub fn array_index_of(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let search = args.first().copied().unwrap_or(Value::undefined());
    let len = crate::vm::array_like_length(this).unwrap_or(0) as usize;
    let from = to_index(args.get(1).copied().unwrap_or(Value::smi(0)), len as u32) as usize;
    if from >= len {
        return Value::smi(-1);
    }
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
                    || (elem.is_boolean()
                        && search.is_boolean()
                        && elem.as_smi() == search.as_smi());
            }
            if eq {
                return Value::smi(i as i32);
            }
        }
    }
    Value::smi(-1)
}

/// Array.prototype.join(separator) — §23.1.3.17. Concatenates the array
/// elements (undefined/null → "") separated by the separator (default ",").
pub fn array_join(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let length = match crate::vm::array_like_length(this) {
        Some(len) => len,
        None => return Value::from_heap_ptr(crate::vm::heap_string(gc, "")),
    };
    let sep = match args.first().copied().unwrap_or(Value::undefined()) {
        v if v.is_undefined() => ",".to_string(),
        v => value_to_js_string(v),
    };
    if length == 0 {
        return Value::from_heap_ptr(crate::vm::heap_string(gc, ""));
    }
    let mut parts: Vec<String> = Vec::new();
    for i in 0..length {
        let elem = crate::vm::array_like_index(this, i).unwrap_or(Value::undefined());
        let next = if elem.is_undefined() || elem.is_null() {
            String::new()
        } else {
            value_to_js_string(elem)
        };
        parts.push(next);
    }
    let joined = parts.join(&sep);
    Value::from_heap_ptr(crate::vm::heap_string(gc, &joined))
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
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
            *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
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
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
            *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
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
        let msg =
            HeapString::allocate(gc, "TypeError: reduce of empty array with no initial value");
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
    let element =
        crate::vm::array_like_index(this, start_index as u32).unwrap_or(Value::undefined());
    vm.push_callback_call(
        gc,
        callback,
        Value::undefined(),
        vec![accumulator, element, Value::smi(start_index as i32), this],
    );
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
        if depth_num.is_sign_negative() {
            0
        } else {
            u32::MAX
        }
    } else {
        depth_num.max(0.0) as u32
    };
    fn flatten(gc: &mut SemiSpace, vm: &Vm, arr_val: Value, depth: u32) -> *mut u8 {
        let result_arr = RuneArray::allocate(gc, &[]);
        let mut result_ptr = result_arr as *mut u8;
        unsafe {
            *(result_ptr.add(8) as *mut *const rune_core::shape::Shape) =
                *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
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
                        let flat_elem =
                            RuneArray::get_element(flattened as *mut RuneArray, j as usize);
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
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
            *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
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
    let resolve_handle = vm
        .get_builtin("_promise_resolve")
        .unwrap_or(Value::undefined());
    let reject_handle = vm
        .get_builtin("_promise_reject")
        .unwrap_or(Value::undefined());
    let executor = args.first().copied().unwrap_or(Value::undefined());
    if executor.is_undefined() {
        return promise_val;
    }
    let resolve_func = vm.create_promise_bridge(gc, promise_val, resolve_handle);
    let reject_func = vm.create_promise_bridge(gc, promise_val, reject_handle);
    vm.pending_promise_ctor = Some(crate::vm::PendingPromiseCtor {
        source_frame_depth: 0,
        promise: promise_val,
        resolve_handle,
        reject_handle,
        resolve_with_result: false,
    });
    vm.push_callback_call(
        gc,
        executor,
        Value::undefined(),
        vec![resolve_func, reject_func],
    );
    Value::undefined()
}

/// Internal: resolve a promise. Promise is `this`.
pub fn promise_resolve_impl(
    _gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_PROMISE && unsafe { Promise::state(ptr) == PROMISE_PENDING } {
            let val = args.first().copied().unwrap_or(Value::undefined());
            unsafe {
                Promise::set_state(ptr, PROMISE_FULFILLED);
                Promise::set_result(ptr, val);
            }
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
                            source_frame_depth: 0,
                            promise: chained,
                            resolve_handle: Value::undefined(),
                            reject_handle: Value::undefined(),
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
            unsafe {
                Promise::set_state(ptr, PROMISE_REJECTED);
                Promise::set_result(ptr, reason);
            }
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
                            source_frame_depth: 0,
                            promise: chained,
                            resolve_handle: Value::undefined(),
                            reject_handle: Value::undefined(),
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
pub fn promise_prototype_then(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let ptr = match this.heap_ptr() {
        Some(p) => p,
        None => return Value::undefined(),
    };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag != TAG_PROMISE {
        return Value::undefined();
    }
    let state = unsafe { Promise::state(ptr) };
    let result = unsafe { Promise::result(ptr) };
    let on_fulfilled = args.first().copied().unwrap_or(Value::undefined());
    let on_rejected = args.get(1).copied().unwrap_or(Value::undefined());
    let proto = vm.promise_prototype.heap_ptr();
    let new_promise_ptr = Promise::allocate(gc, proto);
    let new_promise = Value::from_heap_ptr(new_promise_ptr);
    if state == PROMISE_FULFILLED {
        if let Some(op) = on_fulfilled.heap_ptr() {
            if unsafe { (*(op as *const GcHeader)).tag() == TAG_FUNC } {
                let ppc = crate::vm::PendingPromiseCtor {
                    source_frame_depth: 0,
                    promise: new_promise,
                    resolve_handle: Value::undefined(),
                    reject_handle: Value::undefined(),
                    resolve_with_result: true,
                };
                vm.enqueue_microtask(on_fulfilled, vec![result], Some(ppc));
                return new_promise;
            }
        }
        unsafe {
            Promise::set_state(new_promise_ptr, PROMISE_FULFILLED);
            Promise::set_result(new_promise_ptr, result);
        }
        return new_promise;
    }
    if state == PROMISE_REJECTED {
        if let Some(op) = on_rejected.heap_ptr() {
            if unsafe { (*(op as *const GcHeader)).tag() == TAG_FUNC } {
                let ppc = crate::vm::PendingPromiseCtor {
                    source_frame_depth: 0,
                    promise: new_promise,
                    resolve_handle: Value::undefined(),
                    reject_handle: Value::undefined(),
                    resolve_with_result: true,
                };
                vm.enqueue_microtask(on_rejected, vec![result], Some(ppc));
                return new_promise;
            }
        }
        unsafe {
            Promise::set_state(new_promise_ptr, PROMISE_REJECTED);
            Promise::set_result(new_promise_ptr, result);
        }
        return new_promise;
    }
    // Pending — store reaction in the promise's reactions array
    let reactions_ptr = unsafe { Promise::reactions(ptr) };
    if !reactions_ptr.is_null() {
        unsafe {
            RuneArray::push(gc, reactions_ptr as *mut RuneArray, on_fulfilled);
        }
        unsafe {
            RuneArray::push(gc, reactions_ptr as *mut RuneArray, new_promise);
        }
    }
    new_promise
}

/// Promise.prototype.catch(onRejected)
pub fn promise_prototype_catch(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    promise_prototype_then(
        gc,
        this,
        &[
            Value::undefined(),
            args.first().copied().unwrap_or(Value::undefined()),
        ],
        vm,
    )
}

/// Promise.prototype.finally(onFinally) — calls onFinally when settled, passes through original result.
pub fn promise_prototype_finally(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let on_finally = args.first().copied().unwrap_or(Value::undefined());
    let ptr = match this.heap_ptr() {
        Some(p) => p,
        None => return Value::undefined(),
    };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag != TAG_PROMISE {
        return Value::undefined();
    }
    let state = unsafe { Promise::state(ptr) };
    let result = unsafe { Promise::result(ptr) };
    let proto = vm.promise_prototype.heap_ptr();
    let new_promise_ptr = Promise::allocate(gc, proto);
    let new_promise = Value::from_heap_ptr(new_promise_ptr);

    // If on_finally is not callable, propagate the original result directly
    if !on_finally.is_heap_object()
        || unsafe { (*(on_finally.heap_ptr().unwrap() as *const GcHeader)).tag() != TAG_FUNC }
    {
        if state == PROMISE_FULFILLED || state == PROMISE_REJECTED {
            unsafe {
                Promise::set_state(new_promise_ptr, state);
                Promise::set_result(new_promise_ptr, result);
            }
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
pub fn promise_static_resolve(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let val = args.first().copied().unwrap_or(Value::undefined());

    // §27.2.4.1.2 Promise.resolve: if already a native Promise, return as-is
    if let Some(ptr) = val.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() == TAG_PROMISE } {
            return val;
        }
    }

    // §27.2.4.1.1 PromiseResolve: thenable unwrapping for objects with .then callable
    if val.heap_ptr().is_some() {
        let then_str = HeapString::allocate(gc, "then");
        let then_key = Value::from_heap_ptr(then_str as *mut u8);
        let then_val = load_property_recursive(val, then_key, Some(vm.function_prototype), gc);
        if let Some(then_ptr) = then_val.heap_ptr() {
            let then_tag = unsafe { (*(then_ptr as *const GcHeader)).tag() };
            if then_tag == TAG_FUNC {
                let promise_ptr = Promise::allocate(gc, vm.promise_prototype.heap_ptr());
                let promise_val = Value::from_heap_ptr(promise_ptr);
                let resolve_h = vm
                    .get_builtin("_promise_resolve")
                    .unwrap_or(Value::undefined());
                let reject_h = vm
                    .get_builtin("_promise_reject")
                    .unwrap_or(Value::undefined());
                let resolve_bridge = vm.create_promise_bridge(gc, promise_val, resolve_h);
                let reject_bridge = vm.create_promise_bridge(gc, promise_val, reject_h);
                vm.pending_promise_ctor = Some(crate::vm::PendingPromiseCtor {
                    source_frame_depth: 0,
                    promise: promise_val,
                    resolve_handle: resolve_h,
                    reject_handle: reject_h,
                    resolve_with_result: false,
                });
                vm.push_callback_call(gc, then_val, val, vec![resolve_bridge, reject_bridge]);
                return Value::undefined();
            }
        }
    }

    let ptr = Promise::allocate(gc, vm.promise_prototype.heap_ptr());
    unsafe {
        Promise::set_state(ptr, PROMISE_FULFILLED);
        Promise::set_result(ptr, val);
    }
    Value::from_heap_ptr(ptr)
}

/// Promise.reject(reason) — returns a rejected promise.
pub fn promise_static_reject(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let val = args.first().copied().unwrap_or(Value::undefined());
    let ptr = Promise::allocate(gc, vm.promise_prototype.heap_ptr());
    unsafe {
        Promise::set_state(ptr, PROMISE_REJECTED);
        Promise::set_result(ptr, val);
    }
    Value::from_heap_ptr(ptr)
}

/// Async generator continuation: resumes an async generator with a resolved value.
/// Called via bridge function: async_continue(this=gen_id_smi, args=[value])
pub fn async_continue(_gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let gen_id = this.as_smi().unwrap_or(0) as usize;
    let value = args.first().copied().unwrap_or(Value::undefined());
    vm.pending_async_gen = Some(crate::vm::PendingAsyncGen {
        gen_id,
        arg: value,
        is_throw: false,
    });
    Value::undefined()
}

/// Async generator rejection: resumes an async generator with a thrown error.
/// Called via bridge function: async_reject(this=gen_id_smi, args=[reason])
pub fn async_reject(_gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let gen_id = this.as_smi().unwrap_or(0) as usize;
    let reason = args.first().copied().unwrap_or(Value::undefined());
    vm.pending_async_gen = Some(crate::vm::PendingAsyncGen {
        gen_id,
        arg: reason,
        is_throw: true,
    });
    Value::undefined()
}

/// Promise.all(iterable) — returns a promise that fulfills when all items fulfill,
/// or rejects on the first rejection.
pub fn promise_static_all(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let iterable = args.first().copied().unwrap_or(Value::undefined());
    let proto = vm.promise_prototype.heap_ptr();
    let result_ptr = Promise::allocate(gc, proto);
    let result_val = Value::from_heap_ptr(result_ptr);
    let len = if let Some(l) = crate::vm::array_like_length(iterable) {
        l
    } else {
        unsafe {
            Promise::set_state(result_ptr, PROMISE_FULFILLED);
        }
        return result_val;
    };
    if len == 0 {
        let arr = RuneArray::allocate(gc, &[]);
        unsafe {
            Promise::set_state(result_ptr, PROMISE_FULFILLED);
            Promise::set_result(result_ptr, Value::from_heap_ptr(arr as *mut u8));
        }
        return result_val;
    }
    let mut arr_ptr = RuneArray::allocate(gc, &[]);
    let mut remaining: u32 = len;
    for i in 0..len {
        let item = crate::vm::array_like_index(iterable, i).unwrap_or(Value::undefined());
        let is_promise = if let Some(ptr) = item.heap_ptr() {
            unsafe { (*(ptr as *const GcHeader)).tag() == TAG_PROMISE }
        } else {
            false
        };
        if is_promise {
            let ptr = item.heap_ptr().unwrap();
            let state = unsafe { Promise::state(ptr) };
            if state == PROMISE_FULFILLED {
                let r = unsafe { Promise::result(ptr) };
                arr_ptr = unsafe { RuneArray::push(gc, arr_ptr, r) };
                remaining -= 1;
            } else if state == PROMISE_REJECTED {
                let r = unsafe { Promise::result(ptr) };
                unsafe {
                    Promise::set_state(result_ptr, PROMISE_REJECTED);
                    Promise::set_result(result_ptr, r);
                }
                return result_val;
            }
        } else {
            arr_ptr = unsafe { RuneArray::push(gc, arr_ptr, item) };
            remaining -= 1;
        }
    }
    if remaining == 0 {
        unsafe {
            Promise::set_state(result_ptr, PROMISE_FULFILLED);
            Promise::set_result(result_ptr, Value::from_heap_ptr(arr_ptr as *mut u8));
        }
    }
    result_val
}

/// Promise.race(iterable) — settles with the first settled promise or value.
pub fn promise_static_race(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let iterable = args.first().copied().unwrap_or(Value::undefined());
    let proto = vm.promise_prototype.heap_ptr();
    let result_ptr = Promise::allocate(gc, proto);
    let result_val = Value::from_heap_ptr(result_ptr);
    let len = if let Some(l) = crate::vm::array_like_length(iterable) {
        l
    } else {
        return result_val;
    };
    if len == 0 {
        return result_val;
    }
    for i in 0..len {
        let item = crate::vm::array_like_index(iterable, i).unwrap_or(Value::undefined());
        let is_promise = if let Some(ptr) = item.heap_ptr() {
            unsafe { (*(ptr as *const GcHeader)).tag() == TAG_PROMISE }
        } else {
            false
        };
        if is_promise {
            let ptr = item.heap_ptr().unwrap();
            let state = unsafe { Promise::state(ptr) };
            if state == PROMISE_FULFILLED {
                let r = unsafe { Promise::result(ptr) };
                unsafe {
                    Promise::set_state(result_ptr, PROMISE_FULFILLED);
                    Promise::set_result(result_ptr, r);
                }
                return result_val;
            }
            if state == PROMISE_REJECTED {
                let r = unsafe { Promise::result(ptr) };
                unsafe {
                    Promise::set_state(result_ptr, PROMISE_REJECTED);
                    Promise::set_result(result_ptr, r);
                }
                return result_val;
            }
        } else {
            unsafe {
                Promise::set_state(result_ptr, PROMISE_FULFILLED);
                Promise::set_result(result_ptr, item);
            }
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
            name: "Symbol",
            func: symbol_ctor_builtin,
        },
        Builtin {
            length: 1,
            name: "Symbol_for",
            func: symbol_for_builtin,
        },
        Builtin {
            length: 1,
            name: "Symbol_keyFor",
            func: symbol_key_for_builtin,
        },
        Builtin {
            length: 0,
            name: "Symbol_prototype_toString",
            func: symbol_prototype_to_string,
        },
        Builtin {
            length: 0,
            name: "Symbol_prototype_valueOf",
            func: symbol_prototype_value_of,
        },
        Builtin {
            length: 1,
            name: "Symbol_prototype_toPrimitive",
            func: symbol_prototype_to_primitive,
        },
        Builtin {
            length: 0,
            name: "Array_prototype_values",
            func: array_values_builtin,
        },
        Builtin {
            length: 0,
            name: "Array_prototype_keys",
            func: array_keys_builtin,
        },
        Builtin {
            length: 0,
            name: "Array_prototype_entries",
            func: array_entries_builtin,
        },
        Builtin {
            length: 0,
            name: "Array_prototype_iterator",
            func: array_values_builtin,
        },
        Builtin {
            length: 0,
            name: "Array_iterator_next",
            func: array_iterator_next,
        },
        Builtin {
            length: 0,
            name: "String_prototype_iterator",
            func: string_iterator_builtin,
        },
        Builtin {
            length: 0,
            name: "String_iterator_next",
            func: string_iterator_next,
        },
        Builtin {
            length: 0,
            name: "Iterator_prototype_symbol_iterator",
            func: iterator_prototype_symbol_iterator,
        },
        Builtin {
            length: 1,
            name: "ArrayBuffer",
            func: array_buffer_constructor,
        },
        Builtin {
            length: 1,
            name: "ArrayBuffer_isView",
            func: array_buffer_is_view_builtin,
        },
        Builtin {
            length: 2,
            name: "ArrayBuffer_prototype_slice",
            func: array_buffer_slice_builtin,
        },
        Builtin {
            length: 1,
            name: "Int8Array",
            func: int8array_constructor,
        },
        Builtin {
            length: 3,
            name: "Uint8Array",
            func: uint8array_constructor,
        },
        Builtin {
            length: 3,
            name: "Uint8ClampedArray",
            func: uint8clampedarray_constructor,
        },
        Builtin {
            length: 3,
            name: "Int16Array",
            func: int16array_constructor,
        },
        Builtin {
            length: 3,
            name: "Uint16Array",
            func: uint16array_constructor,
        },
        Builtin {
            length: 3,
            name: "Int32Array",
            func: int32array_constructor,
        },
        Builtin {
            length: 3,
            name: "Uint32Array",
            func: uint32array_constructor,
        },
        Builtin {
            length: 3,
            name: "Float32Array",
            func: float32array_constructor,
        },
        Builtin {
            length: 3,
            name: "Float64Array",
            func: float64array_constructor,
        },
        Builtin {
            length: 2,
            name: "TypedArray_prototype_set",
            func: typed_array_set_builtin,
        },
        Builtin {
            length: 2,
            name: "TypedArray_prototype_subarray",
            func: typed_array_subarray_builtin,
        },
        Builtin {
            length: 3,
            name: "TypedArray_prototype_fill",
            func: typed_array_fill_builtin,
        },
        Builtin {
            length: 1,
            name: "TypedArray_prototype_at",
            func: typed_array_at_builtin,
        },
        Builtin {
            length: 1,
            name: "TypedArray_prototype_indexOf",
            func: typed_array_index_of_builtin,
        },
        Builtin {
            length: 1,
            name: "TypedArray_prototype_includes",
            func: typed_array_includes_builtin,
        },
        Builtin {
            length: 2,
            name: "TypedArray_prototype_slice",
            func: typed_array_slice_builtin,
        },
        Builtin {
            length: 0,
            name: "TypedArray_prototype_values",
            func: typed_array_values_builtin,
        },
        Builtin {
            length: 0,
            name: "TypedArray_prototype_keys",
            func: typed_array_keys_builtin,
        },
        Builtin {
            length: 0,
            name: "TypedArray_prototype_entries",
            func: typed_array_entries_builtin,
        },
        Builtin {
            length: 1,
            name: "Map",
            func: map_constructor,
        },
        Builtin {
            length: 1,
            name: "Map_prototype_set",
            func: map_set_builtin,
        },
        Builtin {
            length: 1,
            name: "Map_prototype_get",
            func: map_get_builtin,
        },
        Builtin {
            length: 1,
            name: "Map_prototype_has",
            func: map_has_builtin,
        },
        Builtin {
            length: 1,
            name: "Map_prototype_delete",
            func: map_delete_builtin,
        },
        Builtin {
            length: 0,
            name: "Map_prototype_clear",
            func: map_clear_builtin,
        },
        Builtin {
            length: 1,
            name: "Map_prototype_forEach",
            func: map_foreach_builtin,
        },
        Builtin {
            length: 0,
            name: "Map_prototype_entries",
            func: map_entries_builtin,
        },
        Builtin {
            length: 0,
            name: "Map_prototype_keys",
            func: map_keys_builtin,
        },
        Builtin {
            length: 0,
            name: "Map_prototype_values",
            func: map_values_builtin,
        },
        Builtin {
            length: 0,
            name: "Map_iterator_next",
            func: map_iterator_next,
        },
        Builtin {
            length: 1,
            name: "Set",
            func: set_constructor,
        },
        Builtin {
            length: 1,
            name: "Set_prototype_add",
            func: set_add_builtin,
        },
        Builtin {
            length: 1,
            name: "Set_prototype_has",
            func: set_has_builtin,
        },
        Builtin {
            length: 1,
            name: "Set_prototype_delete",
            func: set_delete_builtin,
        },
        Builtin {
            length: 0,
            name: "Set_prototype_clear",
            func: set_clear_builtin,
        },
        Builtin {
            length: 1,
            name: "Set_prototype_forEach",
            func: set_foreach_builtin,
        },
        Builtin {
            length: 0,
            name: "Set_prototype_entries",
            func: set_entries_builtin,
        },
        Builtin {
            length: 0,
            name: "Set_prototype_keys",
            func: set_keys_builtin,
        },
        Builtin {
            length: 0,
            name: "Set_prototype_values",
            func: set_values_builtin,
        },
        Builtin {
            length: 0,
            name: "Set_iterator_next",
            func: set_iterator_next,
        },
        Builtin {
            length: 7,
            name: "Date",
            func: date_constructor,
        },
        Builtin {
            length: 0,
            name: "Date_now",
            func: date_now_builtin,
        },
        Builtin {
            length: 1,
            name: "Date_parse",
            func: date_parse_builtin,
        },
        Builtin {
            length: 7,
            name: "Date_UTC",
            func: date_utc_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getDate",
            func: date_get_date_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getDay",
            func: date_get_day_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getFullYear",
            func: date_get_full_year_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getHours",
            func: date_get_hours_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getMilliseconds",
            func: date_get_milliseconds_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getMinutes",
            func: date_get_minutes_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getMonth",
            func: date_get_month_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getSeconds",
            func: date_get_seconds_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getTime",
            func: date_get_time_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getTimezoneOffset",
            func: date_get_timezone_offset_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getUTCDate",
            func: date_get_utc_date_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getUTCDay",
            func: date_get_utc_day_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getUTCFullYear",
            func: date_get_utc_full_year_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getUTCHours",
            func: date_get_utc_hours_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getUTCMilliseconds",
            func: date_get_utc_milliseconds_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getUTCMinutes",
            func: date_get_utc_minutes_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getUTCMonth",
            func: date_get_utc_month_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_getUTCSeconds",
            func: date_get_utc_seconds_builtin,
        },
        Builtin {
            length: 1,
            name: "Date_prototype_setDate",
            func: date_set_date_builtin,
        },
        Builtin {
            length: 3,
            name: "Date_prototype_setFullYear",
            func: date_set_full_year_builtin,
        },
        Builtin {
            length: 4,
            name: "Date_prototype_setHours",
            func: date_set_hours_builtin,
        },
        Builtin {
            length: 1,
            name: "Date_prototype_setMilliseconds",
            func: date_set_milliseconds_builtin,
        },
        Builtin {
            length: 3,
            name: "Date_prototype_setMinutes",
            func: date_set_minutes_builtin,
        },
        Builtin {
            length: 2,
            name: "Date_prototype_setMonth",
            func: date_set_month_builtin,
        },
        Builtin {
            length: 2,
            name: "Date_prototype_setSeconds",
            func: date_set_seconds_builtin,
        },
        Builtin {
            length: 1,
            name: "Date_prototype_setTime",
            func: date_set_time_builtin,
        },
        Builtin {
            length: 1,
            name: "Date_prototype_setUTCDate",
            func: date_set_utc_date_builtin,
        },
        Builtin {
            length: 3,
            name: "Date_prototype_setUTCFullYear",
            func: date_set_utc_full_year_builtin,
        },
        Builtin {
            length: 4,
            name: "Date_prototype_setUTCHours",
            func: date_set_utc_hours_builtin,
        },
        Builtin {
            length: 1,
            name: "Date_prototype_setUTCMilliseconds",
            func: date_set_utc_milliseconds_builtin,
        },
        Builtin {
            length: 3,
            name: "Date_prototype_setUTCMinutes",
            func: date_set_utc_minutes_builtin,
        },
        Builtin {
            length: 2,
            name: "Date_prototype_setUTCMonth",
            func: date_set_utc_month_builtin,
        },
        Builtin {
            length: 2,
            name: "Date_prototype_setUTCSeconds",
            func: date_set_utc_seconds_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_toDateString",
            func: date_to_date_string_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_toISOString",
            func: date_to_iso_string_builtin,
        },
        Builtin {
            length: 1,
            name: "Date_prototype_toJSON",
            func: date_to_json_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_toLocaleDateString",
            func: date_to_locale_date_string_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_toLocaleString",
            func: date_to_locale_string_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_toLocaleTimeString",
            func: date_to_locale_time_string_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_toString",
            func: date_to_string_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_toTimeString",
            func: date_to_time_string_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_toUTCString",
            func: date_to_utc_string_builtin,
        },
        Builtin {
            length: 0,
            name: "Date_prototype_valueOf",
            func: date_value_of_builtin,
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
        Builtin {
            length: 1,
            name: "Promise_prototype_finally",
            func: promise_prototype_finally,
        },
        Builtin {
            length: 1,
            name: "Promise_resolve",
            func: promise_static_resolve,
        },
        Builtin {
            length: 1,
            name: "Promise_reject",
            func: promise_static_reject,
        },
        Builtin {
            length: 1,
            name: "Promise_all",
            func: promise_static_all,
        },
        Builtin {
            length: 1,
            name: "Promise_race",
            func: promise_static_race,
        },
        Builtin {
            length: 1,
            name: "async_continue",
            func: async_continue,
        },
        Builtin {
            length: 1,
            name: "async_reject",
            func: async_reject,
        },
        Builtin {
            length: 2,
            name: "RegExp",
            func: regexp_constructor,
        },
        Builtin {
            length: 1,
            name: "RegExp_prototype_exec",
            func: regexp_exec,
        },
        Builtin {
            length: 1,
            name: "RegExp_prototype_test",
            func: regexp_test,
        },
        Builtin {
            length: 0,
            name: "RegExp_prototype_source",
            func: regexp_source,
        },
        Builtin {
            length: 0,
            name: "RegExp_prototype_flags",
            func: regexp_flags,
        },
        Builtin {
            length: 0,
            name: "RegExp_prototype_lastIndex",
            func: regexp_last_index,
        },
        Builtin {
            length: 2,
            name: "String_prototype_replaceAll",
            func: string_replace_all,
        },
        Builtin {
            length: 1,
            name: "String_prototype_match",
            func: string_match,
        },
        Builtin {
            length: 1,
            name: "String_prototype_search",
            func: string_search,
        },
        Builtin {
            length: 1,
            name: "Error",
            func: error_builtin,
        },
        Builtin {
            length: 1,
            name: "EvalError",
            func: eval_error_builtin,
        },
        Builtin {
            length: 1,
            name: "RangeError",
            func: range_error_builtin,
        },
        Builtin {
            length: 1,
            name: "ReferenceError",
            func: reference_error_builtin,
        },
        Builtin {
            length: 1,
            name: "SyntaxError",
            func: syntax_error_builtin,
        },
        Builtin {
            length: 1,
            name: "TypeError",
            func: type_error_builtin,
        },
        Builtin {
            length: 1,
            name: "URIError",
            func: uri_error_builtin,
        },
        Builtin {
            length: 0,
            name: "Error_prototype_toString",
            func: error_prototype_to_string,
        },
        Builtin {
            length: 1,
            name: "isError",
            func: error_is_error,
        },
        // Object.prototype methods
        Builtin {
            length: 0,
            name: "Object_prototype_toString",
            func: object_prototype_to_string,
        },
        Builtin {
            length: 1,
            name: "Object_prototype_hasOwnProperty",
            func: object_prototype_has_own_property,
        },
        Builtin {
            length: 1,
            name: "Object_prototype_isPrototypeOf",
            func: object_prototype_is_prototype_of,
        },
        Builtin {
            length: 1,
            name: "Object_prototype_propertyIsEnumerable",
            func: object_prototype_property_is_enumerable,
        },
        Builtin {
            length: 0,
            name: "Object_prototype_valueOf",
            func: object_prototype_value_of,
        },
        Builtin {
            length: 1,
            name: "Object_getPrototypeOf",
            func: object_get_prototype_of,
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
            name: "Array_prototype_join",
            func: array_join,
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
    if a.is_undefined() && b.is_undefined() {
        return true;
    }
    if a.is_null() && b.is_null() {
        return true;
    }
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
            if av.is_nan() && bv.is_nan() {
                return true;
            }
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
            if f.is_sign_negative() {
                "-Infinity".to_string()
            } else {
                "Infinity".to_string()
            }
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
            format!("String {{ [[StringData]]: \"{}\" }}", unsafe {
                HeapString::to_string(str_ptr as *mut HeapString)
            })
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

/// Extract the error type name from an exception Value.
/// Order: own `name` property → "TypeError: " message prefix (internal
/// `make_error` objects) → "Error" for message-only objects.
pub fn read_error_name(val: Value) -> Option<String> {
    let ptr = val.heap_ptr()?;
    unsafe {
        let tag = (*(ptr as *const GcHeader)).tag();
        if tag == TAG_STRING {
            let s = HeapString::to_string(ptr as *mut HeapString);
            // Thrown values are encoded as "Name: message" strings; match
            // assert.throws expectations against the name prefix.
            if let Some(idx) = s.find(": ") {
                if idx < 64 && !s[..idx].is_empty() {
                    return Some(s[..idx].to_string());
                }
            }
            return Some(s);
        }
        if tag != TAG_OBJECT {
            return None;
        }
        let shape = JSObject::shape_ptr(ptr as *mut JSObject);
        let key = PropertyKey::from_string("name");
        if let Some(slot) = shape.lookup(&key) {
            let name_val = JSObject::get_slot(ptr as *mut JSObject, slot);
            if let Some(nptr) = name_val.heap_ptr() {
                let t2 = (*(nptr as *const GcHeader)).tag();
                if t2 == TAG_STRING {
                    return Some(HeapString::to_string(nptr as *mut HeapString));
                }
            }
        }
        if let Some(msg) = read_error_message(val) {
            if let Some(idx) = msg.find(": ") {
                if idx < 64 && !msg[..idx].is_empty() {
                    return Some(msg[..idx].to_string());
                }
            }
        }
        Some("Error".to_string())
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
pub fn assert_is_same_value(
    _gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
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

/// RegExp.prototype.exec(string) — run regex, return match array or null.
/// RegExp.prototype.exec(string) — §22.2.6.2 RegExpBuiltinExec.
/// Global/sticky regexps start at lastIndex and advance it; non-global
/// searches from 0. The result array carries non-enumerable "index" and
/// "input" properties.
pub fn regexp_exec(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let regexp_ptr = match get_regexp_this(this) {
        Some(p) => p,
        None => return Value::null(),
    };
    let input = args
        .first()
        .map(|v| string_from_value(*v))
        .unwrap_or_default();
    let s = input;
    let len = s.chars().count();

    let global = unsafe { RegExp::has_flag(regexp_ptr, 0) };
    let sticky = unsafe { RegExp::has_flag(regexp_ptr, 5) };

    let mut last = if global || sticky {
        unsafe { RegExp::last_index(regexp_ptr) as usize }
    } else {
        0
    };

    loop {
        // §22.2.7.2 step 9.a: lastIndex beyond the string → reset + null.
        if last > len {
            if global || sticky {
                unsafe { RegExp::set_last_index(regexp_ptr, 0) };
            }
            return Value::null();
        }
        match regexp_exec_internal(gc, regexp_ptr, &s, last) {
            Some(groups) => {
                let (start, end) = groups[0];
                // §22.2.7.2 step 10: sticky requires the match to start
                // exactly at lastIndex; any later match is a failure.
                if sticky && start != last {
                    unsafe { RegExp::set_last_index(regexp_ptr, 0) };
                    return Value::null();
                }
                // §22.2.7.2 step 12: global/sticky advance lastIndex to the
                // match end (zero-length matches keep the same lastIndex; the
                // caller advances to avoid infinite loops).
                if global || sticky {
                    unsafe { RegExp::set_last_index(regexp_ptr, end as u32) };
                }
                return make_match_result_array(gc, &groups, &s, start, vm.array_prototype);
            }
            None => {
                // Failure: sticky returns null immediately (lastIndex = 0);
                // otherwise advance one code unit and retry.
                if sticky {
                    unsafe { RegExp::set_last_index(regexp_ptr, 0) };
                    return Value::null();
                }
                last += 1;
            }
        }
    }
}

/// RegExp.prototype.test(string) — return true if pattern matches.
/// Global/sticky regexps advance lastIndex exactly like exec.
pub fn regexp_test(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let result = regexp_exec(gc, this, args, vm);
    if result.is_null() {
        Value::boolean(false)
    } else {
        Value::boolean(true)
    }
}

/// The RegExp constructor — §22.2.4.1.
/// Called with `new` (this = freshly allocated TAG_REGEXP) or as a plain
/// function (this = undefined). Plain-call with a RegExp pattern and no flags
/// returns the pattern itself; every other form creates a new RegExp.
pub fn regexp_constructor(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let pattern_arg = args.first().copied().unwrap_or(Value::undefined());
    let flags_arg = args.get(1).copied().unwrap_or(Value::undefined());

    let is_new = this.is_heap_object()
        && this
            .heap_ptr()
            .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_REGEXP });

    // §22.2.4.1 step 3: plain call with a RegExp pattern and no flags returns
    // the pattern itself (same-constructor shortcut).
    if !is_new {
        if let Some(ptr) = pattern_arg.heap_ptr() {
            let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
            if tag == TAG_REGEXP && flags_arg.is_undefined() {
                return pattern_arg;
            }
        }
    }

    // Extract pattern source and flags per §22.2.4.1 steps 4-7.
    let mut flags_str = String::new();
    let pattern_str = if let Some(ptr) = pattern_arg.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_REGEXP {
            let pattern_ptr = unsafe { RegExp::pattern(ptr) };
            let src = unsafe { HeapString::to_string(pattern_ptr as *mut HeapString) };
            if flags_arg.is_undefined() {
                let f = unsafe { RegExp::flags(ptr) };
                let mut fs = String::new();
                if f & 1 != 0 {
                    fs.push('g');
                }
                if f & 2 != 0 {
                    fs.push('i');
                }
                if f & 4 != 0 {
                    fs.push('m');
                }
                if f & 8 != 0 {
                    fs.push('s');
                }
                if f & 16 != 0 {
                    fs.push('u');
                }
                if f & 32 != 0 {
                    fs.push('y');
                }
                if f & 64 != 0 {
                    fs.push('d');
                }
                if f & 128 != 0 {
                    fs.push('v');
                }
                flags_str = fs;
            } else {
                flags_str = arg_to_string(gc, Some(flags_arg), vm);
            }
            src
        } else {
            if !flags_arg.is_undefined() {
                flags_str = arg_to_string(gc, Some(flags_arg), vm);
            }
            value_to_pattern_string(Some(pattern_arg), gc, vm)
        }
    } else {
        if !flags_arg.is_undefined() {
            flags_str = arg_to_string(gc, Some(flags_arg), vm);
        }
        value_to_pattern_string(Some(pattern_arg), gc, vm)
    };

    // §22.2.3.3: flags must only contain d/g/i/m/s/u/v/y, no duplicates.
    let mut seen: u32 = 0;
    for c in flags_str.chars() {
        let bit = match c {
            'g' => 1,
            'i' => 2,
            'm' => 4,
            's' => 8,
            'u' => 16,
            'y' => 32,
            'd' => 64,
            'v' => 128,
            _ => {
                vm.set_pending_exception(make_error(
                    gc,
                    "SyntaxError: Invalid regular expression flags",
                ));
                return Value::undefined();
            }
        };
        if seen & bit != 0 {
            vm.set_pending_exception(make_error(
                gc,
                "SyntaxError: Duplicate regular expression flag",
            ));
            return Value::undefined();
        }
        seen |= bit;
    }

    // §22.2.3.3: pattern must parse, else SyntaxError.
    if rune_regex::parse_regex(&pattern_str).is_err() {
        vm.set_pending_exception(make_error(gc, "SyntaxError: Invalid regular expression"));
        return Value::undefined();
    }

    if is_new {
        let new_ptr = this.heap_ptr().unwrap();
        let pattern_heap = HeapString::allocate(gc, &pattern_str);
        unsafe {
            RegExp::set_pattern(new_ptr, pattern_heap as *mut u8);
            RegExp::set_flags(new_ptr, seen);
            RegExp::set_last_index(new_ptr, 0);
        }
        return this;
    }

    let rx = alloc_regexp_from_string(gc, &pattern_str, seen, vm.regexp_prototype);
    unsafe {
        if let Some(p) = rx.heap_ptr() {
            RegExp::set_last_index(p, 0);
        }
    }
    rx
}

fn get_regexp_this(this: Value) -> Option<*mut u8> {
    if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_REGEXP {
            return Some(ptr);
        }
    }
    None
}

/// RegExp.prototype.source getter — returns the pattern string.
pub fn regexp_source(gc: &mut SemiSpace, this: Value, _args: &[Value], _vm: &mut Vm) -> Value {
    let regexp_ptr = match get_regexp_this(this) {
        Some(p) => p,
        None => return Value::undefined(),
    };
    let pattern = unsafe { HeapString::to_string(RegExp::pattern(regexp_ptr) as *mut HeapString) };
    Value::from_heap_ptr(HeapString::allocate(gc, &pattern) as *mut u8)
}

/// RegExp.prototype.flags getter — returns a string like "gimsuyd".
pub fn regexp_flags(gc: &mut SemiSpace, this: Value, _args: &[Value], _vm: &mut Vm) -> Value {
    let regexp_ptr = match get_regexp_this(this) {
        Some(p) => p,
        None => return Value::undefined(),
    };
    let flags = unsafe { RegExp::flags(regexp_ptr) };
    let mut s = String::new();
    if flags & 1 != 0 {
        s.push('g');
    }
    if flags & 2 != 0 {
        s.push('i');
    }
    if flags & 4 != 0 {
        s.push('m');
    }
    if flags & 8 != 0 {
        s.push('s');
    }
    if flags & 16 != 0 {
        s.push('u');
    }
    if flags & 32 != 0 {
        s.push('y');
    }
    if flags & 64 != 0 {
        s.push('d');
    }
    if flags & 128 != 0 {
        s.push('v');
    }
    Value::from_heap_ptr(HeapString::allocate(gc, &s) as *mut u8)
}

/// RegExp.prototype.lastIndex getter — returns the lastIndex value.
pub fn regexp_last_index(_gc: &mut SemiSpace, this: Value, _args: &[Value], _vm: &mut Vm) -> Value {
    let regexp_ptr = match get_regexp_this(this) {
        Some(p) => p,
        None => return Value::undefined(),
    };
    let li = unsafe { RegExp::last_index(regexp_ptr) };
    Value::smi(li as i32)
}
