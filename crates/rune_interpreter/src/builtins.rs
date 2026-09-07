use crate::vm::SymbolMethodResult;
use crate::vm::Vm;
use crate::vm::get_iter_method;
use crate::vm::get_symbol_method;
use crate::vm::load_property_recursive;
use crate::vm::to_number;
use crate::vm::value_to_array_index;
use crate::vm::value_to_prop_key;
use crate::vm::{CollectionCtorState, PendingCollectionCtor, PendingCollectionForEach};
use crate::vm::{Exit, GeneratorResume, call_builtin_sync};
use rune_core::array::RuneArray;
use rune_core::date;
use rune_core::gc::{
    GcHeader, SemiSpace, TAG_ACCESSOR, TAG_ARRAY, TAG_ARRAY_BUFFER, TAG_DATE, TAG_FLOAT64,
    TAG_FORWARDED, TAG_FUNC, TAG_MAP, TAG_OBJECT, TAG_PROMISE, TAG_REGEXP, TAG_SET, TAG_STRING,
    TAG_STRING_OBJ, TAG_TYPED_ARRAY,
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
    // B1e: holes stringify as undefined (defense in depth — all known
    // paths map holes first, but a stray sentinel must never surface).
    if v == Value::empty_sentinel() {
        return "undefined".to_string();
    }
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
        // §7.1.12.1 Number::toString: non-finite values spell out; -0 prints as "0"
        if f.is_nan() {
            "NaN".to_string()
        } else if f == 0.0 {
            "0".to_string()
        } else if f.is_infinite() {
            if f < 0.0 {
                "-Infinity".to_string()
            } else {
                "Infinity".to_string()
            }
        } else {
            f.to_string()
        }
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
/// Raw ToPrimitive method lookup (TAG_OBJECT chain walk, depth-capped).
/// Extracted from to_primitive_string for the pending_call resume path —
/// behavior identical.
pub(crate) fn toprim_find_method(val: Value, key: &PropertyKey) -> Option<Value> {
    let mut current = val;
    for _ in 0..64 {
        let cptr = current.heap_ptr()?;
        if unsafe { (*(cptr as *const GcHeader)).tag() } != TAG_OBJECT {
            return None;
        }
        let shape = unsafe { JSObject::shape_ptr(cptr as *mut JSObject) };
        if let Some(slot) = shape.lookup(key) {
            return Some(unsafe { JSObject::get_slot(cptr as *mut JSObject, slot) });
        }
        let proto = unsafe { JSObject::prototype(cptr as *mut JSObject) };
        if proto.is_null() {
            return None;
        }
        current = Value::from_heap_ptr(proto);
    }
    None
}

/// One valueOf round-trip for a pending ToPrimitive resume (B1e).
pub(crate) enum ToprimResumeOut {
    Ready(Value),
    WaitPushed,
    Raise(Value),
}

/// Run the valueOf stage of a pending ToPrimitive resume: builtin valueOf
/// runs inline, JS valueOf re-arms pending_call (caller: continue without
/// advancing), anything else raises TypeError.
pub(crate) fn toprim_resume_valueof(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    value: Value,
) -> ToprimResumeOut {
    let key = PropertyKey::from_string("valueOf");
    let Some(method) = toprim_find_method(value, &key) else {
        return ToprimResumeOut::Raise(sort_type_error(
            gc,
            vm,
            "Cannot convert object to primitive value",
        ));
    };
    if let Some(smi) = method.as_smi() {
        if smi < 0 {
            let id = ((-smi) as usize) - 1;
            if id < vm.builtins.len() {
                let result = (vm.builtins[id].func)(gc, value, &[], vm);
                if let Some(exc) = vm.pending_exception.take() {
                    return ToprimResumeOut::Raise(exc);
                }
                if toprim_is_primitive(result) {
                    return ToprimResumeOut::Ready(result);
                }
            }
        }
        return ToprimResumeOut::Raise(sort_type_error(
            gc,
            vm,
            "Cannot convert object to primitive value",
        ));
    }
    if method
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_FUNC)
    {
        vm.pending_call = Some(crate::vm::PendingCall {
            source_frame_depth: vm.frame_depth(),
            cont: crate::vm::PendingCallCont::ToPrimString {
                value,
                tried_valueof: true,
            },
        });
        vm.push_callback_call(gc, method, value, vec![]);
        return ToprimResumeOut::WaitPushed;
    }
    ToprimResumeOut::Raise(sort_type_error(
        gc,
        vm,
        "Cannot convert object to primitive value",
    ))
}

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
        // §7.1.1 ToPrimitive with string hint: call toString(), then valueOf().
        // A2: GetMethod walks the prototype chain — previously only OWN
        // properties dispatched, so inherited methods (notably
        // Error.prototype.toString on error objects) fell through to
        // "[object Object]". Depth-capped; cycles terminate.
        let find_method = |key: &PropertyKey| toprim_find_method(val, key);
        let key = PropertyKey::from_string("toString");
        if let Some(to_string_val) = find_method(&key) {
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
                        cont: crate::vm::PendingCallCont::ToPrimString {
                            value: val,
                            tried_valueof: false,
                        },
                    });
                    vm.push_callback_call(gc, to_string_val, val, vec![]);
                    return None; // caller must return immediately
                }
            }
        }
        // Fall through to valueOf if no toString or toString didn't return a primitive
        let value_of_key = PropertyKey::from_string("valueOf");
        if let Some(value_of_val) = find_method(&value_of_key) {
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
        // A2: chain-walk like the async version so inherited BUILTIN methods
        // (Error.prototype.toString) dispatch; user-defined (JS-func) methods
        // are still skipped — no pending protocol in sync contexts.
        let find_method = |key: &PropertyKey| -> Option<Value> {
            let mut current = val;
            for _ in 0..64 {
                let cptr = current.heap_ptr()?;
                if unsafe { (*(cptr as *const GcHeader)).tag() } != TAG_OBJECT {
                    return None;
                }
                let shape = unsafe { JSObject::shape_ptr(cptr as *mut JSObject) };
                if let Some(slot) = shape.lookup(key) {
                    return Some(unsafe { JSObject::get_slot(cptr as *mut JSObject, slot) });
                }
                let proto = unsafe { JSObject::prototype(cptr as *mut JSObject) };
                if proto.is_null() {
                    return None;
                }
                current = Value::from_heap_ptr(proto);
            }
            None
        };
        let key = PropertyKey::from_string("toString");
        if let Some(to_string_val) = find_method(&key) {
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
        if let Some(value_of_val) = find_method(&value_of_key) {
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
            // B1e: holes (and unallocated tail slots) join as "".
            if elem == Value::empty_sentinel() {
                parts.push(String::new());
            } else {
                parts.push(value_to_js_string(elem));
            }
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
    if val.is_symbol() {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert a Symbol value to a number",
        ));
        return Value::undefined();
    }
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert a Symbol value to a string",
        ));
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
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "Cannot convert a Symbol value to a string",
            ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert a Symbol value to a string",
        ));
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
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "Symbol.keyFor requires that the argument be a symbol",
            ));
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
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "Symbol.prototype.toString requires that 'this' be a Symbol",
            ));
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
        vm.set_pending_exception(crate::errors::error_object(
            _gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Symbol.prototype.valueOf requires that 'this' be a Symbol",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Symbol.prototype[Symbol.toPrimitive] requires that 'this' be a Symbol",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Array.prototype.values requires an array receiver",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Array.prototype.keys requires an array receiver",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Array.prototype.entries requires an array receiver",
        ));
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

// ─── Generator instances (§27.5) ────────────────────────────────────────
// A generator call produces a plain JSObject carrying the hidden
// "__rune_gen" slot (internal Generator id) plus next/return/throw and
// @@iterator directly on the instance (prototype methods = follow-up).

fn iter_result_object(gc: &mut SemiSpace, value: Value, done: bool) -> Value {
    let keys = vec![
        (PropertyKey::from_string("value"), 0),
        (PropertyKey::from_string("done"), 1),
    ];
    let names = vec!["value".to_string(), "done".to_string()];
    let shape = Shape::intern(keys, names);
    let obj = JSObject::allocate(gc, shape, &[value, Value::boolean(done)]);
    Value::from_heap_ptr(obj as *mut u8)
}

// iter_result_object needs &mut Vm for the prototype; wrapper keeps the
// plain helper above for arity reasons.
pub(crate) fn iter_result_with_proto(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    value: Value,
    done: bool,
) -> Value {
    let r = iter_result_object(gc, value, done);
    if let Some(pp) = r.heap_ptr() {
        if let Some(proto) = vm.object_prototype.heap_ptr() {
            unsafe {
                JSObject::set_prototype(pp as *mut JSObject, proto);
            }
        }
    }
    r
}

pub(crate) fn generator_id_of(vm: &Vm, this: Value) -> Option<usize> {
    let ptr = this.heap_ptr()?;
    if unsafe { (*(ptr as *const GcHeader)).tag() } != TAG_OBJECT {
        return None;
    }
    let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
    let slot = shape.lookup(&PropertyKey::from_symbol(vm.gen_state_symbol))?;
    let v = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
    v.as_smi().map(|s| s as usize)
}

pub fn generator_next_builtin(
    _gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let arg = args.first().copied().unwrap_or(Value::undefined());
    let Some(gen_id) = generator_id_of(vm, this) else {
        vm.set_pending_exception(crate::errors::error_object(
            _gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "next called on non-generator",
        ));
        return Value::undefined();
    };
    if vm.generators[gen_id].done {
        return iter_result_with_proto(_gc, vm, Value::undefined(), true);
    }
    if vm.generators[gen_id].executing {
        // §25.3.3.3: resuming an executing generator throws AND completes it.
        vm.generators[gen_id].done = true;
        vm.set_pending_exception(crate::errors::error_object(
            _gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "generator already running",
        ));
        return Value::undefined();
    }
    match vm.resume_generator_full(_gc, gen_id, crate::vm::GeneratorResume::Next(arg)) {
        Ok((v, done)) => iter_result_with_proto(_gc, vm, v, done),
        Err(e) => {
            vm.set_pending_exception(e);
            Value::undefined()
        }
    }
}

/// Outcomes shared by yield* step functions.
/// - `Sync(v)`: completed synchronously with value `v`.
/// - `Wait`: an async getter/callback was pushed; the caller must bail
///   without advancing (opcode arms `continue`; builtins return undefined
///   relying on the skip-list; Return arms `continue`).
/// - `Raise(e)`: unwind error value `e` now.
pub enum YsOut {
    Sync(Value),
    Wait,
    Raise(Value),
    /// A builtin getter unwound via the callback machinery: propagate the
    /// exit (or continue if it already redirected). Vanishingly rare.
    Bail(Option<Exit>),
}

pub enum AfrOut {
    Sync(Value),
    Wait,
    Raise(Value),
}

/// Async-capable property read for yield* result processing.
/// `gen_id`/`abrupt_arg`/`iter` populate the pending state on Wait; plain
/// iteration paths (no abrupt in flight) pass dummies — Star* resume arms
/// never touch them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ys_read_gen(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    gen_id: usize,
    abrupt_arg: Value,
    iter: Value,
    stash: Value,
    phase: crate::vm::YsGetPhase,
    target: Value,
    key: Value,
) -> YsOut {
    use crate::vm::{PendingYieldStarGet, YsGetOut};
    let pend = PendingYieldStarGet {
        source_frame_depth: vm.frame_depth() - 1,
        gen_id,
        phase,
        abrupt_arg,
        iter,
        stash,
    };
    match vm.yieldstar_get(gc, target, key, pend) {
        YsGetOut::Ready(v) => YsOut::Sync(v),
        YsGetOut::Wait => YsOut::Wait,
        YsGetOut::Bail(exit) => YsOut::Bail(exit),
    }
}

/// Classify a loaded method value: undefined/null → absent.
fn classify_method(m: Value) -> Result<Option<Value>, ()> {
    if m.is_undefined() || m.is_null() {
        Ok(None)
    } else if m.as_smi().is_some_and(|s| s < 0)
        || m.heap_ptr()
            .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_FUNC })
    {
        Ok(Some(m))
    } else {
        Err(())
    }
}

fn violation_type_error(vm: &mut Vm, gc: &mut SemiSpace, is_throw: bool) -> Value {
    // Throw without a throw method is a yield* protocol violation; return
    // without one completes directly (unreachable here, kept for symmetry).
    let _ = is_throw;
    crate::errors::error_object(
        gc,
        &vm.error_protos,
        crate::errors::ErrorKind::TypeError,
        "iterator does not have a throw method",
    )
}

/// Continue after a delegate throw/return method value is available
/// (sync or resumed). Handles invocation, violation paths, and result
/// processing.
#[allow(clippy::too_many_arguments)]
pub fn afr_method_loaded(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    gen_id: usize,
    is_throw: bool,
    abrupt_arg: Value,
    iter: Value,
    method: Value,
) -> AfrOut {
    let loaded = match classify_method(method) {
        Ok(m) => m,
        Err(()) => {
            return AfrOut::Raise(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                &format!(
                    "iterator.{} is not a function",
                    if is_throw { "throw" } else { "return" }
                ),
            ));
        }
    };
    match loaded {
        None => {
            if is_throw {
                // Violation path needs the return method (IteratorClose).
                let ret_key = method_return_key(gc);
                match ys_read_gen(
                    vm,
                    gc,
                    gen_id,
                    abrupt_arg,
                    iter,
                    iter,
                    crate::vm::YsGetPhase::CloseReturn,
                    iter,
                    ret_key,
                ) {
                    YsOut::Sync(ret) => match classify_method(ret) {
                        Ok(Some(m)) => forward_call_delegate_method(
                            vm, gc, gen_id, true, abrupt_arg, iter, m, abrupt_arg, true,
                        ),
                        _ => AfrOut::Raise(violation_type_error(vm, gc, true)),
                    },
                    YsOut::Wait => AfrOut::Wait,
                    YsOut::Raise(e) => AfrOut::Raise(e),
                    YsOut::Bail(_) => AfrOut::Raise(crate::errors::error_object(
                        gc,
                        &vm.error_protos,
                        crate::errors::ErrorKind::TypeError,
                        "getter threw",
                    )),
                }
            } else {
                vm.generators[gen_id].done = true;
                vm.generators[gen_id].in_delegate = false;
                AfrOut::Sync(iter_result_with_proto(gc, vm, abrupt_arg, true))
            }
        }
        Some(m) => forward_call_delegate_method(
            vm, gc, gen_id, is_throw, abrupt_arg, iter, m, abrupt_arg, false,
        ),
    }
}

fn method_return_key(gc: &mut SemiSpace) -> Value {
    Value::from_heap_ptr(HeapString::allocate(gc, "return") as *mut u8)
}

/// GetMethod(delegate, name) for yield* forwarding, async-capable: returns
/// the method (or absence marker) synchronously, or Wait after pushing a
/// getter frame (resume via PendingYieldStarGet with an AfrMethod phase).
/// Non-callable methods surface as a TypeError Raise.
fn delegate_method(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    gen_id: usize,
    is_throw: bool,
    abrupt_arg: Value,
    iter: Value,
    name: &str,
) -> YsOut {
    let key = Value::from_heap_ptr(HeapString::allocate(gc, name) as *mut u8);
    let pend = crate::vm::PendingYieldStarGet {
        source_frame_depth: vm.frame_depth() - 1,
        gen_id,
        phase: crate::vm::YsGetPhase::AfrMethod { is_throw },
        abrupt_arg,
        iter,
        stash: iter,
    };
    match vm.yieldstar_get(gc, iter, key, pend) {
        crate::vm::YsGetOut::Ready(m) => YsOut::Sync(m),
        crate::vm::YsGetOut::Wait => YsOut::Wait,
        crate::vm::YsGetOut::Bail(_) => YsOut::Raise(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "getter threw",
        )),
    }
}

/// Finish a forwarded delegate throw()/return() call (shared by the sync
/// path and the async pending-state continuation).
///
/// - done + throw → the original exception continues (done=true).
/// - done + return → complete with the DELEGATE's return value.
/// - value → suspend the outer again yielding it, carrying the original
///   abrupt (it resumes on the next next()/throw()/return()).
fn finish_delegate_afr(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    gen_id: usize,
    is_throw: bool,
    abrupt_arg: Value,
    method_result: Value,
) -> AfrOut {
    if !method_result.is_heap_object() {
        return AfrOut::Raise(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Iterator result is not an object",
        ));
    }
    // done read (async-capable).
    let done = match ys_read_gen(
        vm,
        gc,
        gen_id,
        abrupt_arg,
        method_result,
        method_result,
        crate::vm::YsGetPhase::FinishDone,
        method_result,
        vm.done_key,
    ) {
        YsOut::Sync(v) => v.to_bool(),
        YsOut::Wait => return AfrOut::Wait,
        YsOut::Raise(e) => return AfrOut::Raise(e),
        YsOut::Bail(_) => {
            // See YsGetOut::Bail: already unwound; surface generically.
            return AfrOut::Raise(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "getter threw",
            ));
        }
    };
    // value read (async-capable).
    let value = match ys_read_gen(
        vm,
        gc,
        gen_id,
        abrupt_arg,
        method_result,
        method_result,
        crate::vm::YsGetPhase::FinishValue { is_throw },
        method_result,
        vm.value_key,
    ) {
        YsOut::Sync(v) => v,
        YsOut::Wait => return AfrOut::Wait,
        YsOut::Raise(e) => return AfrOut::Raise(e),
        YsOut::Bail(_) => {
            // See YsGetOut::Bail: already unwound; surface generically.
            return AfrOut::Raise(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "getter threw",
            ));
        }
    };
    if done {
        vm.generators[gen_id].done = true;
        vm.generators[gen_id].in_delegate = false;
        AfrOut::Sync(iter_result_with_proto(gc, vm, value, true))
    } else {
        vm.generators[gen_id].abrupt = Some(if is_throw {
            GeneratorResume::Throw(abrupt_arg)
        } else {
            GeneratorResume::Return(abrupt_arg)
        });
        AfrOut::Sync(iter_result_with_proto(gc, vm, value, false))
    }
}

/// Async completion of a forwarded delegate throw()/return() call (runs
/// in the Return handler when the delegate JS callback returns). Mirrors
/// finish_delegate_afr but speaks the handler convention: unwinding goes
/// through handle_throw directly so no dummy stack slots leak.
#[allow(clippy::too_many_arguments)]
pub fn finish_yieldstar_afr_result(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    gen_id: usize,
    is_throw: bool,
    abrupt_arg: Value,
    _iter: Value,
    closing: bool,
    method_result: Value,
) -> Result<Value, Option<Exit>> {
    if closing {
        // IteratorClose finished for a throw-violation: the protocol
        // violation TypeError continues (not the original exception).
        let e = crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "iterator does not have a throw method",
        );
        return match vm.handle_throw(gc, e) {
            Some(exit) => Err(Some(exit)),
            None => Err(None),
        };
    }
    if !method_result.is_heap_object() {
        let e = crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Iterator result is not an object",
        );
        return match vm.handle_throw(gc, e) {
            Some(exit) => Err(Some(exit)),
            None => Err(None),
        };
    }
    let done_key = vm.done_key;
    let done = match ys_read_gen(
        vm,
        gc,
        gen_id,
        abrupt_arg,
        method_result,
        method_result,
        crate::vm::YsGetPhase::FinishDone,
        method_result,
        done_key,
    ) {
        YsOut::Sync(v) => v.to_bool(),
        YsOut::Wait => return Err(None),
        YsOut::Raise(e) => match vm.handle_throw(gc, e) {
            Some(exit) => return Err(Some(exit)),
            None => return Err(None),
        },
        YsOut::Bail(Some(exit)) => return Err(Some(exit)),
        YsOut::Bail(None) => return Err(None),
    };
    let value_key = vm.value_key;
    let value = match ys_read_gen(
        vm,
        gc,
        gen_id,
        abrupt_arg,
        method_result,
        method_result,
        crate::vm::YsGetPhase::FinishValue { is_throw },
        method_result,
        value_key,
    ) {
        YsOut::Sync(v) => v,
        YsOut::Wait => return Err(None),
        YsOut::Raise(e) => match vm.handle_throw(gc, e) {
            Some(exit) => return Err(Some(exit)),
            None => return Err(None),
        },
        YsOut::Bail(Some(exit)) => return Err(Some(exit)),
        YsOut::Bail(None) => return Err(None),
    };
    if done {
        vm.generators[gen_id].done = true;
        vm.generators[gen_id].in_delegate = false;
        Ok(iter_result_with_proto(gc, vm, value, true))
    } else {
        vm.generators[gen_id].abrupt = Some(if is_throw {
            GeneratorResume::Throw(abrupt_arg)
        } else {
            GeneratorResume::Return(abrupt_arg)
        });
        Ok(iter_result_with_proto(gc, vm, value, false))
    }
}

/// Continue a resumed async yield* property read (Return-handler context).
/// `loaded` is the getter's return value. Advances control exactly like the
/// synchronous path would have.
pub(crate) fn continue_yieldstar_get(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    pg: &crate::vm::PendingYieldStarGet,
    loaded: Value,
) -> Result<(), Exit> {
    use crate::vm::YsGetPhase;
    match pg.phase {
        YsGetPhase::AfrMethod { is_throw } => {
            match afr_method_loaded(vm, gc, pg.gen_id, is_throw, pg.abrupt_arg, pg.iter, loaded) {
                AfrOut::Sync(v) => {
                    vm.push_and_advance(v);
                    Ok(())
                }
                AfrOut::Wait => Ok(()),
                AfrOut::Raise(e) => match vm.handle_throw(gc, e) {
                    Some(exit) => Err(exit),
                    None => Ok(()),
                },
            }
        }
        YsGetPhase::CloseReturn => {
            // Violation-path return method loaded: invoke as closing call.
            match forward_call_delegate_method(
                vm,
                gc,
                pg.gen_id,
                true,
                pg.abrupt_arg,
                pg.iter,
                loaded,
                pg.abrupt_arg,
                true,
            ) {
                AfrOut::Sync(v) => {
                    vm.push_and_advance(v);
                    Ok(())
                }
                AfrOut::Wait => Ok(()),
                AfrOut::Raise(e) => match vm.handle_throw(gc, e) {
                    Some(exit) => Err(exit),
                    None => Ok(()),
                },
            }
        }
        YsGetPhase::StarDone { end_target } => {
            if loaded.to_bool() {
                PendingYieldStarGetPhase::chain_value_done(vm, gc, pg, end_target)
            } else {
                PendingYieldStarGetPhase::chain_value(vm, gc, pg)
            }
        }
        YsGetPhase::StarValue => {
            vm.push_and_advance(loaded);
            Ok(())
        }
        YsGetPhase::StarValueDone { end_target } => {
            vm.pop2_push_jump(loaded, end_target);
            Ok(())
        }
        YsGetPhase::FinishDone => {
            vm.generators[pg.gen_id].done = true;
            vm.generators[pg.gen_id].in_delegate = false;
            let r = iter_result_with_proto(gc, vm, loaded, true);
            vm.push_and_advance(r);
            Ok(())
        }
        YsGetPhase::FinishValue { is_throw } => {
            vm.generators[pg.gen_id].abrupt = Some(if is_throw {
                GeneratorResume::Throw(pg.abrupt_arg)
            } else {
                GeneratorResume::Return(pg.abrupt_arg)
            });
            let r = iter_result_with_proto(gc, vm, loaded, false);
            vm.push_and_advance(r);
            Ok(())
        }
    }
}

/// Helper namespace to keep the StarDone chaining readable.
struct PendingYieldStarGetPhase;
impl PendingYieldStarGetPhase {
    fn chain_value(
        vm: &mut Vm,
        gc: &mut SemiSpace,
        pg: &crate::vm::PendingYieldStarGet,
    ) -> Result<(), Exit> {
        match ys_read_gen(
            vm,
            gc,
            pg.gen_id,
            pg.abrupt_arg,
            pg.iter,
            pg.stash,
            crate::vm::YsGetPhase::StarValue,
            pg.stash,
            vm.value_key,
        ) {
            YsOut::Sync(v) => {
                vm.push_and_advance(v);
                Ok(())
            }
            YsOut::Wait => Ok(()),
            YsOut::Raise(e) => match vm.handle_throw(gc, e) {
                Some(exit) => Err(exit),
                None => Ok(()),
            },
            YsOut::Bail(Some(exit)) => Err(exit),
            YsOut::Bail(None) => Ok(()),
        }
    }
    fn chain_value_done(
        vm: &mut Vm,
        gc: &mut SemiSpace,
        pg: &crate::vm::PendingYieldStarGet,
        end_target: usize,
    ) -> Result<(), Exit> {
        match ys_read_gen(
            vm,
            gc,
            pg.gen_id,
            pg.abrupt_arg,
            pg.iter,
            pg.stash,
            crate::vm::YsGetPhase::StarValueDone { end_target },
            pg.stash,
            vm.value_key,
        ) {
            YsOut::Sync(v) => {
                vm.pop2_push_jump(v, end_target);
                Ok(())
            }
            YsOut::Wait => Ok(()),
            YsOut::Raise(e) => match vm.handle_throw(gc, e) {
                Some(exit) => Err(exit),
                None => Ok(()),
            },
            YsOut::Bail(Some(exit)) => Err(exit),
            YsOut::Bail(None) => Ok(()),
        }
    }
}

/// Route an outer throw()/return() received while suspended inside `yield*`
/// to the delegate iterator's throw/return method (or IteratorClose when
/// absent). Returns the builtin's value, or `None` when an async delegate
/// call was pushed (the pending state owns the continuation).
fn forward_to_delegate(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    gen_id: usize,
    is_throw: bool,
    abrupt_arg: Value,
) -> AfrOut {
    let g = &vm.generators[gen_id];
    let (iter, _next) = match (g.stack.len().checked_sub(2), g.stack.len().checked_sub(1)) {
        (Some(i0), Some(i1)) => (g.stack[i0], g.stack[i1]),
        _ => {
            // No live pair (shouldn't happen): fall back to plain close.
            vm.generators[gen_id].done = true;
            if is_throw {
                return AfrOut::Raise(abrupt_arg);
            }
            return AfrOut::Sync(iter_result_with_proto(gc, vm, abrupt_arg, true));
        }
    };
    let method_name = if is_throw { "throw" } else { "return" };
    let loaded = match delegate_method(vm, gc, gen_id, is_throw, abrupt_arg, iter, method_name) {
        YsOut::Sync(m) => m,
        YsOut::Wait => return AfrOut::Wait,
        YsOut::Raise(e) => return AfrOut::Raise(e),
        YsOut::Bail(_) => {
            // See YsGetOut::Bail: already unwound; surface generically.
            return AfrOut::Raise(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "getter threw",
            ));
        }
    };
    match classify_method(loaded) {
        Err(()) => AfrOut::Raise(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            &format!("iterator.{method_name} is not a function"),
        )),
        Ok(None) => {
            // No throw/return method.
            if is_throw {
                // Throw without a throw method is a yield* protocol
                // violation: IteratorClose first (call return() if present),
                // then throw a TypeError — NOT the original exception.
                let ret_loaded =
                    match delegate_method(vm, gc, gen_id, true, abrupt_arg, iter, "return") {
                        YsOut::Sync(m) => m,
                        YsOut::Wait => return AfrOut::Wait,
                        YsOut::Raise(e) => return AfrOut::Raise(e),
                        YsOut::Bail(_) => {
                            // See YsGetOut::Bail: already unwound; surface generically.
                            return AfrOut::Raise(crate::errors::error_object(
                                gc,
                                &vm.error_protos,
                                crate::errors::ErrorKind::TypeError,
                                "getter threw",
                            ));
                        }
                    };
                match classify_method(ret_loaded) {
                    Ok(Some(ret)) => forward_call_delegate_method(
                        vm, gc, gen_id, true, abrupt_arg, iter, ret, abrupt_arg, true,
                    ),
                    _ => AfrOut::Raise(crate::errors::error_object(
                        gc,
                        &vm.error_protos,
                        crate::errors::ErrorKind::TypeError,
                        "iterator does not have a throw method",
                    )),
                }
            } else {
                vm.generators[gen_id].done = true;
                vm.generators[gen_id].in_delegate = false;
                AfrOut::Sync(iter_result_with_proto(gc, vm, abrupt_arg, true))
            }
        }
        Ok(Some(m)) => forward_call_delegate_method(
            vm, gc, gen_id, is_throw, abrupt_arg, iter, m, abrupt_arg, false,
        ),
    }
}

/// Invoke a delegate throw/return method, sync when builtin, async (pending
/// state) when a JS function. `call_arg` is the method argument; `closing`
/// marks IteratorClose calls whose value is ignored.
#[allow(clippy::too_many_arguments)]
fn forward_call_delegate_method(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    gen_id: usize,
    is_throw: bool,
    abrupt_arg: Value,
    iter: Value,
    method: Value,
    call_arg: Value,
    closing: bool,
) -> AfrOut {
    if method.as_smi().is_some_and(|s| s < 0) {
        match call_builtin_sync(vm, gc, method, iter, &[call_arg]) {
            Ok(v) => {
                if closing {
                    // IteratorClose for a throw-violation: the protocol
                    // violation TypeError continues (not the original).
                    return if is_throw {
                        AfrOut::Raise(violation_type_error(vm, gc, true))
                    } else {
                        vm.generators[gen_id].done = true;
                        vm.generators[gen_id].in_delegate = false;
                        AfrOut::Sync(iter_result_with_proto(gc, vm, abrupt_arg, true))
                    };
                }
                finish_delegate_afr(vm, gc, gen_id, is_throw, abrupt_arg, v)
            }
            Err(Some(crate::vm::Exit::Throw(v))) => AfrOut::Raise(v),
            Err(_) => {
                // Redirected or exotic exit already arranged; surface a
                // generic unwind for the caller to propagate.
                AfrOut::Raise(crate::errors::error_object(
                    gc,
                    &vm.error_protos,
                    crate::errors::ErrorKind::TypeError,
                    "delegate method threw",
                ))
            }
        }
    } else {
        vm.pending_yield_star_afr = Some(crate::vm::PendingYieldStarAbrupt {
            source_frame_depth: vm.frame_depth() - 1,
            gen_id,
            is_throw,
            abrupt_arg,
            iter,
            closing,
        });
        vm.push_callback_call(gc, method, iter, vec![call_arg]);
        AfrOut::Wait
    }
}

pub fn generator_return_builtin(
    _gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let v = args.first().copied().unwrap_or(Value::undefined());
    let Some(gen_id) = generator_id_of(vm, this) else {
        return iter_result_with_proto(_gc, vm, v, true);
    };
    if vm.generators[gen_id].executing {
        vm.set_pending_exception(crate::errors::error_object(
            _gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "generator already running",
        ));
        return Value::undefined();
    }
    if !vm.generators[gen_id].started || vm.generators[gen_id].done {
        vm.generators[gen_id].done = true;
        return iter_result_with_proto(_gc, vm, v, true);
    }
    // Suspended inside yield* delegation: forward to the delegate's return
    // (or complete directly when absent).
    if vm.generators[gen_id].in_delegate {
        return match forward_to_delegate(vm, _gc, gen_id, false, v) {
            AfrOut::Sync(rv) => rv,
            AfrOut::Wait => Value::undefined(),
            AfrOut::Raise(err) => {
                vm.set_pending_exception(err);
                Value::undefined()
            }
        };
    }
    // Resume in Return mode: finally blocks run, then completion (v, true).
    match vm.resume_generator_full(_gc, gen_id, crate::vm::GeneratorResume::Return(v)) {
        Ok((rv, done)) => iter_result_with_proto(_gc, vm, rv, done),
        Err(e) => {
            vm.set_pending_exception(e);
            Value::undefined()
        }
    }
}

pub fn generator_throw_builtin(
    _gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let e = args.first().copied().unwrap_or(Value::undefined());
    let Some(gen_id) = generator_id_of(vm, this) else {
        vm.set_pending_exception(e);
        return Value::undefined();
    };
    if vm.generators[gen_id].executing {
        vm.set_pending_exception(crate::errors::error_object(
            _gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "generator already running",
        ));
        return Value::undefined();
    }
    if vm.generators[gen_id].done {
        vm.set_pending_exception(e);
        return Value::undefined();
    }
    if !vm.generators[gen_id].started {
        // Throw before start: complete abruptly without running the body.
        vm.generators[gen_id].done = true;
        vm.set_pending_exception(e);
        return Value::undefined();
    }
    // Suspended inside yield* delegation: forward to the delegate's throw
    // (with IteratorClose fallback), preserving the outer abrupt.
    if vm.generators[gen_id].in_delegate {
        return match forward_to_delegate(vm, _gc, gen_id, true, e) {
            AfrOut::Sync(v) => v,
            AfrOut::Wait => Value::undefined(),
            AfrOut::Raise(err) => {
                vm.set_pending_exception(err);
                Value::undefined()
            }
        };
    }
    // Resume in Throw mode: in-generator catch/finally blocks run; uncaught
    // throws complete the generator and propagate.
    match vm.resume_generator_full(_gc, gen_id, crate::vm::GeneratorResume::Throw(e)) {
        Ok((rv, done)) => iter_result_with_proto(_gc, vm, rv, done),
        Err(err) => {
            vm.set_pending_exception(err);
            Value::undefined()
        }
    }
}

pub fn generator_symbol_iterator_builtin(
    _gc: &mut SemiSpace,
    this: Value,
    _args: &[Value],
    _vm: &mut Vm,
) -> Value {
    this
}

/// Build the instance object returned by calling a generator function.
pub fn make_generator_instance(gc: &mut SemiSpace, vm: &mut Vm, gen_id: usize) -> Value {
    let next_h = find_handle(&vm.builtins, "Generator_prototype_next").unwrap();
    let ret_h = find_handle(&vm.builtins, "Generator_prototype_return").unwrap();
    let thr_h = find_handle(&vm.builtins, "Generator_prototype_throw").unwrap();
    let it_h = find_handle(&vm.builtins, "Generator_prototype_symbol_iterator").unwrap();
    let keys = vec![
        (PropertyKey::from_string("next"), 0),
        (PropertyKey::from_string("return"), 1),
        (PropertyKey::from_string("throw"), 2),
        (PropertyKey::from_symbol(rune_core::symbol::SYM_ITERATOR), 3),
        (PropertyKey::from_symbol(vm.gen_state_symbol), 4),
    ];
    let key_names = vec![
        "next".to_string(),
        "return".to_string(),
        "throw".to_string(),
        "\u{0}".to_string(),
        "\u{0}".to_string(),
    ];
    let shape = Shape::intern(keys, key_names);
    let vals = vec![next_h, ret_h, thr_h, it_h, Value::smi(gen_id as i32)];
    let obj = JSObject::allocate(gc, shape, &vals);
    Value::from_heap_ptr(obj as *mut u8)
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
                                let v = unsafe {
                                    RuneArray::get_element(arr_ptr as *mut RuneArray, index)
                                };
                                // B1e: holes yield undefined (Get semantics;
                                // the iterator does not skip them).
                                if v == Value::empty_sentinel() {
                                    Value::undefined()
                                } else {
                                    v
                                }
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
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "String.prototype[Symbol.iterator] requires a string receiver",
            ));
            return Value::undefined();
        }
    } else {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "String.prototype[Symbol.iterator] requires a string receiver",
        ));
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
    vm.set_pending_exception(crate::errors::error_object(
        gc,
        &vm.error_protos,
        crate::errors::ErrorKind::TypeError,
        "Map.prototype method called on incompatible receiver",
    ));
    None
}

fn set_receiver(gc: &mut SemiSpace, this: Value, vm: &mut Vm) -> Option<*mut u8> {
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_SET {
            return Some(ptr);
        }
    }
    vm.set_pending_exception(crate::errors::error_object(
        gc,
        &vm.error_protos,
        crate::errors::ErrorKind::TypeError,
        "Set.prototype method called on incompatible receiver",
    ));
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
    vm.set_pending_exception(crate::errors::error_object(
        gc,
        &vm.error_protos,
        crate::errors::ErrorKind::TypeError,
        "Date.prototype method called on incompatible receiver",
    ));
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
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::RangeError,
                "Invalid time value",
            ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::RangeError,
            "Invalid typed array length",
        ));
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
    vm.set_pending_exception(crate::errors::error_object(
        gc,
        &vm.error_protos,
        crate::errors::ErrorKind::TypeError,
        "Method called on incompatible receiver",
    ));
    None
}

/// Receiver check for ArrayBuffer builtins.
fn array_buffer_receiver(gc: &mut SemiSpace, this: Value, vm: &mut Vm) -> Option<*mut u8> {
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_ARRAY_BUFFER {
            return Some(ptr);
        }
    }
    vm.set_pending_exception(crate::errors::error_object(
        gc,
        &vm.error_protos,
        crate::errors::ErrorKind::TypeError,
        "Method called on incompatible receiver",
    ));
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
                vm.set_pending_exception(crate::errors::error_object(
                    gc,
                    &vm.error_protos,
                    crate::errors::ErrorKind::RangeError,
                    "Start offset of Uint8Array should be a multiple of 1",
                ));
                return Value::undefined();
            }
            let buf_len = unsafe { typedarray::RuneArrayBuffer::byte_length(fp) };
            let (new_byte_len, new_len) = if length_arg.is_undefined() {
                if buf_len % size != 0 {
                    vm.set_pending_exception(crate::errors::error_object(
                        gc,
                        &vm.error_protos,
                        crate::errors::ErrorKind::RangeError,
                        "Attempting to construct an invalid TypedArray",
                    ));
                    return Value::undefined();
                }
                if buf_len < offset {
                    vm.set_pending_exception(crate::errors::error_object(
                        gc,
                        &vm.error_protos,
                        crate::errors::ErrorKind::RangeError,
                        "Start offset is outside the bounds of the buffer",
                    ));
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
                    vm.set_pending_exception(crate::errors::error_object(
                        gc,
                        &vm.error_protos,
                        crate::errors::ErrorKind::RangeError,
                        "Invalid typed array length",
                    ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::RangeError,
            "Offset is out of bounds",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert undefined or null to object",
        ));
        return Value::undefined();
    };

    if src_len + target_offset > target_len {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::RangeError,
            "Offset is out of bounds",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "requires a typed array receiver",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "requires a typed array receiver",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "requires a typed array receiver",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "callback is not a function",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "callback is not a function",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Iterator result is not an object",
        ));
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
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "Iterator value is not an object",
            ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "value is not iterable",
        ));
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
                vm.set_pending_exception(crate::errors::error_object(
                    gc,
                    &vm.error_protos,
                    crate::errors::ErrorKind::TypeError,
                    "iterator.next is not a function",
                ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "iterator.next is not a function",
        ));
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
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "value[Symbol.iterator] is not callable",
            ));
            return FillOutcome::Threw;
        }
        SymbolMethodResult::NotFound => {
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "value is not iterable",
            ));
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
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "value is not iterable",
            ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "value[Symbol.iterator] is not callable",
        ));
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
                    vm.set_pending_exception(crate::errors::error_object(
                        gc,
                        &vm.error_protos,
                        crate::errors::ErrorKind::TypeError,
                        &format!("{name} method called on an object with a non-callable @@method property"),
                    ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert a Symbol value to a string",
        ));
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
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "Cannot convert object to primitive value",
            ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Error.prototype.toString requires that 'this' be an Object",
        ));
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
    } else if this.to_boolean().is_some() {
        "Boolean".to_string()
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert undefined or null to object",
        ));
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
                // B1e: an own accessor overlay counts as present.
                if crate::vm::array_overlay_accessor(ptr as *mut rune_core::array::RuneArray, index)
                    .is_some()
                {
                    return Value::boolean(true);
                }
                let len = unsafe {
                    rune_core::array::RuneArray::length(ptr as *mut rune_core::array::RuneArray)
                };
                // B1e: holes (and unallocated length-extended tail
                // slots) are not own properties.
                index < len as usize
                    && index
                        < unsafe {
                            rune_core::array::RuneArray::capacity(
                                ptr as *mut rune_core::array::RuneArray,
                            )
                        } as usize
                    && unsafe {
                        rune_core::array::RuneArray::get_element(
                            ptr as *mut rune_core::array::RuneArray,
                            index,
                        )
                    } != Value::empty_sentinel()
            } else {
                key_is_str("length")
            }
        }
        TAG_TYPED_ARRAY => {
            if let Some(index) = value_to_array_index(key) {
                // B1e: an own accessor overlay counts as present.
                if crate::vm::array_overlay_accessor(ptr as *mut rune_core::array::RuneArray, index)
                    .is_some()
                {
                    return Value::boolean(true);
                }
                let len = unsafe { rune_core::typedarray::RuneTypedArray::length(ptr) };
                index < len
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert undefined or null to object",
        ));
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
                // B1e: an own accessor overlay is own; enumerability
                // comes from its stored attributes.
                let overlay = unsafe {
                    rune_core::array::RuneArray::extra_props(
                        ptr as *mut rune_core::array::RuneArray,
                    )
                };
                if !overlay.is_null() {
                    let okey = rune_core::shape::PropertyKey::from_string(&index.to_string());
                    let oshape = unsafe { JSObject::shape_ptr(overlay as *mut JSObject) };
                    if let Some(oslot) = oshape.lookup(&okey) {
                        let ov = unsafe { JSObject::get_slot(overlay as *mut JSObject, oslot) };
                        if ov.heap_ptr().is_some_and(|vp| unsafe {
                            (*(vp as *const GcHeader)).tag() == TAG_ACCESSOR
                        }) {
                            return Value::boolean(
                                oshape.attr_at(oslot) & rune_core::shape::ATTR_ENUMERABLE != 0,
                            );
                        }
                    }
                }
                let len = unsafe {
                    rune_core::array::RuneArray::length(ptr as *mut rune_core::array::RuneArray)
                };
                // B1e: holes (and unallocated length-extended tail
                // slots) are not own properties.
                index < len as usize
                    && index
                        < unsafe {
                            rune_core::array::RuneArray::capacity(
                                ptr as *mut rune_core::array::RuneArray,
                            )
                        } as usize
                    && unsafe {
                        rune_core::array::RuneArray::get_element(
                            ptr as *mut rune_core::array::RuneArray,
                            index,
                        )
                    } != Value::empty_sentinel()
            } else {
                key_is_str("length")
            }
        }
        TAG_TYPED_ARRAY => {
            if let Some(index) = value_to_array_index(key) {
                let len = unsafe { rune_core::typedarray::RuneTypedArray::length(ptr) };
                index < len
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Object.prototype.valueOf called on non-object",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Object.getPrototypeOf called on non-object",
        ));
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
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Object.prototype.isPrototypeOf called on non-object",
        ));
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
        let msg = crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Object.keys called on null or undefined",
        );
        vm.set_pending_exception(msg);
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
                    // A4: non-enumerable own properties are excluded.
                    if shape.attr_at(i) & rune_core::shape::ATTR_ENUMERABLE == 0 {
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
                    // B1e: holes are not own properties (keys/values/
                    // entries/for-in all skip them).
                    if value == Value::empty_sentinel() {
                        continue;
                    }
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

/// Object.assign(target, ...sources) — §20.1.2.1: copies own enumerable
/// properties (indices ascending first, then string keys in insertion order —
/// the order object_own_entries already produces) onto target.
pub fn object_assign(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let target = args.first().copied().unwrap_or(Value::undefined());
    // Step 1: ToObject(target) — null/undefined throw; primitives become
    // fresh empty objects (engine ToObject-lite).
    let target_obj = if target.is_null() || target.is_undefined() {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert undefined or null to object",
        ));
        return Value::undefined();
    } else if target.heap_ptr().is_none() {
        let shape = Shape::empty();
        let ptr = JSObject::allocate(gc, shape, &[]);
        unsafe {
            if let Some(proto) = vm.object_prototype.heap_ptr() {
                *((ptr as *mut u8).add(24) as *mut *mut u8) = proto;
            }
        }
        Value::from_heap_ptr(ptr as *mut u8)
    } else {
        target
    };
    // Step 3: for each source (skip undefined/null)
    for &next_source in args.iter().skip(1) {
        if next_source.is_null() || next_source.is_undefined() {
            continue;
        }
        let entries = match object_own_entries(gc, next_source, vm) {
            Ok(e) => e,
            Err(()) => return Value::undefined(),
        };
        for (key, value) in entries {
            // Set(targetObj, key, value, true) via the VM's store path so
            // dense arrays / extra_props / shapes all behave identically.
            let key_val = Value::from_heap_ptr(HeapString::allocate(gc, &key) as *mut u8);
            crate::vm::do_store_property(target_obj, key_val, value, gc, vm);
        }
    }
    target_obj
}

/// Object.is(value1, value2) — §20.1.2.15 SameValue: NaN↔NaN true, ±0 distinct.
pub fn object_is(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let a = args.first().copied().unwrap_or(Value::undefined());
    let b = args.get(1).copied().unwrap_or(Value::undefined());
    let na = a.as_smi().map(|s| s as f64).or_else(|| a.as_float64());
    let nb = b.as_smi().map(|s| s as f64).or_else(|| b.as_float64());
    if let (Some(x), Some(y)) = (na, nb) {
        if x.is_nan() && y.is_nan() {
            return Value::boolean(true);
        }
        if x == 0.0 && y == 0.0 {
            // Smi zero is always +0; floats carry a sign bit
            let neg = |v: Value, f: f64| !v.is_smi() && f.is_sign_negative();
            return Value::boolean(neg(a, x) == neg(b, y));
        }
        return Value::boolean(x == y);
    }
    Value::boolean(crate::vm::values_strictly_equal(a, b))
}

/// Object.getOwnPropertyNames(obj) — §20.1.2.10. The engine does not track
/// enumerability flags on shapes yet, so this returns the same key set as
/// Object.keys (documented gap).
pub fn object_get_own_property_names(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
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

/// Object.fromEntries(iterable) — §20.1.2.7. Sync-only: accepts an array of
/// [key, value] pairs (general iterables drain via JS callbacks and are not
/// supported inside sync builtins — documented gap).
pub fn object_from_entries(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let iterable = args.first().copied().unwrap_or(Value::undefined());
    if iterable.is_null() || iterable.is_undefined() {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Object.fromEntries called on null or undefined",
        ));
        return Value::undefined();
    }
    // Step 2: fresh ordinary object
    let shape = Shape::empty();
    let obj_ptr = JSObject::allocate(gc, shape, &[]);
    unsafe {
        if let Some(proto) = vm.object_prototype.heap_ptr() {
            *((obj_ptr as *mut u8).add(24) as *mut *mut u8) = proto;
        }
    }
    let obj = Value::from_heap_ptr(obj_ptr as *mut u8);
    // AddEntriesFromIterable — array-of-pairs support only
    if let Some(ptr) = iterable.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_ARRAY {
            let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
            for i in 0..len {
                let entry = crate::vm::array_like_index(iterable, i).unwrap_or(Value::undefined());
                let key_v = crate::vm::array_like_index(entry, 0).unwrap_or(Value::undefined());
                let val_v = crate::vm::array_like_index(entry, 1).unwrap_or(Value::undefined());
                // ToPropertyKey(key): strings direct, numbers ToString'd
                let key_str = match key_v.as_smi() {
                    Some(n) => n.to_string(),
                    None => {
                        if let Some(f) = key_v.as_float64() {
                            f.to_string()
                        } else if let Some(kp) = key_v.heap_ptr() {
                            let ktag = unsafe { (*(kp as *const GcHeader)).tag() };
                            if ktag == TAG_STRING {
                                unsafe { HeapString::to_string(kp as *mut HeapString) }
                            } else {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    }
                };
                unsafe {
                    JSObject::add_property(
                        obj_ptr,
                        PropertyKey::from_string(&key_str),
                        key_str,
                        val_v,
                    );
                }
            }
        }
    }
    obj
}

/// Object.hasOwn(obj, key) — §20.1.2.14: HasOwnProperty after ToObject.
pub fn object_has_own(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let target = args.first().copied().unwrap_or(Value::undefined());
    if target.is_null() || target.is_undefined() {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert undefined or null to object",
        ));
        return Value::undefined();
    }
    let key = args.get(1).copied().unwrap_or(Value::undefined());
    let Some(ptr) = target.heap_ptr() else {
        return Value::boolean(false);
    };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    match tag {
        TAG_OBJECT => {
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            if let Some(pk) = crate::vm::value_to_prop_key(key) {
                Value::boolean(shape.lookup(&pk).is_some())
            } else {
                Value::boolean(false)
            }
        }
        TAG_ARRAY => {
            if let Some(idx) = crate::vm::value_to_array_index(key) {
                // B1e: an own accessor overlay counts as present.
                if crate::vm::array_overlay_accessor(ptr as *mut RuneArray, idx).is_some() {
                    return Value::boolean(true);
                }
                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
                // B1e: holes are not own properties.
                Value::boolean(
                    (idx as u32) < len
                        && idx < unsafe { RuneArray::capacity(ptr as *mut RuneArray) } as usize
                        && unsafe { RuneArray::get_element(ptr as *mut RuneArray, idx) }
                            != Value::empty_sentinel(),
                )
            } else if let Some(pk) = crate::vm::value_to_prop_key(key) {
                if pk.as_u64() == PropertyKey::from_string("length").as_u64() {
                    return Value::boolean(true);
                }
                let extra = unsafe { RuneArray::extra_props(ptr as *mut RuneArray) };
                if !extra.is_null() {
                    let shape = unsafe { JSObject::shape_ptr(extra as *mut JSObject) };
                    Value::boolean(shape.lookup(&pk).is_some())
                } else {
                    Value::boolean(false)
                }
            } else {
                Value::boolean(false)
            }
        }
        _ => Value::boolean(false),
    }
}

/// Object.setPrototypeOf(obj, proto) — §20.1.2.23. Note step order: an
/// invalid proto throws even when obj is a primitive passthrough.
pub fn object_set_prototype_of(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let obj = args.first().copied().unwrap_or(Value::undefined());
    let proto = args.get(1).copied().unwrap_or(Value::undefined());
    // Step 1: RequireObjectCoercible(obj)
    if obj.is_null() || obj.is_undefined() {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert undefined or null to object",
        ));
        return Value::undefined();
    }
    // Step 2: proto must be Object or null
    let proto_ok = proto.is_null()
        || proto
            .heap_ptr()
            .map(|p| {
                let t = unsafe { (*(p as *const GcHeader)).tag() };
                matches!(t, TAG_OBJECT | TAG_ARRAY | TAG_FUNC)
            })
            .unwrap_or(false);
    if !proto_ok {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "prototype must be an Object or null",
        ));
        return Value::undefined();
    }
    // Step 3: non-object obj returned as-is
    let Some(ptr) = obj.heap_ptr() else {
        return obj;
    };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    let new_proto = if proto.is_null() {
        std::ptr::null_mut()
    } else {
        proto.heap_ptr().unwrap()
    };
    match tag {
        TAG_OBJECT => unsafe { JSObject::set_prototype(ptr as *mut JSObject, new_proto) },
        TAG_ARRAY => unsafe { RuneArray::set_prototype(ptr as *mut RuneArray, new_proto) },
        _ => {}
    }
    obj
}

/// Object.create(proto) — creates a new object with the given prototype.
/// Per §20.1.2.2, throws TypeError if proto is not an Object or null.
/// A4: parsed property descriptor (§6.1.7.1 lite). `has_*` tracks field
/// presence (explicit `undefined` differs from absent for defaults).
struct PropDesc {
    has_value: bool,
    value: Value,
    has_writable: bool,
    writable: bool,
    has_get: bool,
    get: Value,
    has_set: bool,
    set: Value,
    has_enumerable: bool,
    enumerable: bool,
    has_configurable: bool,
    configurable: bool,
}

impl PropDesc {
    fn empty() -> Self {
        PropDesc {
            has_value: false,
            value: Value::undefined(),
            has_writable: false,
            writable: false,
            has_get: false,
            get: Value::undefined(),
            has_set: false,
            set: Value::undefined(),
            has_enumerable: false,
            enumerable: false,
            has_configurable: false,
            configurable: false,
        }
    }
}

/// Read one descriptor field: presence via proto-chain walk + value via the
/// shared recursive load. Accessor-valued fields are out of scope (sync
/// builtins can't run getters) — treated as absent, documented gap.
fn desc_field(desc_obj: Value, name: &str) -> (bool, Value) {
    let Some(dptr) = desc_obj.heap_ptr() else {
        return (false, Value::undefined());
    };
    let dtag = unsafe { (*(dptr as *const GcHeader)).tag() };
    // Function descriptors store fields on their extra_props object.
    let start = if dtag == TAG_FUNC {
        let ep = unsafe {
            rune_core::function::Func::extra_props(dptr as *mut rune_core::function::Func)
        };
        if ep.is_null() {
            return (false, Value::undefined());
        }
        ep
    } else if dtag == TAG_OBJECT {
        dptr
    } else {
        return (false, Value::undefined());
    };
    let key = PropertyKey::from_string(name);
    let mut current = start;
    for _ in 0..64 {
        let shape = unsafe { JSObject::shape_ptr(current as *mut JSObject) };
        if let Some(slot) = shape.lookup(&key) {
            let v = unsafe { JSObject::get_slot(current as *mut JSObject, slot) };
            // Accessor-held descriptor fields: treated as absent (sync gap).
            if v.heap_ptr()
                .is_some_and(|vp| unsafe { (*(vp as *const GcHeader)).tag() == TAG_ACCESSOR })
            {
                return (false, Value::undefined());
            }
            return (true, v);
        }
        let proto = unsafe { JSObject::prototype(current as *mut JSObject) };
        if proto.is_null() {
            return (false, Value::undefined());
        }
        current = proto;
    }
    (false, Value::undefined())
}

/// ToPropertyDescriptor (§6.1.7.1 + §20.1.2.3 validation): parse a
/// descriptor object, rejecting data/accessor mixing and non-callable
/// getters/setters with TypeError values.
fn to_property_descriptor(gc: &mut SemiSpace, vm: &Vm, desc_obj: Value) -> Result<PropDesc, Value> {
    // ToPropertyDescriptor requires Type(Obj) = Object — primitives
    // (including strings) throw; functions count as objects.
    if !is_object_value(desc_obj) {
        return Err(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Property descriptor must be an object",
        ));
    }
    let mut d = PropDesc::empty();
    let (p, v) = desc_field(desc_obj, "enumerable");
    d.has_enumerable = p;
    d.enumerable = v.to_bool();
    let (p, v) = desc_field(desc_obj, "configurable");
    d.has_configurable = p;
    d.configurable = v.to_bool();
    (d.has_value, d.value) = desc_field(desc_obj, "value");
    let (p, v) = desc_field(desc_obj, "writable");
    d.has_writable = p;
    d.writable = v.to_bool();
    let (has_get, get) = desc_field(desc_obj, "get");
    let (has_set, set) = desc_field(desc_obj, "set");
    // Present-but-undefined means absent; present null is NOT callable and
    // must throw (only undefined is exempt).
    if has_get && !get.is_undefined() {
        match classify_method(get) {
            Ok(Some(_)) => {
                d.has_get = true;
                d.get = get;
            }
            _ => {
                return Err(crate::errors::error_object(
                    gc,
                    &vm.error_protos,
                    crate::errors::ErrorKind::TypeError,
                    "Getter must be a function",
                ));
            }
        }
    }
    if has_set && !set.is_undefined() {
        match classify_method(set) {
            Ok(Some(_)) => {
                d.has_set = true;
                d.set = set;
            }
            _ => {
                return Err(crate::errors::error_object(
                    gc,
                    &vm.error_protos,
                    crate::errors::ErrorKind::TypeError,
                    "Setter must be a function",
                ));
            }
        }
    }
    if (d.has_value || d.has_writable) && (d.has_get || d.has_set) {
        return Err(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Invalid property descriptor: cannot mix data and accessor fields",
        ));
    }
    Ok(d)
}

/// PropertyKey + key_name recovery for definition (§7.1.17 lite): strings,
/// symbols (empty key_name — excluded from enumeration by tag), Smis and
/// String wrappers. Other heap values cannot round-trip a name → TypeError.
fn define_key_and_name(raw_key: Value) -> Result<(PropertyKey, String), ()> {
    if let Some(id) = raw_key.as_symbol_id() {
        return Ok((PropertyKey::from_symbol(id), String::new()));
    }
    if let Some(ptr) = raw_key.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            return Ok((PropertyKey::from_string(&s), s));
        }
        if tag == TAG_STRING_OBJ {
            let sptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
            let s = unsafe { HeapString::to_string(sptr as *mut HeapString) };
            return Ok((PropertyKey::from_string(&s), s));
        }
        return Err(());
    }
    if let Some(v) = raw_key.as_smi() {
        return Ok((PropertyKey::from_string(&v.to_string()), v.to_string()));
    }
    Err(())
}

/// Core [[DefineOwnProperty]] for TAG_OBJECT receivers (A4). Shared by
/// defineProperty/defineProperties/create-with-properties. Implements
/// OrdinaryDefineOwnProperty + ValidateAndApplyPropertyDescriptor for data
/// and accessor descriptors (accessor slot values are AccessorPairs, which
/// the F3 getter/setter paths already dispatch).
fn define_own_property(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    mut obj_ptr: *mut JSObject,
    key: PropertyKey,
    key_name: String,
    desc: &PropDesc,
) -> Result<*mut JSObject, Value> {
    let mut type_err = |msg: &str| {
        crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            msg,
        )
    };
    let shape = unsafe { JSObject::shape_ptr(obj_ptr) };
    let is_accessor_desc = desc.has_get || desc.has_set;
    let attr_for = |d: &PropDesc| -> u8 {
        let mut a = 0u8;
        if d.has_enumerable && d.enumerable {
            a |= rune_core::shape::ATTR_ENUMERABLE;
        }
        if d.has_configurable && d.configurable {
            a |= rune_core::shape::ATTR_CONFIGURABLE;
        }
        if !is_accessor_desc && d.has_writable && d.writable {
            a |= rune_core::shape::ATTR_WRITABLE;
        }
        a
    };
    match shape.lookup(&key) {
        None => {
            // Create with absent fields defaulted (data: value undefined,
            // writable false; all flags false).
            if unsafe { !JSObject::is_extensible(obj_ptr) } {
                return Err(type_err(
                    "Cannot define property on a non-extensible object",
                ));
            }
            let attr = attr_for(desc);
            // B1f: room first — the pair allocation below may GC-move us,
            // and the add itself must never hit the capacity assert.
            obj_ptr = vm.ensure_object_capacity(gc, obj_ptr);
            if is_accessor_desc {
                let pair = rune_core::accessor::AccessorPair::allocate(
                    gc,
                    if desc.has_get {
                        desc.get
                    } else {
                        Value::undefined()
                    },
                    if desc.has_set {
                        desc.set
                    } else {
                        Value::undefined()
                    },
                );
                obj_ptr = resolve_forwarded(obj_ptr as *mut u8) as *mut JSObject;
                unsafe {
                    JSObject::add_property_with_attrs(
                        obj_ptr,
                        key,
                        key_name,
                        Value::from_heap_ptr(pair),
                        attr,
                    );
                }
            } else {
                unsafe {
                    JSObject::add_property_with_attrs(
                        obj_ptr,
                        key,
                        key_name,
                        if desc.has_value {
                            desc.value
                        } else {
                            Value::undefined()
                        },
                        attr,
                    );
                }
            }
            Ok(obj_ptr)
        }
        Some(slot) => {
            let cur_attr = shape.attr_at(slot);
            let cur_val = unsafe { JSObject::get_slot(obj_ptr, slot) };
            let cur_is_accessor = cur_val
                .heap_ptr()
                .is_some_and(|vp| unsafe { (*(vp as *const GcHeader)).tag() == TAG_ACCESSOR });
            let cur_configurable = cur_attr & rune_core::shape::ATTR_CONFIGURABLE != 0;
            if !cur_configurable {
                // Locked-down property: reject everything but compatible
                // same-kind, same-value (SameValue) updates.
                if desc.has_configurable && desc.configurable {
                    return Err(type_err("Cannot redefine a non-configurable property"));
                }
                if desc.has_enumerable
                    && desc.enumerable != (cur_attr & rune_core::shape::ATTR_ENUMERABLE != 0)
                {
                    return Err(type_err("Cannot redefine a non-configurable property"));
                }
                if cur_is_accessor != is_accessor_desc
                    && (desc.has_value || desc.has_writable || desc.has_get || desc.has_set)
                {
                    return Err(type_err("Cannot redefine a non-configurable property"));
                }
                if !cur_is_accessor && !is_accessor_desc {
                    let cur_writable = cur_attr & rune_core::shape::ATTR_WRITABLE != 0;
                    if !cur_writable {
                        if desc.has_writable && desc.writable {
                            return Err(type_err("Cannot make a non-writable property writable"));
                        }
                        if desc.has_value && !same_value(desc.value, cur_val) {
                            return Err(type_err("Cannot assign to a non-writable property"));
                        }
                    }
                }
                if cur_is_accessor && is_accessor_desc {
                    let (cur_get, cur_set) = unsafe {
                        let vp = cur_val.heap_ptr().unwrap();
                        (
                            rune_core::accessor::AccessorPair::getter(vp),
                            rune_core::accessor::AccessorPair::setter(vp),
                        )
                    };
                    if desc.has_get && !same_value(desc.get, cur_get) {
                        return Err(type_err("Cannot redefine a non-configurable property"));
                    }
                    if desc.has_set && !same_value(desc.set, cur_set) {
                        return Err(type_err("Cannot redefine a non-configurable property"));
                    }
                }
            }
            // Apply: merge descriptor fields over current, keep the rest.
            // Configurable: provided bits win, absent bits preserved (data
            // results keep current-writable when unwritten; conversions take
            // descriptor-or-false). Non-configurable: only writable
            // true→false may change (validated above).
            use rune_core::shape::{ATTR_CONFIGURABLE, ATTR_ENUMERABLE, ATTR_WRITABLE};
            let cur_e = cur_attr & ATTR_ENUMERABLE != 0;
            let cur_c = cur_attr & ATTR_CONFIGURABLE != 0;
            let cur_w = cur_attr & ATTR_WRITABLE != 0;
            let merged_attr = if cur_configurable {
                let mut a = 0u8;
                if desc.has_enumerable {
                    if desc.enumerable {
                        a |= ATTR_ENUMERABLE;
                    }
                } else if cur_e {
                    a |= ATTR_ENUMERABLE;
                }
                if desc.has_configurable {
                    if desc.configurable {
                        a |= ATTR_CONFIGURABLE;
                    }
                } else if cur_c {
                    a |= ATTR_CONFIGURABLE;
                }
                if !is_accessor_desc {
                    let w = if desc.has_writable {
                        desc.writable
                    } else if !cur_is_accessor {
                        cur_w
                    } else {
                        false
                    };
                    if w {
                        a |= ATTR_WRITABLE;
                    }
                }
                a
            } else if !cur_is_accessor
                && !is_accessor_desc
                && cur_w
                && desc.has_writable
                && !desc.writable
            {
                cur_attr & !ATTR_WRITABLE
            } else {
                cur_attr
            };
            let new_shape = rune_core::shape::Shape::with_replaced_attr(shape, slot, merged_attr);
            unsafe {
                JSObject::set_shape_ptr(obj_ptr, new_shape);
            }
            // Store the value (data) or pair (accessor) when provided.
            if is_accessor_desc {
                let (get, set) = if cur_is_accessor && cur_configurable {
                    let vp = cur_val.heap_ptr().unwrap();
                    unsafe {
                        (
                            if desc.has_get {
                                desc.get
                            } else {
                                rune_core::accessor::AccessorPair::getter(vp)
                            },
                            if desc.has_set {
                                desc.set
                            } else {
                                rune_core::accessor::AccessorPair::setter(vp)
                            },
                        )
                    }
                } else {
                    (
                        if desc.has_get {
                            desc.get
                        } else {
                            Value::undefined()
                        },
                        if desc.has_set {
                            desc.set
                        } else {
                            Value::undefined()
                        },
                    )
                };
                let pair = rune_core::accessor::AccessorPair::allocate(gc, get, set);
                // B1f: the allocation may have GC-moved the object.
                obj_ptr = resolve_forwarded(obj_ptr as *mut u8) as *mut JSObject;
                unsafe {
                    JSObject::set_slot(obj_ptr, slot, Value::from_heap_ptr(pair));
                }
            } else if desc.has_value {
                unsafe {
                    JSObject::set_slot(obj_ptr, slot, desc.value);
                }
            }
            Ok(obj_ptr)
        }
    }
}

/// Require a TAG_OBJECT receiver for definition builtins (arrays and other
/// exotic receivers are follow-ups; primitives throw per ToObject).
fn define_target(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    what: &str,
    target: Value,
) -> Result<*mut JSObject, Value> {
    if target.is_null() || target.is_undefined() {
        return Err(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            &format!("{what} called on null or undefined"),
        ));
    }
    match target.heap_ptr() {
        Some(ptr) if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_OBJECT => {
            Ok(ptr as *mut JSObject)
        }
        _ => Err(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            &format!("{what} called on non-object"),
        )),
    }
}

fn define_array_type_err(gc: &mut SemiSpace, vm: &Vm, msg: &str) -> Value {
    crate::errors::error_object(
        gc,
        &vm.error_protos,
        crate::errors::ErrorKind::TypeError,
        msg,
    )
}

/// B1e: `Object.defineProperty` on a dense-array index. Data descriptors
/// write the dense element (growing with holes when past the end);
/// accessor descriptors live in the extra_props overlay (dense elements
/// can't hold AccessorPairs inline) with the dense slot left a hole.
/// Non-configurable overlay entries validate like ValidateAndApply.
fn define_array_index_property(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    arr: Value,
    arr_ptr: *mut RuneArray,
    idx: usize,
    key_name: String,
    desc: &PropDesc,
) -> Result<(), Value> {
    let is_accessor_desc = desc.has_get || desc.has_set;
    let overlay_key = PropertyKey::from_string(&idx.to_string());
    // Fetch (and lazily allocate) the overlay object.
    let overlay = unsafe {
        let mut props = RuneArray::extra_props(arr_ptr);
        if props.is_null() {
            let new_obj = JSObject::allocate(gc, Shape::empty(), &[]);
            // Re-resolve: allocation may have moved the array.
            let moved = arr.heap_ptr().unwrap();
            RuneArray::set_extra_props(moved as *mut RuneArray, new_obj as *mut u8);
            props = new_obj as *mut u8;
        }
        props
    };
    let arr_ptr = arr.heap_ptr().unwrap() as *mut RuneArray;
    let overlay_shape = unsafe { JSObject::shape_ptr(overlay as *mut JSObject) };
    let existing = overlay_shape.lookup(&overlay_key).map(|slot| {
        let attr = overlay_shape.attr_at(slot);
        let cur = unsafe { JSObject::get_slot(overlay as *mut JSObject, slot) };
        (slot, attr, cur)
    });
    if let Some((_, attr, _)) = existing {
        use rune_core::shape::{ATTR_CONFIGURABLE, ATTR_ENUMERABLE};
        if attr & ATTR_CONFIGURABLE == 0 {
            if desc.has_configurable && desc.configurable {
                return Err(define_array_type_err(
                    gc,
                    vm,
                    "Cannot redefine a non-configurable property",
                ));
            }
            if desc.has_enumerable && desc.enumerable != (attr & ATTR_ENUMERABLE != 0) {
                return Err(define_array_type_err(
                    gc,
                    vm,
                    "Cannot redefine a non-configurable property",
                ));
            }
            // Overlay entries are always accessors; any data fields or
            // changed get/set on a locked entry reject.
            if desc.has_value || desc.has_writable {
                return Err(define_array_type_err(
                    gc,
                    vm,
                    "Cannot redefine a non-configurable property",
                ));
            }
            if is_accessor_desc {
                let vp = existing.unwrap().2.heap_ptr().unwrap();
                let (cur_get, cur_set) = unsafe {
                    (
                        rune_core::accessor::AccessorPair::getter(vp),
                        rune_core::accessor::AccessorPair::setter(vp),
                    )
                };
                if desc.has_get && !same_value(desc.get, cur_get) {
                    return Err(define_array_type_err(
                        gc,
                        vm,
                        "Cannot redefine a non-configurable property",
                    ));
                }
                if desc.has_set && !same_value(desc.set, cur_set) {
                    return Err(define_array_type_err(
                        gc,
                        vm,
                        "Cannot redefine a non-configurable property",
                    ));
                }
            }
        }
    }
    if is_accessor_desc {
        // Merge get/set over the current pair when redefining.
        let (get, set) = match existing {
            Some((_, _, cur))
                if cur.heap_ptr().is_some_and(|vp| unsafe {
                    (*(vp as *const GcHeader)).tag() == TAG_ACCESSOR
                }) =>
            {
                let vp = cur.heap_ptr().unwrap();
                unsafe {
                    (
                        if desc.has_get {
                            desc.get
                        } else {
                            rune_core::accessor::AccessorPair::getter(vp)
                        },
                        if desc.has_set {
                            desc.set
                        } else {
                            rune_core::accessor::AccessorPair::setter(vp)
                        },
                    )
                }
            }
            _ => (
                if desc.has_get {
                    desc.get
                } else {
                    Value::undefined()
                },
                if desc.has_set {
                    desc.set
                } else {
                    Value::undefined()
                },
            ),
        };
        let pair = rune_core::accessor::AccessorPair::allocate(gc, get, set);
        let pair_val = Value::from_heap_ptr(pair);
        let mut attr = 0u8;
        if desc.has_enumerable && desc.enumerable {
            attr |= rune_core::shape::ATTR_ENUMERABLE;
        }
        if desc.has_configurable && desc.configurable {
            attr |= rune_core::shape::ATTR_CONFIGURABLE;
        }
        // Re-resolve after the pair allocation (GC may have moved things).
        let overlay = unsafe { RuneArray::extra_props(arr.heap_ptr().unwrap() as *mut RuneArray) };
        let overlay_shape = unsafe { JSObject::shape_ptr(overlay as *mut JSObject) };
        if let Some(slot) = overlay_shape.lookup(&overlay_key) {
            let new_shape = Shape::with_replaced_attr(overlay_shape, slot, attr);
            unsafe {
                JSObject::set_shape_ptr(overlay as *mut JSObject, new_shape);
                JSObject::set_slot(overlay as *mut JSObject, slot, pair_val);
            }
        } else {
            unsafe {
                JSObject::add_property_with_attrs(
                    overlay as *mut JSObject,
                    overlay_key,
                    key_name,
                    pair_val,
                    attr,
                );
            }
        }
        // The dense slot stays a hole (the accessor shadows it).
        let arr_ptr = arr.heap_ptr().unwrap() as *mut RuneArray;
        let len = unsafe { RuneArray::length(arr_ptr) } as usize;
        if idx < len {
            unsafe { RuneArray::set_element(arr_ptr, idx, Value::empty_sentinel()) };
        } else {
            // Defining past the end extends length (holes fill the gap).
            crate::vm::do_store_property(
                arr,
                Value::smi(idx as i32),
                Value::empty_sentinel(),
                gc,
                vm,
            );
        }
    } else {
        // Data descriptor: drop any overlay accessor, write dense.
        if existing.is_some() {
            unsafe { JSObject::remove_property(overlay as *mut JSObject, &overlay_key) };
        }
        let value = if desc.has_value {
            desc.value
        } else {
            Value::undefined()
        };
        crate::vm::do_store_property(arr, Value::smi(idx as i32), value, gc, vm);
    }
    // Silence unused-mut on arr_ptr in case the optimizer disagrees.
    let _ = arr_ptr;
    Ok(())
}

/// Object.defineProperty(obj, key, descriptor) — defines or redefines an
/// own property (§20.1.2.3). Returns obj; failures throw TypeError.
pub fn object_define_property(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let target = args.first().copied().unwrap_or(Value::undefined());
    let raw_key = args.get(1).copied().unwrap_or(Value::undefined());
    let (key, key_name) = match define_key_and_name(raw_key) {
        Ok(k) => k,
        Err(()) => {
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "Invalid property key",
            ));
            return Value::undefined();
        }
    };
    let desc_obj = args.get(2).copied().unwrap_or(Value::undefined());
    let desc = match to_property_descriptor(gc, vm, desc_obj) {
        Ok(d) => d,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    // B1e: dense-array definitions. Index keys go to the array-index
    // path (data → dense element with hole-preserving growth; accessor →
    // extra_props overlay pair). Other named keys (not "length") define on
    // the extra_props object (match-result "index"/"input" precedent).
    if let Some(tptr) = target.heap_ptr() {
        if unsafe { (*(tptr as *const GcHeader)).tag() } == TAG_ARRAY {
            if let Some(idx) = value_to_array_index(raw_key) {
                if let Err(e) = define_array_index_property(
                    gc,
                    vm,
                    target,
                    tptr as *mut RuneArray,
                    idx,
                    key_name,
                    &desc,
                ) {
                    vm.set_pending_exception(e);
                    return Value::undefined();
                }
                return target;
            }
            if let Some(pk) = value_to_prop_key(raw_key) {
                if pk.as_u64() != PropertyKey::from_string("length").as_u64() {
                    let overlay = unsafe {
                        let mut props = RuneArray::extra_props(tptr as *mut RuneArray);
                        if props.is_null() {
                            let new_obj = JSObject::allocate(gc, Shape::empty(), &[]);
                            let moved = target.heap_ptr().unwrap();
                            RuneArray::set_extra_props(moved as *mut RuneArray, new_obj as *mut u8);
                            props = new_obj as *mut u8;
                        }
                        props
                    };
                    match define_own_property(gc, vm, overlay as *mut JSObject, pk, key_name, &desc)
                    {
                        Ok(live_overlay) => {
                            // The overlay may have grown (moved): re-link it
                            // from the live array.
                            let arr_live =
                                refresh_value(target).heap_ptr().unwrap() as *mut RuneArray;
                            unsafe {
                                RuneArray::set_extra_props(arr_live, live_overlay as *mut u8);
                            }
                        }
                        Err(e) => {
                            vm.set_pending_exception(e);
                            return Value::undefined();
                        }
                    }
                    return refresh_value(target);
                }
            }
        }
    }
    let obj_ptr = match define_target(gc, vm, "Object.defineProperty", target) {
        Ok(p) => p,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    match define_own_property(gc, vm, obj_ptr, key, key_name, &desc) {
        Ok(live) => Value::from_heap_ptr(live as *mut u8),
        Err(e) => {
            vm.set_pending_exception(e);
            Value::undefined()
        }
    }
}

/// Object.defineProperties(obj, {k: descriptor}) — batch form (§20.1.2.4).
pub fn object_define_properties(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let target = args.first().copied().unwrap_or(Value::undefined());
    if let Err(e) = define_target(gc, vm, "Object.defineProperties", target) {
        vm.set_pending_exception(e);
        return Value::undefined();
    }
    let props = args.get(1).copied().unwrap_or(Value::undefined());
    if props.is_null() || props.is_undefined() {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Property list must be an object",
        ));
        return Value::undefined();
    }
    // Collect own enumerable string keys of the properties object, then
    // define each in order (partial failure leaves earlier ones defined).
    let keys: Vec<(PropertyKey, String, Value)> = {
        let shape = match props.heap_ptr() {
            Some(pptr) if unsafe { (*(pptr as *const GcHeader)).tag() } == TAG_OBJECT => unsafe {
                JSObject::shape_ptr(pptr as *mut JSObject)
            },
            _ => {
                vm.set_pending_exception(crate::errors::error_object(
                    gc,
                    &vm.error_protos,
                    crate::errors::ErrorKind::TypeError,
                    "Property list must be an object",
                ));
                return Value::undefined();
            }
        };
        let count = unsafe { JSObject::slot_count(props.heap_ptr().unwrap() as *mut JSObject) };
        let mut out = Vec::new();
        for i in 0..count {
            if shape.entries[i].0.is_symbol()
                || shape.attr_at(i) & rune_core::shape::ATTR_ENUMERABLE == 0
            {
                continue;
            }
            let v = unsafe { JSObject::get_slot(props.heap_ptr().unwrap() as *mut JSObject, i) };
            let name = shape.key_name_at(i).unwrap_or("").to_string();
            out.push((shape.entries[i].0, name, v));
        }
        out
    };
    // Root the target on the operand stack: descriptor parsing and object
    // allocation below may trigger GC, which moves unrooted values (args
    // live in a plain Rust Vec). Re-derive the raw pointer per iteration.
    vm.push(target);
    let target_slot = vm.stack.len() - 1;
    for (pkey, name, desc_obj) in keys {
        let desc = match to_property_descriptor(gc, vm, desc_obj) {
            Ok(d) => d,
            Err(e) => {
                vm.set_pending_exception(e);
                vm.stack.truncate(target_slot);
                return Value::undefined();
            }
        };
        let live_ptr = vm.stack[target_slot].heap_ptr().unwrap() as *mut JSObject;
        if let Err(e) = define_own_property(gc, vm, live_ptr, pkey, name, &desc) {
            vm.set_pending_exception(e);
            let current = vm.stack[target_slot];
            vm.stack.truncate(target_slot);
            return current;
        }
    }
    let result = vm.stack[target_slot];
    vm.stack.truncate(target_slot);
    result
}

/// Object.getOwnPropertyDescriptor(obj, key) — own-property descriptor
/// object or undefined (§20.1.2.10).
pub fn object_get_own_property_descriptor(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let target = args.first().copied().unwrap_or(Value::undefined());
    if target.is_null() || target.is_undefined() {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert undefined or null to object",
        ));
        return Value::undefined();
    }
    let Some(ptr) = target.heap_ptr() else {
        // Primitives have no own properties to describe.
        return Value::undefined();
    };
    if unsafe { (*(ptr as *const GcHeader)).tag() } != TAG_OBJECT {
        return Value::undefined();
    }
    let raw_key = args.get(1).copied().unwrap_or(Value::undefined());
    let (key, _) = match define_key_and_name(raw_key) {
        Ok(k) => k,
        Err(()) => return Value::undefined(),
    };
    let obj = ptr as *mut JSObject;
    let shape = unsafe { JSObject::shape_ptr(obj) };
    let Some(slot) = shape.lookup(&key) else {
        return Value::undefined();
    };
    let attr = shape.attr_at(slot);
    let val = unsafe { JSObject::get_slot(obj, slot) };
    let pairs: Vec<(&str, Value)> = vec![
        (
            "enumerable",
            Value::boolean(attr & rune_core::shape::ATTR_ENUMERABLE != 0),
        ),
        (
            "configurable",
            Value::boolean(attr & rune_core::shape::ATTR_CONFIGURABLE != 0),
        ),
    ];
    let (mut names, mut vals): (Vec<String>, Vec<Value>) = (vec![], vec![]);
    for (k, v) in pairs {
        names.push(k.to_string());
        vals.push(v);
    }
    if val
        .heap_ptr()
        .is_some_and(|vp| unsafe { (*(vp as *const GcHeader)).tag() == TAG_ACCESSOR })
    {
        let (get, set) = unsafe {
            let vp = val.heap_ptr().unwrap();
            (
                rune_core::accessor::AccessorPair::getter(vp),
                rune_core::accessor::AccessorPair::setter(vp),
            )
        };
        names.push("get".to_string());
        vals.push(get);
        names.push("set".to_string());
        vals.push(set);
    } else {
        names.push("value".to_string());
        vals.push(val);
        names.push("writable".to_string());
        vals.push(Value::boolean(attr & rune_core::shape::ATTR_WRITABLE != 0));
    }
    let entries: Vec<(PropertyKey, usize)> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (PropertyKey::from_string(n), i))
        .collect();
    let shape = Shape::intern(entries, names);
    let obj = JSObject::allocate(gc, shape, &vals);
    Value::from_heap_ptr(obj as *mut u8)
}

/// Shared integrity-level helper: preventExtensions (level 0), seal (1),
/// freeze (2). Mass-updates attributes via shape transitions and clears the
/// extensible bit. Returns obj (seal/freeze/preventExtensions) — the
/// is-checks are separate builtins below.
fn set_integrity_level(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    what: &str,
    target: Value,
    level: u8,
) -> Result<Value, Value> {
    let obj_ptr = define_target(gc, vm, what, target)?;
    let shape = unsafe { JSObject::shape_ptr(obj_ptr) };
    let mut attrs = shape.attrs.clone();
    while attrs.len() < shape.entries.len() {
        attrs.push(rune_core::shape::ATTR_DEFAULT);
    }
    for (i, a) in attrs.iter_mut().enumerate() {
        // seal: configurable → false. freeze: additionally writable → false
        // for data properties (accessor slots keep no writable bit).
        *a &= !rune_core::shape::ATTR_CONFIGURABLE;
        if level >= 2 {
            let is_accessor = unsafe {
                JSObject::get_slot(obj_ptr, i)
                    .heap_ptr()
                    .is_some_and(|vp| (*(vp as *const GcHeader)).tag() == TAG_ACCESSOR)
            };
            if !is_accessor {
                *a &= !rune_core::shape::ATTR_WRITABLE;
            }
        }
    }
    let new_shape = rune_core::shape::Shape::intern_with_attrs(
        shape.entries.clone(),
        shape.key_names.clone(),
        attrs,
    );
    unsafe {
        JSObject::set_shape_ptr(obj_ptr, new_shape);
        JSObject::set_extensible(obj_ptr, false);
    }
    Ok(target)
}

/// Object.preventExtensions / seal / freeze — return the object.
macro_rules! integrity_builtin {
    ($name:ident, $what:expr, $level:expr) => {
        pub fn $name(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
            let target = args.first().copied().unwrap_or(Value::undefined());
            // Primitives: return as-is (spec ToObject would box, but the
            // observable result is the primitive itself).
            if !target.is_heap_object() && !target.is_null() && !target.is_undefined() {
                return target;
            }
            match set_integrity_level(gc, vm, $what, target, $level) {
                Ok(v) => v,
                Err(e) => {
                    vm.set_pending_exception(e);
                    Value::undefined()
                }
            }
        }
    };
}

integrity_builtin!(object_prevent_extensions, "Object.preventExtensions", 0);
integrity_builtin!(object_seal, "Object.seal", 1);
integrity_builtin!(object_freeze, "Object.freeze", 2);

/// Object.isExtensible / isSealed / isFrozen (§20.1.2.9/13/15).
/// Primitives → isExtensible false, isSealed/isFrozen true (spec).
macro_rules! integrity_test_builtin {
    ($name:ident, $kind:expr) => {
        pub fn $name(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
            let _ = gc;
            let _ = vm;
            let target = args.first().copied().unwrap_or(Value::undefined());
            let Some(ptr) = target.heap_ptr() else {
                return Value::boolean($kind != 0);
            };
            if unsafe { (*(ptr as *const GcHeader)).tag() } != TAG_OBJECT {
                // Non-plain receivers (arrays, wrappers): always extensible
                // in this engine's model (no per-object seal tracking there).
                return Value::boolean($kind == 0);
            }
            let obj = ptr as *mut JSObject;
            Value::boolean(match $kind {
                0 => unsafe { JSObject::is_extensible(obj) },
                1 => {
                    !unsafe { JSObject::is_extensible(obj) }
                        && unsafe {
                            JSObject::shape_ptr(obj)
                                .attrs
                                .iter()
                                .all(|&a| a & rune_core::shape::ATTR_CONFIGURABLE == 0)
                        }
                }
                _ => {
                    let shape = unsafe { JSObject::shape_ptr(obj) };
                    !unsafe { JSObject::is_extensible(obj) }
                        && shape
                            .attrs
                            .iter()
                            .all(|&a| a & rune_core::shape::ATTR_CONFIGURABLE == 0)
                        && (0..shape.entries.len()).all(|i| {
                            let is_accessor = unsafe {
                                JSObject::get_slot(obj, i).heap_ptr().is_some_and(|vp| {
                                    (*(vp as *const GcHeader)).tag() == TAG_ACCESSOR
                                })
                            };
                            is_accessor || shape.attr_at(i) & rune_core::shape::ATTR_WRITABLE == 0
                        })
                }
            })
        }
    };
}

integrity_test_builtin!(object_is_extensible, 0);
integrity_test_builtin!(object_is_sealed, 1);
integrity_test_builtin!(object_is_frozen, 2);

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
            vm.set_pending_exception(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "Object.create expects an object or null",
            ));
        }
    }
    // A4: Object.create(proto, propertiesObject) — define each own
    // enumerable property of propertiesObject on the new object (§20.1.2.2
    // step 2, via the shared defineProperty core).
    if args.len() >= 2 && !args[1].is_undefined() {
        let props = args[1];
        let keys: Vec<(PropertyKey, String, Value)> = match props.heap_ptr() {
            Some(pptr) if unsafe { (*(pptr as *const GcHeader)).tag() } == TAG_OBJECT => {
                let shape = unsafe { JSObject::shape_ptr(pptr as *mut JSObject) };
                let count = unsafe { JSObject::slot_count(pptr as *mut JSObject) };
                let mut out = Vec::new();
                for i in 0..count {
                    if shape.entries[i].0.is_symbol()
                        || shape.attr_at(i) & rune_core::shape::ATTR_ENUMERABLE == 0
                    {
                        continue;
                    }
                    let v = unsafe { JSObject::get_slot(pptr as *mut JSObject, i) };
                    let name = shape.key_name_at(i).unwrap_or("").to_string();
                    out.push((shape.entries[i].0, name, v));
                }
                out
            }
            _ => {
                vm.set_pending_exception(crate::errors::error_object(
                    gc,
                    &vm.error_protos,
                    crate::errors::ErrorKind::TypeError,
                    "Property list must be an object",
                ));
                return Value::from_heap_ptr(ptr as *mut u8);
            }
        };
        // The new object may move during descriptor parsing allocations —
        // re-resolve through a rooted stack slot (same discipline as the
        // error constructor).
        let obj_val = Value::from_heap_ptr(ptr as *mut u8);
        vm.push(obj_val);
        let obj_slot = vm.stack.len() - 1;
        for (pkey, name, desc_obj) in keys {
            let desc = match to_property_descriptor(gc, vm, desc_obj) {
                Ok(d) => d,
                Err(e) => {
                    vm.set_pending_exception(e);
                    vm.stack.truncate(obj_slot);
                    return Value::from_heap_ptr(ptr as *mut u8);
                }
            };
            let live = vm.stack[obj_slot].heap_ptr().unwrap() as *mut JSObject;
            if let Err(e) = define_own_property(gc, vm, live, pkey, name, &desc) {
                vm.set_pending_exception(e);
                // Stack slots are GC-updated, so this is the live object.
                let current = vm.stack[obj_slot];
                vm.stack.truncate(obj_slot);
                return current;
            }
        }
        let result = vm.stack[obj_slot];
        vm.stack.truncate(obj_slot);
        return result;
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
    Value::boolean(
        val.heap_ptr()
            .is_some_and(|ptr| unsafe { (*(ptr as *const GcHeader)).tag() == TAG_ARRAY }),
    )
}

/// Array constructor body, shared by `new Array()` and plain `Array()` calls
/// (§22.1.1.1): no args → []; single Number arg → length form (ToUint32 must
/// round-trip, else RangeError); otherwise the args are the elements.
/// Dense arrays cannot represent very large sparse tails: lengths above
/// 1M allocate empty with the length set (mirrors `length=` assignment).
pub fn array_constructor(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if args.is_empty() {
        return build_array(gc, &[], vm);
    }
    if args.len() == 1 {
        let n = args[0];
        let is_number = n.as_smi().is_some() || n.as_float64().is_some();
        if is_number {
            let num = n
                .as_smi()
                .map(|v| v as f64)
                .or_else(|| n.as_float64())
                .unwrap();
            let uint = num as u32 as f64;
            if !(0.0..=4294967295.0).contains(&num) || uint != num {
                vm.set_pending_exception(crate::errors::error_object(
                    gc,
                    &vm.error_protos,
                    crate::errors::ErrorKind::RangeError,
                    "Invalid array length",
                ));
                return Value::undefined();
            }
            let len = uint as usize;
            if len > 1_000_000 {
                let arr = RuneArray::allocate(gc, &[]);
                wire_array_proto(gc, vm, arr);
                unsafe {
                    RuneArray::set_length(arr, len as u32);
                }
                return Value::from_heap_ptr(arr as *mut u8);
            }
            // B1f-3: Array(len) creates holes, not undefined elements
            // (the 8-5/8-b visitation families; reads still yield undefined
            // via the hole funnels — only presence changes).
            let elems = vec![Value::empty_sentinel(); len];
            return build_array(gc, &elems, vm);
        }
    }
    build_array(gc, args, vm)
}

/// Wire Array.prototype onto a freshly allocated dense array (shared with
/// build_array for paths that allocate directly).
fn wire_array_proto(_gc: &mut SemiSpace, vm: &Vm, arr: *mut RuneArray) {
    unsafe {
        let ptr = arr as *mut u8;
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
            *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(ptr.add(24) as *mut *mut u8) = proto;
        }
    }
}

/// Array.of(...items) — collects arguments into a fresh array (§22.1.3.1).
pub fn array_of(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    // Generic over `this` ctor in spec; engine always builds dense arrays.
    build_array(gc, args, vm)
}

/// Array.prototype.copyWithin(target, start, end?) — in-place block copy
/// (§23.1.3.4). Missing source indices DELETE the target (throwing on
/// non-configurable own targets); symbol index/length arguments throw.
/// Element reads are sync data-path (getter dispatch is future work, as in
/// B1a length getters). The source range is snapshotted first, which is
/// overlap-safe by construction.
pub fn array_copy_within(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let length = match checked_array_length(gc, vm, this) {
        Ok(len) => len as i64,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    // Spec order: target, then start, then end.
    let to = match clamp_index_throwing(gc, vm, args.first().copied(), length, 0) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    let from = match clamp_index_throwing(gc, vm, args.get(1).copied(), length, 0) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    let end = match clamp_index_throwing(gc, vm, args.get(2).copied(), length, length) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    let count = (end - from).max(0).min(length - to) as usize;
    let (to, from) = (to as usize, from as usize);
    // Presence-aware snapshot: None marks a hole (deletes the target).
    let snapshot: Vec<Option<Value>> = (0..count)
        .map(|k| {
            let key = Value::smi((from + k) as i32);
            if crate::vm::has_property(this, key, None) {
                Some(
                    crate::vm::array_like_index(this, (from + k) as u32)
                        .unwrap_or(Value::undefined()),
                )
            } else {
                None
            }
        })
        .collect();
    for (k, slot) in snapshot.into_iter().enumerate() {
        let to_key = Value::smi((to + k) as i32);
        match slot {
            Some(v) => {
                crate::vm::do_store_property(this, to_key, v, gc, vm);
            }
            None => {
                // DeletePropertyOrThrow on the target index.
                if let Some(ptr) = this.heap_ptr() {
                    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
                    if tag == TAG_OBJECT {
                        if let Some(key) = crate::vm::value_to_prop_key(to_key) {
                            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                            if let Some(s) = shape.lookup(&key) {
                                if shape.attr_at(s) & rune_core::shape::ATTR_CONFIGURABLE == 0 {
                                    vm.set_pending_exception(crate::errors::error_object(
                                        gc,
                                        &vm.error_protos,
                                        crate::errors::ErrorKind::TypeError,
                                        "Cannot delete a non-configurable property",
                                    ));
                                    return Value::undefined();
                                }
                                unsafe { JSObject::remove_property(ptr as *mut JSObject, &key) };
                            }
                        }
                    } else if tag == TAG_ARRAY {
                        // B1e: a missing source DELETES the target (hole),
                        // routed through do_store_property (grow-safe for
                        // length-extended tails).
                        crate::vm::do_store_property(this, to_key, Value::empty_sentinel(), gc, vm);
                    }
                }
            }
        }
    }
    this
}

/// Clamp an index argument per ToIntegerOrInfinity + relative clamping:
/// negative counts from len, out-of-range clamps to [0, len]. `def` is the
/// default when the argument is missing.
fn clamp_index_arg(arg: Option<Value>, len: i64, def: i64) -> i64 {
    let Some(v) = arg else { return def };
    let n = to_integer_or_infinity(v);
    if n.is_infinite() {
        return if n > 0.0 { len } else { 0 };
    }
    let n = n as i64;
    if n < 0 { (len + n).max(0) } else { n.min(len) }
}

/// Throwing clamp (B1d): symbols fail ToIntegerOrInfinity with TypeError.
/// Used by copyWithin/toSpliced/with index arguments (return-abrupt-from-*
/// tests). Value-coercion via valueOf stays a sync gap (0 fallback).
fn clamp_index_throwing(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    arg: Option<Value>,
    len: i64,
    def: i64,
) -> Result<i64, Value> {
    if let Some(v) = arg {
        if v.is_symbol() {
            return Err(crate::errors::error_object(
                gc,
                &vm.error_protos,
                crate::errors::ErrorKind::TypeError,
                "Cannot convert a Symbol value to a number",
            ));
        }
    }
    Ok(clamp_index_arg(arg, len, def))
}

/// LengthOfArrayLike with the symbol abrupt (B1d): a symbol `length`
/// throws via ToNumber. Ok(length) or Err(error value to raise).
fn checked_array_length(gc: &mut SemiSpace, vm: &mut Vm, this: Value) -> Result<u32, Value> {
    // B1f: LengthOfArrayLike reads through the proto chain (A5_T1 sets
    // Object.prototype.length) — a symbol anywhere on it throws.
    let mut current = this;
    for _ in 0..crate::vm::MAX_PROTOTYPE_DEPTH {
        let Some(ptr) = current.heap_ptr() else {
            break;
        };
        if unsafe { (*(ptr as *const GcHeader)).tag() } != TAG_OBJECT {
            break;
        }
        let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
        if let Some(slot) = shape.lookup(&PropertyKey::from_string("length")) {
            let lv = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
            if lv.is_symbol() {
                return Err(crate::errors::error_object(
                    gc,
                    &vm.error_protos,
                    crate::errors::ErrorKind::TypeError,
                    "Cannot convert a Symbol value to a number",
                ));
            }
            break;
        }
        let proto = unsafe { JSObject::prototype(ptr as *mut JSObject) };
        if proto.is_null() {
            break;
        }
        current = Value::from_heap_ptr(proto);
    }
    Ok(crate::vm::array_like_length(this).unwrap_or(0))
}
/// Array.prototype.toSpliced(start, deleteCount, ...items) — non-mutating
/// splice (§23.1.3.33 lite): copies the receiver, applies splice index
/// math on the copy, returns it. Holes copy as undefined (dense model).
pub fn array_to_spliced(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let len = match checked_array_length(gc, vm, this) {
        Ok(len) => len as i64,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    let start = match clamp_index_throwing(gc, vm, args.first().copied(), len, 0) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    let delete = if args.len() < 2 {
        len - start
    } else if args[1].is_symbol() {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert a Symbol value to a number",
        ));
        return Value::undefined();
    } else {
        let n = to_integer_or_infinity(args[1]);
        (n.max(0.0) as i64).min(len - start)
    };
    let items: &[Value] = if args.len() > 2 { &args[2..] } else { &[] };
    let mut out: Vec<Value> = Vec::with_capacity(len as usize + items.len());
    for i in 0..start as usize {
        out.push(crate::vm::array_like_index(this, i as u32).unwrap_or(Value::undefined()));
    }
    out.extend_from_slice(items);
    for i in (start + delete) as usize..len as usize {
        out.push(crate::vm::array_like_index(this, i as u32).unwrap_or(Value::undefined()));
    }
    build_array(gc, &out, vm)
}

/// Array.prototype.with(index, value) — copy with one element replaced
/// (§23.1.3.36 lite). Negative index counts from the end; out of range
/// (incl. -0?) throws RangeError.
pub fn array_with(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let len = match checked_array_length(gc, vm, this) {
        Ok(len) => len as usize,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    // B1e (unmasked by the Exp fix): ArrayCreate(len) throws RangeError for
    // len > 2^32-1, before any element Get (with/length-exceeding-...).
    if let Some(lptr) = this.heap_ptr() {
        if unsafe { (*(lptr as *const GcHeader)).tag() } == TAG_OBJECT {
            let shape = unsafe { JSObject::shape_ptr(lptr as *mut JSObject) };
            if let Some(slot) = shape.lookup(&PropertyKey::from_string("length")) {
                let lv = unsafe { JSObject::get_slot(lptr as *mut JSObject, slot) };
                let n = lv
                    .as_smi()
                    .map(|v| v as f64)
                    .or_else(|| lv.as_float64())
                    .unwrap_or(f64::NAN);
                if n > 4_294_967_295.0 {
                    vm.set_pending_exception(crate::errors::error_object(
                        gc,
                        &vm.error_protos,
                        crate::errors::ErrorKind::RangeError,
                        "Invalid array length",
                    ));
                    return Value::undefined();
                }
            }
        }
    }
    let value = args.get(1).copied().unwrap_or(Value::undefined());
    let rel = args.first().copied().unwrap_or(Value::undefined());
    if rel.is_symbol() {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert a Symbol value to a number",
        ));
        return Value::undefined();
    }
    let idx = to_integer_or_infinity(rel) as i64;
    let actual = if idx < 0 { len as i64 + idx } else { idx };
    if actual < 0 || actual >= len as i64 {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::RangeError,
            "Invalid array index",
        ));
        return Value::undefined();
    }
    let mut elems: Vec<Value> = (0..len)
        .map(|i| crate::vm::array_like_index(this, i as u32).unwrap_or(Value::undefined()))
        .collect();
    elems[actual as usize] = value;
    build_array(gc, &elems, vm)
}

/// Array.prototype.push(value) — pushes value to the array, returns new length.
/// Auto-grows the array if capacity is exhausted and updates VM references.
/// ================= B1f-1: stack/queue mutators audit =================
///
/// Array.prototype.push/pop/shift/unshift/reverse (§23.1.3.22/.23/.26/.27/
/// .37): generic over heap receivers (dense fast discipline + plain-object
/// HasProperty/Get/Set/Delete), hole-preserving, u64 lengths with ToLength
/// clamping, 2^53-1 overflow TypeErrors, spec-order length writes, exact
/// length return values. Sync data-path: builtin getters/setters run
/// inline; JS accessors are a documented sync-gap (B1f-6 runs them through
/// the machine); ToObject boxing is absent (primitives take the
/// discarded-box path, strings the exotic-reject path).
///
/// LengthOfArrayLike as (clamped u64, raw f64) — direct symbol lengths
/// throw via checked_array_length; ToLength clamps to 2^53-1 (never
/// iterates past it).
fn mutator_length(gc: &mut SemiSpace, vm: &mut Vm, this: Value) -> Result<(u64, f64), Value> {
    checked_array_length(gc, vm, this)?;
    let raw = length_to_number(gc, vm, length_walk_value(this))?;
    Ok((to_length_clamp(raw), raw))
}

/// First length slot value up the chain (or undefined): dense length,
/// own-or-inherited data slots, UTF-16 length for strings. Pure sync walk
/// (accessor lengths surface as their pair value for the converter).
fn length_walk_value(this: Value) -> Value {
    let Some(ptr) = this.heap_ptr() else {
        return Value::undefined();
    };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag == TAG_ARRAY {
        let len = unsafe { RuneArray::length(ptr as *mut RuneArray) };
        return length_value(len as u64);
    }
    if tag == TAG_STRING {
        let len = unsafe {
            HeapString::to_string(ptr as *mut HeapString)
                .encode_utf16()
                .count()
        };
        return length_value(len as u64);
    }
    if tag != TAG_OBJECT {
        return Value::undefined();
    }
    let mut current = this;
    for _ in 0..crate::vm::MAX_PROTOTYPE_DEPTH {
        let Some(cptr) = current.heap_ptr() else {
            return Value::undefined();
        };
        if unsafe { (*(cptr as *const GcHeader)).tag() } != TAG_OBJECT {
            return Value::undefined();
        }
        let shape = unsafe { JSObject::shape_ptr(cptr as *mut JSObject) };
        if let Some(slot) = shape.lookup(&PropertyKey::from_string("length")) {
            return unsafe { JSObject::get_slot(cptr as *mut JSObject, slot) };
        }
        let proto = unsafe { JSObject::prototype(cptr as *mut JSObject) };
        if proto.is_null() {
            return Value::undefined();
        }
        current = Value::from_heap_ptr(proto);
    }
    Value::undefined()
}

/// ToNumber for length values with builtin-inline coercion (B1f): Smi/float
/// direct; symbols throw; strings parse; heap objects try builtin valueOf
/// then builtin toString inline (String objects coerce — reverse A2_T3 uses
/// `new String("9")`); JS-driven methods read as 0 (B1f-6 runs them through
/// the machine).
fn length_to_number(gc: &mut SemiSpace, vm: &mut Vm, v: Value) -> Result<f64, Value> {
    if let Some(n) = v.as_smi().map(|v| v as f64).or_else(|| v.as_float64()) {
        return Ok(n);
    }
    // B1f-3: ToNumber(true) is 1 (boolean lengths: {length: true} → 1).
    if let Some(b) = v.to_boolean() {
        return Ok(if b { 1.0 } else { 0.0 });
    }
    if v.is_symbol() {
        return Err(sort_type_error(
            gc,
            vm,
            "Cannot convert a Symbol value to a number",
        ));
    }
    if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            let t = s.trim();
            if t.is_empty() {
                return Ok(0.0);
            }
            return Ok(t.parse::<f64>().unwrap_or(f64::NAN));
        }
        // String objects coerce by their inner string (own valueOf
        // overrides are a documented gap — B1f-6 dispatches methods).
        if tag == TAG_STRING_OBJ {
            let str_ptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
            let s = unsafe { HeapString::to_string(str_ptr as *mut HeapString) };
            let t = s.trim();
            if t.is_empty() {
                return Ok(0.0);
            }
            return Ok(t.parse::<f64>().unwrap_or(f64::NAN));
        }
        if tag == TAG_OBJECT {
            for name in ["valueOf", "toString"] {
                let key = PropertyKey::from_string(name);
                let method = toprim_find_method(v, &key);
                let Some(m) = method else { continue };
                let Some(smi) = m.as_smi() else { continue };
                if smi >= 0 {
                    continue;
                }
                let id = ((-smi) as usize) - 1;
                if id >= vm.builtins.len() {
                    continue;
                }
                let result = (vm.builtins[id].func)(gc, v, &[], vm);
                if let Some(exc) = vm.pending_exception.take() {
                    return Err(exc);
                }
                if toprim_is_primitive(result) {
                    if result.is_symbol() {
                        return Err(sort_type_error(
                            gc,
                            vm,
                            "Cannot convert a Symbol value to a number",
                        ));
                    }
                    return Ok(crate::vm::to_number(result));
                }
                // Non-primitive: fall through to the next method.
            }
        }
    }
    Ok(0.0)
}

/// ToLength clamp (§7.1.22 lite): NaN/negative → 0, above 2^53-1 saturates.
fn to_length_clamp(n: f64) -> u64 {
    if n.is_nan() || n <= 0.0 {
        0
    } else {
        n.min(9_007_199_254_740_991.0) as u64
    }
}

/// Length as a return value: Smi inside the i31 range, float64 above
/// (Value::smi debug-asserts i31 — lengths near 2^32 must not wrap).
pub(crate) fn length_value(len: u64) -> Value {
    if len < (1 << 30) {
        Value::smi(len as i32)
    } else {
        Value::from_float64(len as f64)
    }
}

/// Canonical index key for walks (B1f): Smi fast path, HeapString past the
/// i31 range (huge walks only — normal paths never allocate here).
fn index_key(gc: &mut SemiSpace, k: u64) -> Value {
    if k < (1 << 30) {
        Value::smi(k as i32)
    } else {
        Value::from_heap_ptr(HeapString::allocate(gc, &k.to_string()) as *mut u8)
    }
}

/// What a mutator write faces at an index (shared with the sort machine).
pub(crate) enum StoreTarget {
    /// Plain data slot (or absent — create it).
    Data,
    /// Builtin setter: run inline.
    SetterBuiltin(Value),
    /// JS setter: sync-gap (B1f-6 runs it through the machine).
    SetterJs(Value),
    /// Getter-only or invalid setter: strict Set rejects.
    GetterOnly,
    SetterInvalid,
}

/// Classify a write target: own-or-inherited accessor scan (dense overlay
/// for arrays, shape slots for objects, then the tag-guarded proto chain).
pub(crate) fn classify_store(gc: &mut SemiSpace, obj: Value, idx: usize) -> StoreTarget {
    let Some(pair) = sort_find_accessor(gc, obj, idx as u64) else {
        return StoreTarget::Data;
    };
    let Some(aptr) = pair.heap_ptr() else {
        return StoreTarget::GetterOnly;
    };
    let setter = unsafe { rune_core::accessor::AccessorPair::setter(aptr) };
    if setter.is_undefined() || setter.is_null() {
        return StoreTarget::GetterOnly;
    }
    if let Some(smi) = setter.as_smi() {
        if smi < 0 {
            return StoreTarget::SetterBuiltin(setter);
        }
        return StoreTarget::SetterInvalid;
    }
    if setter
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_FUNC)
    {
        return StoreTarget::SetterJs(setter);
    }
    StoreTarget::SetterInvalid
}

/// Own data value for strict-Set SameValue checks (B1f): dense elements
/// (holes read as absent) and own object slots only — never the proto
/// chain (a proto match must not excuse a failed own store).
fn own_data_value(obj: Value, key: Value, idx: u64) -> Option<Value> {
    let ptr = obj.heap_ptr()?;
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag == TAG_ARRAY {
        let len = unsafe { RuneArray::length(ptr as *mut RuneArray) } as u64;
        let cap = unsafe { RuneArray::capacity(ptr as *mut RuneArray) } as u64;
        if idx < len && idx < cap {
            let elem = unsafe { RuneArray::get_element(ptr as *mut RuneArray, idx as usize) };
            if elem != Value::empty_sentinel() {
                return Some(elem);
            }
        }
        return None;
    }
    if tag != TAG_OBJECT {
        return None;
    }
    let prop = crate::vm::value_to_prop_key(key)?;
    let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
    shape
        .lookup(&prop)
        .map(|slot| unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) })
}

/// Sync spec-Set(throw=true) on an index for mutators (B1f-1). Builtin
/// setters run inline; getter-only/read-only reject with TypeError; JS
/// setters are skipped (documented sync-gap — B1f-6 dispatches them).
/// Never calls handle_throw (setup-safe like sort_store_one).
fn mutator_store(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    obj: &mut Value,
    idx: u64,
    val: Value,
) -> Result<(), Value> {
    let key = index_key(gc, idx);
    // The key allocation may have GC-moved the receiver; every allocating
    // step below refreshes the same way (growth sets forwarding, GC sets
    // forwarding — refresh_value follows both).
    *obj = refresh_value(*obj);
    // String exotics reject every write (length behaves getter-only).
    if let Some(ptr) = obj.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_STRING {
            return Err(sort_type_error(
                gc,
                vm,
                "Cannot assign to read only property of a string",
            ));
        }
    }
    match classify_store(gc, *obj, idx.min(usize::MAX as u64) as usize) {
        StoreTarget::Data => {
            if crate::vm::do_store_property(*obj, key, val, gc, vm) {
                *obj = refresh_value(*obj);
                Ok(())
            } else {
                *obj = refresh_value(*obj);
                // SameValue stores succeed silently (ValidateAndApply).
                if own_data_value(*obj, key, idx).is_some_and(|cur| same_value(cur, val)) {
                    Ok(())
                } else {
                    Err(sort_type_error(
                        gc,
                        vm,
                        "Cannot assign to read-only property",
                    ))
                }
            }
        }
        StoreTarget::SetterBuiltin(setter) => {
            let id = ((-setter.as_smi().unwrap()) as usize) - 1;
            if id < vm.builtins.len() {
                (vm.builtins[id].func)(gc, *obj, &[val], vm);
                *obj = refresh_value(*obj);
                if let Some(exc) = vm.pending_exception.take() {
                    return Err(exc);
                }
                return Ok(());
            }
            Err(sort_type_error(gc, vm, "setter is not a function"))
        }
        // B1f-6 runs user setters through the machine; the sync audit
        // leaves the slot untouched rather than corrupting it.
        StoreTarget::SetterJs(_) => Ok(()),
        StoreTarget::GetterOnly => Err(sort_type_error(
            gc,
            vm,
            "Cannot set property with only a getter",
        )),
        StoreTarget::SetterInvalid => Err(sort_type_error(gc, vm, "setter is not a function")),
    }
}

/// Sync element read for mutators (B1f-1): full Get (proto consult, holes
/// included); builtin getters run inline; JS getters read as undefined
/// (documented sync-gap — B1f-6 dispatches them).
fn mutator_read(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    obj: &mut Value,
    idx: u64,
) -> Result<Value, Value> {
    let raw = crate::vm::load_property_recursive(*obj, index_key(gc, idx), None, gc);
    // The key allocation may have GC-moved the receiver.
    *obj = refresh_value(*obj);
    let Some(aptr) = raw.heap_ptr() else {
        return Ok(raw);
    };
    if unsafe { (*(aptr as *const GcHeader)).tag() } != TAG_ACCESSOR {
        return Ok(raw);
    }
    let getter = unsafe { rune_core::accessor::AccessorPair::getter(aptr) };
    if getter.is_undefined() || getter.is_null() {
        return Ok(Value::undefined());
    }
    if let Some(smi) = getter.as_smi() {
        if smi < 0 {
            let id = ((-smi) as usize) - 1;
            if id < vm.builtins.len() {
                let result = (vm.builtins[id].func)(gc, *obj, &[], vm);
                // The inline call may have allocated or grown.
                *obj = refresh_value(*obj);
                if let Some(exc) = vm.pending_exception.take() {
                    return Err(exc);
                }
                return Ok(result);
            }
        }
        return Ok(Value::undefined());
    }
    // JS getter: B1f-6 dispatches it; the sync audit reads undefined.
    Ok(Value::undefined())
}

/// Ensure dense capacity covers `need` (B1f): materializes length-extended
/// tails as holes up front so the move loops never grow mid-walk (a grow
/// moves the array and stales the local receiver Value — pre-growing once
/// keeps every subsequent raw/do_store write sound). Refuses absurd sizes;
/// leftovers grow per-index exactly like before.
fn ensure_dense_capacity(gc: &mut SemiSpace, vm: &mut Vm, obj: &mut Value, need: u64) {
    let Some(ptr) = obj.heap_ptr() else {
        return;
    };
    if unsafe { (*(ptr as *const GcHeader)).tag() } != TAG_ARRAY {
        return;
    }
    // B1f: never materialize absurd tails (A3 pushes onto length 2^32-1;
    // unbounded growth OOMs the 16 MiB semispace). The 1M bound mirrors
    // the sparse-construction threshold; beyond it slots stay holes and
    // per-index growth takes over exactly like before this helper.
    if need > 1_000_000 {
        return;
    }
    let mut arr = ptr as *mut RuneArray;
    while (unsafe { RuneArray::capacity(arr) } as u64) < need {
        unsafe {
            arr = grow_dense_array(gc, vm, arr);
        }
    }
    // A grow moves the array: refresh the caller's receiver (the old
    // address is abandoned — every later read/write must use the live
    // pointer). update_heap_reference inside grow_dense_array fixed the
    // VM roots; only this Rust-local copy needs rewriting.
    *obj = Value::from_heap_ptr(arr as *mut u8);
}

/// Spec Set(obj, "length") for mutators (B1f-1 + B1f-2): dense sets the magic
/// length (past 2^32-1 throws RangeError — the array-exotic length
/// invariant, §23.1.4.1); plain objects set the slot when present; strings
/// always throw (getter-only model); non-string primitives are
/// discarded-box no-ops. B1f-2: an accessor-pair length dispatches the setter
/// (builtin inline) and throws getter-only/invalid (splice A6.1_T3); a JS
/// setter is quiet-skipped (B1f-6 runs it through the machine —
/// set_length_no_args stays failing). Non-writable/frozen length states cannot
/// be constructed yet (B1f-5), so no attribute checks run here.
fn set_length_checked(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    obj: &mut Value,
    len: u64,
) -> Result<(), Value> {
    let Some(ptr) = obj.heap_ptr() else {
        // Discarded-box primitive: nothing to store into.
        return Ok(());
    };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag == TAG_ARRAY {
        if len > 4_294_967_295 {
            // push's element Set landed first (named slot); the length
            // update then throws, exactly like V8 (A3).
            return Err(sort_range_error(gc, vm, "Invalid array length"));
        }
        // Dense lengths fit u32 by construction (growth paths that could
        // exceed it cannot materialize that many elements).
        unsafe { RuneArray::set_length(ptr as *mut RuneArray, len as u32) };
        return Ok(());
    }
    if tag == TAG_STRING {
        return Err(sort_type_error(
            gc,
            vm,
            "Cannot assign to read only property 'length' of a string",
        ));
    }
    if tag != TAG_OBJECT {
        return Ok(());
    }
    let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
    if let Some(slot) = shape.lookup(&PropertyKey::from_string("length")) {
        let cur = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
        if let Some(aptr) = cur.heap_ptr() {
            if unsafe { (*(aptr as *const GcHeader)).tag() } == TAG_ACCESSOR {
                let setter = unsafe { rune_core::accessor::AccessorPair::setter(aptr) };
                if setter.is_undefined() || setter.is_null() {
                    return Err(sort_type_error(
                        gc,
                        vm,
                        "Cannot set property length which has only a getter",
                    ));
                }
                if let Some(smi) = setter.as_smi() {
                    if smi < 0 {
                        let id = ((-smi) as usize) - 1;
                        if id < vm.builtins.len() {
                            (vm.builtins[id].func)(gc, *obj, &[length_value(len)], vm);
                            *obj = refresh_value(*obj);
                            if let Some(exc) = vm.pending_exception.take() {
                                return Err(exc);
                            }
                            return Ok(());
                        }
                        return Err(sort_type_error(gc, vm, "setter is not a function"));
                    }
                    return Err(sort_type_error(gc, vm, "setter is not a function"));
                }
                // JS setter: B1f-6 dispatches it; the sync audit leaves the
                // slot untouched rather than corrupting it.
                *obj = refresh_value(*obj);
                return Ok(());
            }
        }
        unsafe { JSObject::set_slot(ptr as *mut JSObject, slot, length_value(len)) };
        return Ok(());
    }
    // Absent length (e.g. pop/shift on {}) is CREATED by the spec Set.
    let key = Value::from_heap_ptr(HeapString::allocate(gc, "length") as *mut u8);
    // The key allocation may have GC-moved the receiver.
    *obj = refresh_value(*obj);
    crate::vm::do_store_property(*obj, key, length_value(len), gc, vm);
    *obj = refresh_value(*obj);
    Ok(())
}

/// True when a move span provably performs no observable work (B1f):
/// no indexed entries (data or accessor, own or inherited) anywhere in
/// [lo, hi). Lets huge sparse walks (the clamps tests at 2^53 lengths)
/// complete instantly instead of iterating billions of absent indices.
/// Sound: absent→absent deletes are no-ops, and with no accessors no user
/// code can observe or perturb the walk. Dense arrays always move data
/// (their indices are present unless holes — holes still move), so callers
/// only consult this for plain objects.
fn move_range_is_quiet(obj: Value, lo: u64, hi: u64) -> bool {
    let mut current = obj;
    for _ in 0..crate::vm::MAX_PROTOTYPE_DEPTH {
        let Some(cptr) = current.heap_ptr() else {
            return true;
        };
        if unsafe { (*(cptr as *const GcHeader)).tag() } != TAG_OBJECT {
            // Conservative: exotic links (e.g. dense-array protos serving
            // indices) may observe the walk.
            return false;
        }
        let shape = unsafe { JSObject::shape_ptr(cptr as *mut JSObject) };
        let count = unsafe { JSObject::slot_count(cptr as *mut JSObject) };
        for i in 0..count {
            let Some(name) = shape.key_name_at(i) else {
                continue;
            };
            // Any indexed entry in span (data or accessor) is observable:
            // canonical indices AND huge named-overflow indices (B1f-1 key
            // model: index_key emits `k.to_string()`, so exact string equality
            // is the membership test — B1f-2 splice A3_T1 needs "4294967295"
            // to count, which canonical_index_name excludes).
            if sparse_key_in_range(name, lo, hi).is_some() {
                return false;
            }
            // Any accessor anywhere could serve an index in span via the
            // pair path... pairs serve only their own key, which the
            // numeric test above already covers — but a pair under a
            // NON-index name is harmless. Only index-named pairs matter,
            // already handled. (No extra check needed.)
        }
        let proto = unsafe { JSObject::prototype(cptr as *mut JSObject) };
        if proto.is_null() {
            return true;
        }
        current = Value::from_heap_ptr(proto);
    }
    false
}

/// DeletePropertyOrThrow on an index, routing through the shared sort rule
/// (dense punches holes, objects check configurability).
fn mutator_delete(gc: &mut SemiSpace, vm: &Vm, obj: &mut Value, idx: u64) -> Result<(), Value> {
    // Deletes never allocate, but refresh anyway: a later grow relies on a
    // live receiver and this keeps the discipline total.
    let out = sort_delete_one(gc, vm, *obj, idx);
    *obj = refresh_value(*obj);
    out
}

/// Array.prototype.push(...items) — appends all args, returns the new
/// length (§23.1.3.23). Generic over heap receivers; strings reject every
/// write; non-string primitives take the discarded-box path.
pub fn array_push(gc: &mut SemiSpace, mut this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let (mut len, _) = match mutator_length(gc, vm, this) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    let count = args.len() as u64;
    if len + count > 9_007_199_254_740_991 {
        vm.set_pending_exception(sort_type_error(
            gc,
            vm,
            "Pushing elements exceeds the maximum array length",
        ));
        return Value::undefined();
    }
    let is_prim = this.heap_ptr().is_none();
    let is_string = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_STRING);
    if !is_prim && !is_string {
        ensure_dense_capacity(gc, vm, &mut this, len + count);
    }
    for (k, &item) in args.iter().enumerate() {
        let at = len + k as u64;
        if is_prim && !is_string {
            // Discarded-box primitive: the store succeeds into the void.
            continue;
        }
        if let Err(e) = mutator_store(gc, vm, &mut this, at, refresh_value(item)) {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    }
    len += count;
    if !is_prim {
        if let Err(e) = set_length_checked(gc, vm, &mut this, len) {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    }
    length_value(len)
}
pub fn array_pop(gc: &mut SemiSpace, mut this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let (len, _) = match mutator_length(gc, vm, this) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    if len == 0 {
        if this.heap_ptr().is_some() {
            if let Err(e) = set_length_checked(gc, vm, &mut this, 0) {
                vm.set_pending_exception(e);
                return Value::undefined();
            }
        }
        return Value::undefined();
    }
    if this.heap_ptr().is_none() {
        return Value::undefined();
    }
    let last = len - 1;
    let element = match mutator_read(gc, vm, &mut this, last) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    if let Err(e) = mutator_delete(gc, vm, &mut this, last) {
        vm.set_pending_exception(e);
        return Value::undefined();
    }
    if let Err(e) = set_length_checked(gc, vm, &mut this, last) {
        vm.set_pending_exception(e);
        return Value::undefined();
    }
    element
}
pub fn array_shift(gc: &mut SemiSpace, mut this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let (len, _) = match mutator_length(gc, vm, this) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    if len == 0 {
        if this.heap_ptr().is_some() {
            if let Err(e) = set_length_checked(gc, vm, &mut this, 0) {
                vm.set_pending_exception(e);
                return Value::undefined();
            }
        }
        return Value::undefined();
    }
    if this.heap_ptr().is_none() {
        return Value::undefined();
    }
    let first = match mutator_read(gc, vm, &mut this, 0) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    // B1f: huge sparse walks with no indexed entries anywhere are
    // provably silent — skip them (the clamps tests iterate at 2^53).
    // Dense arrays always move data; only plain objects consult quiet.
    let is_dense = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_ARRAY);
    if is_dense {
        ensure_dense_capacity(gc, vm, &mut this, len);
    }
    let mut k: u64 = 1;
    if !is_dense && move_range_is_quiet(this, 0, len) {
        k = len;
    }
    while k < len {
        let from_present = crate::vm::has_property(this, index_key(gc, k), None);
        if from_present {
            let v = match mutator_read(gc, vm, &mut this, k) {
                Ok(v) => v,
                Err(e) => {
                    vm.set_pending_exception(e);
                    return Value::undefined();
                }
            };
            if let Err(e) = mutator_store(gc, vm, &mut this, k - 1, v) {
                vm.set_pending_exception(e);
                return Value::undefined();
            }
        } else if let Err(e) = mutator_delete(gc, vm, &mut this, k - 1) {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
        k += 1;
    }
    if let Err(e) = mutator_delete(gc, vm, &mut this, len - 1) {
        vm.set_pending_exception(e);
        return Value::undefined();
    }
    if let Err(e) = set_length_checked(gc, vm, &mut this, len - 1) {
        vm.set_pending_exception(e);
        return Value::undefined();
    }
    first
}
pub fn array_unshift(gc: &mut SemiSpace, mut this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let (len, _) = match mutator_length(gc, vm, this) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    let count = args.len() as u64;
    // u64 math throughout: the old u32 `length + arg_count` wrapped (the
    // length-near-integer-limit ENGINE PANIC).
    if count > 0 && len + count > 9_007_199_254_740_991 {
        vm.set_pending_exception(sort_type_error(
            gc,
            vm,
            "Unshifting elements exceeds the maximum array length",
        ));
        return Value::undefined();
    }
    let is_prim = this.heap_ptr().is_none();
    let is_dense = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_ARRAY);
    if !is_prim && is_dense {
        ensure_dense_capacity(gc, vm, &mut this, len + count);
    }
    if !is_prim {
        // Backward slide: from k-1 to k+count-1 (spec order). Quiet
        // huge sparse spans skip (see shift).
        let mut k = len;
        if !is_dense && move_range_is_quiet(this, 0, len + count) {
            k = 0;
        }
        while k > 0 {
            let from = k - 1;
            let to = k + count - 1;
            if crate::vm::has_property(this, index_key(gc, from), None) {
                let v = match mutator_read(gc, vm, &mut this, from) {
                    Ok(v) => v,
                    Err(e) => {
                        vm.set_pending_exception(e);
                        return Value::undefined();
                    }
                };
                if let Err(e) = mutator_store(gc, vm, &mut this, to, v) {
                    vm.set_pending_exception(e);
                    return Value::undefined();
                }
            } else if let Err(e) = mutator_delete(gc, vm, &mut this, to) {
                vm.set_pending_exception(e);
                return Value::undefined();
            }
            k -= 1;
        }
        for (j, &item) in args.iter().enumerate() {
            if let Err(e) = mutator_store(gc, vm, &mut this, j as u64, refresh_value(item)) {
                vm.set_pending_exception(e);
                return Value::undefined();
            }
        }
        if let Err(e) = set_length_checked(gc, vm, &mut this, len + count) {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    }
    length_value(len + count)
}
pub fn array_reverse(gc: &mut SemiSpace, mut this: Value, _args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let (len, _) = match mutator_length(gc, vm, this) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    if len <= 1 || this.heap_ptr().is_none() {
        return this;
    }
    let is_dense = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_ARRAY);
    // B1f: a span with no indexed entries anywhere reverses to itself
    // (all four swap cases are no-ops) — required for the 2^53 walks.
    if !is_dense && move_range_is_quiet(this, 0, len) {
        return this;
    }
    if is_dense {
        ensure_dense_capacity(gc, vm, &mut this, len);
    }
    let middle = len / 2;
    let mut lower: u64 = 0;
    while lower != middle {
        let upper = len - lower - 1;
        let lower_present = crate::vm::has_property(this, index_key(gc, lower), None);
        let lower_val = if lower_present {
            match mutator_read(gc, vm, &mut this, lower) {
                Ok(v) => v,
                Err(e) => {
                    vm.set_pending_exception(e);
                    return Value::undefined();
                }
            }
        } else {
            Value::undefined()
        };
        let upper_present = crate::vm::has_property(this, index_key(gc, upper), None);
        let upper_val = if upper_present {
            match mutator_read(gc, vm, &mut this, upper) {
                Ok(v) => v,
                Err(e) => {
                    vm.set_pending_exception(e);
                    return Value::undefined();
                }
            }
        } else {
            Value::undefined()
        };
        let step: Result<(), Value> = match (lower_present, upper_present) {
            (true, true) => mutator_store(gc, vm, &mut this, lower, upper_val)
                .and_then(|()| mutator_store(gc, vm, &mut this, upper, lower_val)),
            (false, true) => mutator_store(gc, vm, &mut this, lower, upper_val)
                .and_then(|()| mutator_delete(gc, vm, &mut this, upper)),
            (true, false) => mutator_delete(gc, vm, &mut this, lower)
                .and_then(|()| mutator_store(gc, vm, &mut this, upper, lower_val)),
            (false, false) => Ok(()),
        };
        if let Err(e) = step {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
        lower += 1;
    }
    this
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
        let err = make_error(
            gc,
            &vm.error_protos,
            "TypeError: Cannot convert undefined or null to object",
        );
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

/// ToIntegerOrInfinity lite (B1b: shared with vm.rs search stepping).
/// Handles undefined/null/bool/Smi/float directly; numeric strings
/// (incl. exponents and Infinity) parse per ToNumber-lite; anything else
/// (objects with valueOf, hex, unparseable) yields 0 — documented gap
/// shared with the string repeat/pad paths that also use this helper.
pub(crate) fn to_integer_or_infinity(v: Value) -> f64 {
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
    // Numeric strings (fromIndex "2"/"3E0", repeat counts, pad lengths).
    if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            let t = s.trim();
            if t.is_empty() {
                return 0.0;
            }
            if t.eq_ignore_ascii_case("infinity") || t == "+Infinity" {
                return f64::INFINITY;
            }
            if t == "-Infinity" {
                return f64::NEG_INFINITY;
            }
            if let Ok(n) = t.parse::<f64>() {
                if n.is_nan() {
                    return 0.0;
                }
                return n.trunc();
            }
            return 0.0;
        }
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
        let err = make_error(gc, &vm.error_protos, "RangeError: Invalid count value");
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
        let err = make_error(gc, &vm.error_protos, "RangeError: Invalid string length");
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
        let err = make_error(gc, &vm.error_protos, "RangeError: Invalid string length");
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
            // Parse and execute regex (flags-aware for i/m/s)
            match rune_regex::parse_regex(&pattern) {
                Ok(expr) => {
                    let nfa = rune_regex::nfa::compile(&expr);
                    let pike_vm = rune_regex::pikevm::PikeVm::new();
                    let flags = unsafe { RegExp::flags(ptr) };
                    if let Some(m) = pike_vm.exec_with_flags(&nfa, &s, 0, flags) {
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
                    let flags = unsafe { RegExp::flags(ptr) };
                    if is_fn_replacement {
                        // Callable replacement: state machine in the Return
                        // handler re-invokes fn per match (spec §22.1.3.20
                        // @@replace path: fn(match, ...captures, position, string)).
                        let fn_val = replacement_fn.unwrap();
                        match pike_vm.exec_with_flags(&nfa, &s, 0, flags) {
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
                                    regex_flags: flags,
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
                    while let Some(m) = pike_vm.exec_with_flags(&nfa, &s, last_end, flags) {
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
                regex_flags: 0,
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
    let flags = unsafe { RegExp::flags(regexp_ptr) };
    match rune_regex::parse_regex(&pattern) {
        Ok(expr) => {
            let nfa = rune_regex::nfa::compile(&expr);
            let pike_vm = rune_regex::pikevm::PikeVm::new();
            pike_vm
                .exec_with_flags(&nfa, input, start_pos, flags)
                .map(|m| m.groups)
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

/// Coerce a Value to f64 for the newer Math methods: Smi/float direct,
/// numeric strings parsed (§7.1.4 ToNumber-lite), else NaN.
fn math_to_f64(v: Value) -> f64 {
    if let Some(smi) = v.as_smi() {
        return smi as f64;
    }
    if let Some(f) = v.as_float64() {
        return f;
    }
    if let Some(b) = v.to_boolean() {
        return if b { 1.0 } else { 0.0 };
    }
    if let Some(ptr) = v.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING {
            let s = unsafe { HeapString::to_string(ptr as *mut HeapString) };
            let t = s.trim();
            if t.is_empty() {
                return 0.0;
            }
            return t.parse::<f64>().unwrap_or(f64::NAN);
        }
    }
    if v.is_undefined() {
        return f64::NAN;
    }
    if v.is_null() {
        return 0.0;
    }
    f64::NAN
}

/// Wrap an f64 builtin result: integral finite values within i32 become
/// Smis (same convention as math_op_unary), everything else stays float64.
fn math_result(r: f64) -> Value {
    if r.fract() == 0.0 && r.is_finite() {
        let i = r as i32;
        if i as f64 == r {
            return Value::smi(i);
        }
    }
    Value::from_float64(r)
}

/// Math.round — §21.3.2.29: closest integral Number, ties toward +∞.
/// Math.round(-3.5) === -3 (NOT away-from-zero).
pub fn math_round(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    // Step 2: non-finite or already integral → n unchanged
    if !x.is_finite() || x == x.trunc() {
        return Value::from_float64(x);
    }
    // Step 3: 0 < n < 0.5 → +0
    if x > 0.0 && x < 0.5 {
        return Value::smi(0);
    }
    // Step 4: -0.5 <= n < -0 → -0
    if (-0.5..0.0).contains(&x) && x < 0.0 {
        return Value::from_float64(-0.0);
    }
    // Step 5: tie toward +∞. floor(x+0.5) is exact here because the
    // 0.49999999999999994 hazard is excluded by step 3 and |fract| >= 0.5
    // boundaries are handled; large magnitudes returned integral by step 2.
    let r = (x + 0.5).floor();
    if r.fract() == 0.0 && r.is_finite() {
        let i = r as i32;
        if i as f64 == r {
            return Value::smi(i);
        }
    }
    Value::from_float64(r)
}

/// Math.trunc — §21.3.2.37: integral part toward zero, ±0 preserved.
pub fn math_trunc(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    if !x.is_finite() || x == 0.0 {
        return Value::from_float64(x);
    }
    let r = x.trunc();
    if r == 0.0 {
        // Preserve the sign of zero for inputs in (-1, 1)
        return Value::from_float64(if x < 0.0 { -0.0 } else { 0.0 });
    }
    if r.fract() == 0.0 && r.is_finite() {
        let i = r as i32;
        if i as f64 == r {
            return Value::smi(i);
        }
    }
    Value::from_float64(r)
}

/// Math.sign — §21.3.2.30: NaN/±0 → n unchanged, negative → -1, else 1.
pub fn math_sign(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    if x.is_nan() || x == 0.0 {
        return Value::from_float64(x);
    }
    Value::smi(if x < 0.0 { -1 } else { 1 })
}

/// Math.hypot — §21.3.2.19: coerce all first, ±Inf wins over NaN, all-zero → +0.
pub fn math_hypot(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let coerced: Vec<f64> = args.iter().map(|&v| math_to_f64(v)).collect();
    // Step 3: any ±Infinity → +Infinity (checked before NaN)
    if coerced.iter().any(|n| n.is_infinite()) {
        return Value::from_float64(f64::INFINITY);
    }
    // Step 5: any NaN → NaN
    if coerced.iter().any(|n| n.is_nan()) {
        return Value::from_float64(f64::NAN);
    }
    // Step 6: all zeros → +0
    let sum_sq: f64 = coerced.iter().map(|n| n * n).sum();
    math_result(sum_sq.sqrt())
}

/// Math.clz32 — §21.3.2.11: leading zero bits of ToUint32(x).
pub fn math_clz32(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    let n = if x.is_finite() {
        x.trunc() as i64 as u32
    } else {
        0u32
    };
    Value::smi(n.leading_zeros() as i32)
}

/// Math.imul — §21.3.2.20: 32-bit integer multiplication.
pub fn math_imul(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let a = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    let b = math_to_f64(args.get(1).copied().unwrap_or(Value::undefined()));
    let ua = if a.is_finite() {
        a.trunc() as i64 as u32
    } else {
        0u32
    };
    let ub = if b.is_finite() {
        b.trunc() as i64 as u32
    } else {
        0u32
    };
    let product = ua.wrapping_mul(ub);
    Value::smi(product as i32)
}

/// Math.cbrt / log family / exp / trig — direct f64 ops via the shared helper.
pub fn math_cbrt(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    math_result(x.cbrt())
}

pub fn math_log(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    math_result(x.ln())
}

pub fn math_log2(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    math_result(x.log2())
}

pub fn math_log10(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    math_result(x.log10())
}

pub fn math_exp(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    math_result(x.exp())
}

pub fn math_sin(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    math_result(x.sin())
}

pub fn math_cos(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    math_result(x.cos())
}

pub fn math_tan(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    math_result(x.tan())
}

pub fn math_asin(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    math_result(x.asin())
}

pub fn math_acos(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    math_result(x.acos())
}

pub fn math_atan(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let x = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    math_result(x.atan())
}

pub fn math_atan2(_gc: &mut SemiSpace, _this: Value, args: &[Value], _vm: &mut Vm) -> Value {
    let y = math_to_f64(args.first().copied().unwrap_or(Value::undefined()));
    let x = math_to_f64(args.get(1).copied().unwrap_or(Value::undefined()));
    math_result(y.atan2(x))
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
/// isNaN(number) — ToNumber coercion, then a NaN check (§21.1.2.5).
/// Symbol arguments throw (ToNumber abrupt), like the numeric ops (C5).
pub fn is_nan_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let v = args.first().copied().unwrap_or(Value::undefined());
    if v.is_symbol() {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert a Symbol value to a number",
        ));
        return Value::undefined();
    }
    Value::boolean(crate::vm::to_number(v).is_nan())
}

/// isFinite(number) — ToNumber coercion, then a finiteness check (§21.1.2.4).
pub fn is_finite_builtin(gc: &mut SemiSpace, _this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let v = args.first().copied().unwrap_or(Value::undefined());
    if v.is_symbol() {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Cannot convert a Symbol value to a number",
        ));
        return Value::undefined();
    }
    Value::boolean(crate::vm::to_number(v).is_finite())
}

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
                    let err = crate::errors::error_object(
                        gc,
                        &vm.error_protos,
                        crate::errors::ErrorKind::TypeError,
                        "Converting circular structure to JSON",
                    );
                    vm.set_pending_exception(err);
                    return Err(());
                }
                stack.push(ptr);
                let len = unsafe { RuneArray::length(ptr as *mut RuneArray) } as usize;
                let mut parts: Vec<String> = Vec::with_capacity(len);
                for i in 0..len {
                    let elem = unsafe { RuneArray::get_element(ptr as *mut RuneArray, i) };
                    // B1e: holes (and unallocated tail slots) stringify
                    // as null.
                    if elem == Value::empty_sentinel() {
                        parts.push("null".to_string());
                        continue;
                    }
                    parts.push(
                        stringify_val(gc, elem, stack, vm).unwrap_or_else(|_| "null".to_string()),
                    );
                }
                stack.pop();
                return Ok(format!("[{}]", parts.join(",")));
            }
            if tag == TAG_OBJECT {
                if stack.contains(&ptr) {
                    vm.set_pending_exception(crate::errors::error_object(
                        gc,
                        &vm.error_protos,
                        crate::errors::ErrorKind::TypeError,
                        "Converting circular structure to JSON",
                    ));
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
                cont: crate::vm::PendingCallCont::Raw,
            });
            vm.push_callback_call(_gc, target, new_this, call_args);
            return Value::undefined();
        }
    }
    Value::undefined()
}

/// Function.prototype.apply(thisArg, argArray) — §20.2.3.1. argArray is read
/// as an array-like (sync); undefined/null → no arguments.
pub fn apply_builtin(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let target = this;
    let new_this = args.first().copied().unwrap_or(Value::undefined());
    let arg_array = args.get(1).copied().unwrap_or(Value::undefined());

    // Step 2: IsCallable(func) else TypeError
    let target_smi = target.as_smi().filter(|&s| s < 0);
    let is_js_func = target
        .heap_ptr()
        .map(|ptr| unsafe { (*(ptr as *const GcHeader)).tag() } == rune_core::gc::TAG_FUNC)
        .unwrap_or(false);
    if !(target_smi.is_some() || is_js_func) {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "Function.prototype.apply called on non-function",
        ));
        return Value::undefined();
    }

    // Step 3-4: CreateListFromArrayLike (undefined/null → empty list)
    let call_args: Vec<Value> = if arg_array.is_null() || arg_array.is_undefined() {
        Vec::new()
    } else {
        match crate::vm::array_like_length(arg_array) {
            Some(len) => (0..len)
                .map(|i| crate::vm::array_like_index(arg_array, i).unwrap_or(Value::undefined()))
                .collect(),
            None => {
                vm.set_pending_exception(crate::errors::error_object(
                    gc,
                    &vm.error_protos,
                    crate::errors::ErrorKind::TypeError,
                    "CreateListFromArrayLike called on non-object",
                ));
                return Value::undefined();
            }
        }
    };

    // Step 6: builtin target → direct call; JS function → pending callback
    if let Some(smi) = target_smi {
        let id = ((-smi) as usize) - 1;
        if id < vm.builtins.len() {
            return (vm.builtins[id].func)(gc, new_this, &call_args, vm);
        }
    }
    vm.pending_call = Some(crate::vm::PendingCall {
        source_frame_depth: 0,
        cont: crate::vm::PendingCallCont::Raw,
    });
    vm.push_callback_call(gc, target, new_this, call_args);
    Value::undefined()
}

// ---------- B1f-2 shared helpers (slice/splice/concat audit) ----------

/// Array exotic length limit (2^32-1): result materialization past it throws
/// RangeError like ArrayCreate (S15.4.4.10_A3_T1).
const MAX_ARRAY_LENGTH: u64 = 4_294_967_295;
/// ToLength clamp (2^53-1): LengthOfArrayLike saturates here, overflow past it
/// throws TypeError (splice step 9, concat spread/single arms).
const MAX_SAFE_INTEGER_U64: u64 = 9_007_199_254_740_991;

/// ToIntegerOrInfinity with builtin-inline object coercion (B1f-2 sync audit):
/// booleans/Smis/floats direct; symbols throw; strings + String objects parse;
/// heap objects try builtin valueOf/toString inline (length_to_number). Every
/// other value (incl. JS-driven methods) reads as 0 — B1f-6 dispatches those
/// through the machine (splice A2.2_T5 valueOf-deleteCount stays failing).
fn to_integer_sync(gc: &mut SemiSpace, vm: &mut Vm, v: Value) -> Result<f64, Value> {
    if let Some(b) = v.to_boolean() {
        return Ok(if b { 1.0 } else { 0.0 });
    }
    let n = length_to_number(gc, vm, v)?;
    if n.is_nan() || n == 0.0 {
        Ok(0.0)
    } else if n.is_infinite() {
        Ok(n)
    } else {
        Ok(n.trunc())
    }
}

/// ToClampedIndex lite (§7.1.26 over ToIntegerOrInfinity): negative counts from
/// `len`, result clamped to [0, len]. Pure u64/f64 math — never iterates.
fn to_clamped_index_checked(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    v: Value,
    len: u64,
) -> Result<u64, Value> {
    let n = to_integer_sync(gc, vm, v)?;
    if n.is_nan() || n == 0.0 {
        Ok(0)
    } else if n < 0.0 {
        Ok(((len as f64 + n).max(0.0).min(len as f64)) as u64)
    } else {
        Ok(n.min(len as f64) as u64)
    }
}

/// Fresh dense array with %Array.prototype% (B1f-2): the plain ArrayCreate all
/// three copy methods share. Species dispatch (custom ctors) is a documented
/// gap — B1f-6 runs user ctors through the machine (all create-species* stay
/// failing, counted there).
fn fresh_dense_array(gc: &mut SemiSpace, vm: &Vm) -> Value {
    let arr = RuneArray::allocate(gc, &[]);
    unsafe {
        let ptr = arr as *mut u8;
        *(ptr.add(8) as *mut *const Shape) = *DENSE_ARRAY_SHAPE as *const Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(ptr.add(24) as *mut *mut u8) = proto;
        }
    }
    Value::from_heap_ptr(arr as *mut u8)
}

/// Push onto a fresh result array under the Array length invariant (B1f-2):
/// indices stop at 2^32-1 — anything past throws RangeError like ArrayCreate.
/// Refreshes both sides across the growing allocation (B1f-1 discipline).
fn result_push(gc: &mut SemiSpace, vm: &Vm, result: &mut Value, val: Value) -> Result<(), Value> {
    *result = refresh_value(*result);
    let rptr = match result.heap_ptr() {
        Some(p) => p as *mut RuneArray,
        None => return Err(sort_range_error(gc, vm, "Invalid array length")),
    };
    if unsafe { RuneArray::length(rptr) } as u64 >= MAX_ARRAY_LENGTH {
        return Err(sort_range_error(gc, vm, "Invalid array length"));
    }
    let val = refresh_value(val);
    unsafe {
        let np = RuneArray::push(gc, rptr, val);
        *result = Value::from_heap_ptr(np as *mut u8);
    }
    Ok(())
}

/// Fill `count` holes at the result tail (B1f-2): positional gaps between
/// sparse-candidate reads stay holes. A gap that would carry the result past
/// 2^32-1 throws RangeError up front — the fills are provably silent (no indexed
/// entries anywhere in the gap, same proof as move_range_is_quiet), so no
/// observable Get is skipped. Caveat (B1f-6): a far throwing getter past the
/// limit would surface RangeError instead of its own throw.
fn result_fill_holes(
    gc: &mut SemiSpace,
    vm: &Vm,
    result: &mut Value,
    count: u64,
) -> Result<(), Value> {
    if count == 0 {
        return Ok(());
    }
    *result = refresh_value(*result);
    let cur = result
        .heap_ptr()
        .map(|p| unsafe { RuneArray::length(p as *mut RuneArray) } as u64)
        .unwrap_or(0);
    if cur + count > MAX_ARRAY_LENGTH {
        return Err(sort_range_error(gc, vm, "Invalid array length"));
    }
    for _ in 0..count {
        result_push(gc, vm, result, Value::empty_sentinel())?;
    }
    Ok(())
}

/// LengthOfArrayLike for the copy family (B1f-2): B1f-1 chain-aware lengths
/// plus TypedArray exotic lengths (integer-indexed count, read-only) plus
/// String-object exotic lengths (inner-string units — the spreadable-string
/// content reads). JS length getters read as pair values (B1f-6 dispatches them
/// — same sync-gap as B1f-1, e.g. slice create-non-array-invalid-len stays
/// failing).
fn seq_length(gc: &mut SemiSpace, vm: &mut Vm, this: Value) -> Result<(u64, f64), Value> {
    if let Some(ptr) = this.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_TYPED_ARRAY {
            let len = unsafe { typedarray::RuneTypedArray::length(ptr) } as u64;
            return Ok((len, len as f64));
        }
        if tag == TAG_STRING_OBJ {
            let sptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
            let units = unsafe { HeapString::to_string(sptr as *mut HeapString) }
                .encode_utf16()
                .count() as u64;
            return Ok((units, units as f64));
        }
    }
    mutator_length(gc, vm, this)
}

/// HasProperty for the copy family (B1f-2): the funnel check plus String-object
/// exotic indices (own shape first, else inner-string units — the funnel has no
/// TAG_STRING_OBJ presence arm). Primitive strings and TypedArrays ride the
/// funnel directly.
fn seq_has(obj: Value, key: Value) -> bool {
    if crate::vm::has_property(obj, key, None) {
        return true;
    }
    if let Some(ptr) = obj.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_STRING_OBJ {
            if let Some(idx) = value_to_array_index(key) {
                let sptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
                let units = unsafe { HeapString::to_string(sptr as *mut HeapString) }
                    .encode_utf16()
                    .count();
                return idx < units;
            }
        }
    }
    false
}

/// Get for the copy family (B1f-2): primitive-string and String-object indices
/// serve UTF-16 units (load_property_recursive has no primitive-string arm);
/// everything else rides mutator_read (builtin-inline, JS gap → undefined).
fn seq_read(gc: &mut SemiSpace, vm: &mut Vm, obj: &mut Value, idx: u64) -> Result<Value, Value> {
    if let Some(ptr) = obj.heap_ptr() {
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_STRING || tag == TAG_STRING_OBJ {
            let s = if tag == TAG_STRING {
                unsafe { HeapString::to_string(ptr as *mut HeapString) }
            } else {
                let sptr = unsafe { StringObject::string_ptr(ptr as *mut StringObject) };
                unsafe { HeapString::to_string(sptr as *mut HeapString) }
            };
            *obj = refresh_value(*obj);
            let units: Vec<u16> = s.encode_utf16().collect();
            if idx < units.len() as u64 {
                let u = units[idx as usize];
                let ch = char::decode_utf16(std::iter::once(u))
                    .next()
                    .unwrap()
                    .unwrap_or(char::REPLACEMENT_CHARACTER);
                let hs = HeapString::allocate(gc, &ch.to_string());
                *obj = refresh_value(*obj);
                return Ok(Value::from_heap_ptr(hs as *mut u8));
            }
            return Ok(Value::undefined());
        }
    }
    mutator_read(gc, vm, obj, idx)
}

/// IsConcatSpreadable (§23.1.3.2.1, B1f-2 sync audit): non-Objects (primitives
/// incl. primitive strings) are never spreadable; otherwise Get
/// @@isConcatSpreadable (proto walk via the funnel, builtin-inline) with
/// ToBoolean on non-undefined, else the IsArray fallback. Uses Get, not
/// GetMethod — any non-undefined value boolean-coerces, never throws. JS
/// getters read as absent (B1f-6: is-concat-spreadable-get-err/get-order stay
/// failing).
fn is_concat_spreadable_sync(gc: &mut SemiSpace, vm: &mut Vm, item: Value) -> Result<bool, Value> {
    let Some(ptr) = item.heap_ptr() else {
        return Ok(false);
    };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag == TAG_STRING {
        return Ok(false);
    }
    let raw = load_property_recursive(
        item,
        Value::symbol(rune_core::symbol::SYM_IS_CONCAT_SPREADABLE),
        None,
        gc,
    );
    // Resolve accessor pairs exactly like mutator_read (builtin inline, JS gap).
    let v = match raw.heap_ptr() {
        Some(aptr) if unsafe { (*(aptr as *const GcHeader)).tag() } == TAG_ACCESSOR => {
            let getter = unsafe { rune_core::accessor::AccessorPair::getter(aptr) };
            if getter.is_undefined() || getter.is_null() {
                Value::undefined()
            } else if let Some(smi) = getter.as_smi() {
                if smi < 0 {
                    let id = ((-smi) as usize) - 1;
                    if id < vm.builtins.len() {
                        let r = (vm.builtins[id].func)(gc, item, &[], vm);
                        if let Some(exc) = vm.pending_exception.take() {
                            return Err(exc);
                        }
                        r
                    } else {
                        Value::undefined()
                    }
                } else {
                    Value::undefined()
                }
            } else {
                Value::undefined()
            }
        }
        _ => raw,
    };
    if v.is_undefined() {
        return Ok(tag == TAG_ARRAY);
    }
    Ok(v.to_bool())
}

/// Strict numeric key membership (B1f-2): all digits, no leading zeros (unless
/// the key is exactly "0"), value in [lo, hi). Unlike canonical_index_name this
/// admits huge named indices (the B1f-1 named-overflow model) — index_key only
/// ever emits `k.to_string()`, so exact string equality is the membership test.
fn sparse_key_in_range(name: &str, lo: u64, hi: u64) -> Option<u64> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if name.len() > 1 && name.starts_with('0') {
        return None;
    }
    let n: u64 = name.parse().ok()?;
    if n.to_string() != name {
        return None;
    }
    (lo <= n && n < hi).then_some(n)
}

/// Sorted indexed keys present (own or inherited) in [lo, hi) (B1f-2): lets huge
/// sparse ranges walk in O(entries) — gaps are provably silent (no indexed
/// entries anywhere in the gap, same proof as move_range_is_quiet), so readers
/// and shifts visit only these. Returns None when an exotic link (non-plain
/// proto, e.g. a dense-array proto serving indices) cannot be enumerated —
/// the caller falls back to a direct walk.
fn sparse_present_in(obj: Value, lo: u64, hi: u64) -> Option<Vec<u64>> {
    if lo >= hi {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    let mut current = obj;
    for _ in 0..crate::vm::MAX_PROTOTYPE_DEPTH {
        let Some(cptr) = current.heap_ptr() else {
            return Some(out);
        };
        let tag = unsafe { (*(cptr as *const GcHeader)).tag() };
        if tag == TAG_OBJECT {
            let shape = unsafe { JSObject::shape_ptr(cptr as *mut JSObject) };
            let count = unsafe { JSObject::slot_count(cptr as *mut JSObject) };
            for i in 0..count {
                if let Some(name) = shape.key_name_at(i) {
                    if let Some(n) = sparse_key_in_range(name, lo, hi) {
                        out.push(n);
                    }
                }
            }
            let proto = unsafe { JSObject::prototype(cptr as *mut JSObject) };
            if proto.is_null() {
                break;
            }
            current = Value::from_heap_ptr(proto);
        } else if tag == TAG_ARRAY {
            // Named overflow (huge indices) + accessor overlay pairs live in
            // extra_props; materialized elements are walked directly by the
            // caller, and the proto chain below is enumerated by the loop.
            let extra = unsafe { RuneArray::extra_props(cptr as *mut RuneArray) };
            if !extra.is_null() {
                let eshape = unsafe { JSObject::shape_ptr(extra as *mut JSObject) };
                let ecount = unsafe { JSObject::slot_count(extra as *mut JSObject) };
                for i in 0..ecount {
                    if let Some(name) = eshape.key_name_at(i) {
                        if let Some(n) = sparse_key_in_range(name, lo, hi) {
                            out.push(n);
                        }
                    }
                }
            }
            let proto = unsafe { JSObject::prototype(cptr as *mut JSObject) };
            if proto.is_null() {
                break;
            }
            current = Value::from_heap_ptr(proto);
        } else {
            // Exotic link (typed array, string object, regexp, ...): indices
            // may be served without shape entries — cannot enumerate.
            return None;
        }
    }
    out.sort_unstable();
    out.dedup();
    Some(out)
}

/// Copy [lo, hi) from `obj` onto the fresh `result` positionally (B1f-2):
/// holes stay holes (sentinel pushes), present elements are Get-pushed as own
/// (CreateDataPropertyOrThrow — no setter dispatch on a fresh array, so the
/// A4_T1 proto-served element lands own). Dense receivers walk the
/// materialized window directly (bounded by capacity — tails past capacity are
/// holes by construction) plus sparse tail candidates; plain objects walk
/// sparse candidates only (O(entries) — the 2^53 clamps ranges); exotics walk
/// directly (memory-bounded: strings/typed arrays). Returns Err on abrupt Gets
/// or the 2^32-1 RangeError valve.
fn copy_range_to_result(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    obj: &mut Value,
    result: &mut Value,
    lo: u64,
    hi: u64,
) -> Result<(), Value> {
    if lo >= hi {
        return Ok(());
    }
    *obj = refresh_value(*obj);
    let is_dense = obj
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_ARRAY);
    if is_dense {
        let (dlen, cap) = obj
            .heap_ptr()
            .map(|p| unsafe {
                (
                    RuneArray::length(p as *mut RuneArray) as u64,
                    RuneArray::capacity(p as *mut RuneArray) as u64,
                )
            })
            .unwrap_or((0, 0));
        let win_end = hi.min(dlen.min(cap)).max(lo);
        let mut k = lo;
        while k < win_end {
            let key = index_key(gc, k);
            *obj = refresh_value(*obj);
            if seq_has(*obj, key) {
                let mut o = *obj;
                let v = seq_read(gc, vm, &mut o, k)?;
                *obj = o;
                result_push(gc, vm, result, v)?;
            } else {
                result_push(gc, vm, result, Value::empty_sentinel())?;
            }
            k += 1;
        }
        // Tail past the materialized window: holes except sparse candidates
        // (named overflow, overlay pairs, proto entries).
        if win_end < hi {
            let tail_cands = sparse_present_in(*obj, win_end, hi).unwrap_or_default();
            copy_sparse_to_result(gc, vm, obj, result, tail_cands, win_end, hi)?;
        }
        return Ok(());
    }
    if let Some(cands) = sparse_present_in(*obj, lo, hi) {
        return copy_sparse_to_result(gc, vm, obj, result, cands, lo, hi);
    }
    // Exotic fallback: direct walk (bounded in practice).
    let mut k = lo;
    while k < hi {
        let key = index_key(gc, k);
        *obj = refresh_value(*obj);
        if seq_has(*obj, key) {
            let mut o = *obj;
            let v = seq_read(gc, vm, &mut o, k)?;
            *obj = o;
            result_push(gc, vm, result, v)?;
        } else {
            result_push(gc, vm, result, Value::empty_sentinel())?;
        }
        k += 1;
    }
    Ok(())
}

/// Sparse positional copy (B1f-2): visit only the given `sparse_present_in`
/// candidates in [lo, hi), filling every gap with holes. Order-preserving —
/// Gets run in ascending index order exactly like the spec loop; gaps are
/// provably silent.
fn copy_sparse_to_result(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    obj: &mut Value,
    result: &mut Value,
    cands: Vec<u64>,
    lo: u64,
    hi: u64,
) -> Result<(), Value> {
    let mut pos = lo;
    for c in cands {
        if c < pos || c >= hi {
            continue;
        }
        let key = index_key(gc, c);
        *obj = refresh_value(*obj);
        if seq_has(*obj, key) {
            result_fill_holes(gc, vm, result, c - pos)?;
            let mut o = *obj;
            let v = seq_read(gc, vm, &mut o, c)?;
            *obj = o;
            result_push(gc, vm, result, v)?;
            pos = c + 1;
        } else {
            // Vanished between collect and walk (builtin-getter shape motion):
            // treat the whole span as holes.
            result_fill_holes(gc, vm, result, c - pos + 1)?;
            pos = c + 1;
        }
    }
    result_fill_holes(gc, vm, result, hi - pos)?;
    Ok(())
}

/// Array.prototype.slice(start, end) — §23.1.3.28 audit (B1f-2): generic over
/// heap receivers with hole preservation and proto fallthrough (A4_T1), u64
/// walks with ToLength clamping, ToIntegerOrInfinity start/end (floats truncate
/// — A2.1_T1/A2.2_T1), species-plain result (create-species* → B1f-6), 2^32-1
/// RangeError before any copy (A3_T1/T2, was ENGINE PANIC). Observable
/// length/element getters (create-non-array-invalid-len) + Proxy/resizable →
/// B1f-6/B7/out-of-scope.
pub fn array_slice(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let (len, _) = match seq_length(gc, vm, this) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    let start_v = args.first().copied().unwrap_or(Value::undefined());
    let k = match to_clamped_index_checked(gc, vm, start_v, len) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    let end_v = args.get(1).copied().unwrap_or(Value::undefined());
    let fin = if end_v.is_undefined() {
        len
    } else {
        match to_clamped_index_checked(gc, vm, end_v, len) {
            Ok(v) => v,
            Err(e) => {
                vm.set_pending_exception(e);
                return Value::undefined();
            }
        }
    };
    let count = fin.saturating_sub(k);
    // Species ArrayCreate throws before any element Get past 2^32-1.
    if count > MAX_ARRAY_LENGTH {
        vm.set_pending_exception(sort_range_error(gc, vm, "Invalid array length"));
        return Value::undefined();
    }
    let mut result = fresh_dense_array(gc, vm);
    let mut obj = this;
    if let Err(e) = copy_range_to_result(gc, vm, &mut obj, &mut result, k, fin) {
        vm.set_pending_exception(e);
        return Value::undefined();
    }
    // Length is exact by construction (holes pushed as sentinel); set
    // explicitly per spec step 9 (covers non-Array species — plain here).
    result = refresh_value(result);
    if let Some(rptr) = result.heap_ptr() {
        unsafe { RuneArray::set_length(rptr as *mut RuneArray, count as u32) };
    }
    result
}

/// Array.prototype.concat — §23.1.3.2 audit (B1f-2): species-plain result,
/// IsConcatSpreadable via Get (own + proto flags dispatch now — the
/// Boolean/String-prototype flag reads work; lone-surrogate content stays B2),
/// LengthOfArrayLike per spreadable AFTER the spreadable check (spec order),
/// 2^53-1 overflow TypeErrors on both arms (arg-length-exceeding, was ENGINE
/// PANIC), hole preservation on spread ranges (sloppy-arguments tail). Single
/// items push as-is; the ToObject-boxed `this` identity is B2 (call-with-boolean
/// stays failing — no %Boolean.prototype% exists yet). TypedArray spread reads
/// ride the integer-indexed exotic (length + elements); TA named stores
/// (the spreadable flag itself) depend on B4 named-prop support.
pub fn array_concat(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let mut result = fresh_dense_array(gc, vm);
    let mut next: u64 = 0;
    // Operands: `this` first, then the args — each refreshed at use (pushes may
    // GC-move them; B1f-1 discipline). Never pre-collect into a Vec (stale).
    let total = 1 + args.len();
    let mut n = 0usize;
    while n < total {
        let raw = if n == 0 { this } else { args[n - 1] };
        let item = refresh_value(raw);
        let spreadable = match is_concat_spreadable_sync(gc, vm, item) {
            Ok(v) => v,
            Err(e) => {
                vm.set_pending_exception(e);
                return Value::undefined();
            }
        };
        // The spreadable Get may have allocated (builtin getter): refresh.
        let mut obj = refresh_value(item);
        if spreadable {
            let (ilen, _) = match seq_length(gc, vm, obj) {
                Ok(v) => v,
                Err(e) => {
                    vm.set_pending_exception(e);
                    return Value::undefined();
                }
            };
            if next + ilen > MAX_SAFE_INTEGER_U64 {
                vm.set_pending_exception(sort_type_error(
                    gc,
                    vm,
                    "Concat spread exceeds the maximum array length",
                ));
                return Value::undefined();
            }
            // Append [0, ilen) positionally (result length == next here).
            if let Err(e) = copy_range_to_result(gc, vm, &mut obj, &mut result, 0, ilen) {
                vm.set_pending_exception(e);
                return Value::undefined();
            }
            next += ilen;
        } else {
            if next >= MAX_SAFE_INTEGER_U64 {
                vm.set_pending_exception(sort_type_error(
                    gc,
                    vm,
                    "Concat spread exceeds the maximum array length",
                ));
                return Value::undefined();
            }
            if let Err(e) = result_push(gc, vm, &mut result, obj) {
                vm.set_pending_exception(e);
                return Value::undefined();
            }
            next += 1;
        }
        n += 1;
    }
    // Final length set explicitly (spec step 6 — trailing holes/non-Array
    // species; plain here, exact by construction).
    result = refresh_value(result);
    if let Some(rptr) = result.heap_ptr() {
        unsafe { RuneArray::set_length(rptr as *mut RuneArray, next as u32) };
    }
    result
}

/// Array.prototype.unshift — §23.1.3.35, inserts elements at start and returns new length
/// Array.prototype.splice — §23.1.3.31 audit (B1f-2): spec-order abrupts
/// (clamped start → integer deleteCount → 2^53-1 overflow TypeError BEFORE any
/// mutation — throws-if-integer-limit-exceeded), presence-aware deleted copy
/// (holes stay holes) and presence-aware shifts (move-or-delete with the
/// non-configurable throw — A4_T1), u64 index math with ToLength clamping (the
/// length-near/exceeding families, was u32-wrap ENGINE PANIC), O(entries)
/// sparse walks for plain objects, dense pre-grow + refresh discipline.
/// Object deleteCount valueOf (A2.2_T5) + observable length accessors
/// (set_length_no_args) + frozen/sealed (target-array-*) + species/Proxy →
/// B1f-6/B1f-5/B7 (documented, counted).
pub fn array_splice(gc: &mut SemiSpace, mut this: Value, args: &[Value], vm: &mut Vm) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let (len, _) = match seq_length(gc, vm, this) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    // Step 3: actualStart (start absent → undefined → 0, same value).
    let start_v = args.first().copied().unwrap_or(Value::undefined());
    let actual_start = match to_clamped_index_checked(gc, vm, start_v, len) {
        Ok(v) => v,
        Err(e) => {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    };
    // Steps 5-8: no args → 0 deletes; one arg → to the end; else
    // ToIntegerOrInfinity clamped to [0, len - actualStart] (symbols throw).
    let item_count: u64 = args.len().saturating_sub(2) as u64;
    let actual_delete: u64 = if args.is_empty() {
        0
    } else if args.len() == 1 {
        len.saturating_sub(actual_start)
    } else {
        match to_integer_sync(gc, vm, args[1]) {
            Ok(dc) => dc.clamp(0.0, len.saturating_sub(actual_start) as f64) as u64,
            Err(e) => {
                vm.set_pending_exception(e);
                return Value::undefined();
            }
        }
    };
    // Step 9: overflow BEFORE any mutation (u64 — the old u32 wrap PANICKED).
    // Exact: del ≤ len - actual_start ≤ len, so no saturation triggers.
    let new_len = len.saturating_add(item_count).saturating_sub(actual_delete);
    if new_len > MAX_SAFE_INTEGER_U64 {
        vm.set_pending_exception(sort_type_error(
            gc,
            vm,
            "Splice exceeds the maximum array length",
        ));
        return Value::undefined();
    }
    // Species ArrayCreate throws past 2^32-1 before the deleted copy.
    if actual_delete > MAX_ARRAY_LENGTH {
        vm.set_pending_exception(sort_range_error(gc, vm, "Invalid array length"));
        return Value::undefined();
    }
    // Steps 10-13: deleted copy (fresh array, holes preserved).
    let mut deleted = fresh_dense_array(gc, vm);
    let mut ro = this;
    if let Err(e) = copy_range_to_result(
        gc,
        vm,
        &mut ro,
        &mut deleted,
        actual_start,
        actual_start + actual_delete,
    ) {
        vm.set_pending_exception(e);
        return Value::undefined();
    }
    this = refresh_value(ro);
    deleted = refresh_value(deleted);
    if let Some(dptr) = deleted.heap_ptr() {
        unsafe { RuneArray::set_length(dptr as *mut RuneArray, actual_delete as u32) };
    }
    // Discarded-box primitives: the boxed temp is thrown away (spec ToObject).
    if this.heap_ptr().is_none() {
        return deleted;
    }
    let is_dense = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_ARRAY);
    if is_dense && item_count > actual_delete {
        ensure_dense_capacity(gc, vm, &mut this, new_len);
    }
    // Steps 14-15: shifts in spec k-order (B1f-1 idiom verbatim): per-k
    // presence-checked move-or-delete via the funnels (holes/OOB/cap all read
    // absent with proto fallthrough, so no window math is needed for
    // correctness); huge sparse quiet spans skip entirely (the 2^53 clamps
    // ranges — same proof as move_range_is_quiet). Dense receivers always walk
    // (their elements live outside shapes); test spans are small, and the
    // theoretical huge-dense-tail walk is C-hardening shared with B1f-1 shift.
    if item_count != actual_delete {
        let left = item_count < actual_delete;
        let (lo, hi) = (actual_start, len.saturating_sub(actual_delete));
        // Union span of touched positions for the quiet check.
        let (ulo, uhi) = if left {
            (actual_start + item_count, len)
        } else {
            (actual_start + actual_delete, new_len)
        };
        let skip = !is_dense && move_range_is_quiet(this, ulo, uhi);
        if !skip {
            // Per-k move-or-delete body, inlined per direction (B1f-1 shape).
            if left {
                let mut k = lo;
                while k < hi {
                    let from = k + actual_delete;
                    let to = k + item_count;
                    let key = index_key(gc, from);
                    this = refresh_value(this);
                    if seq_has(this, key) {
                        let mut o = this;
                        match seq_read(gc, vm, &mut o, from) {
                            Ok(v) => {
                                this = o;
                                if let Err(e) = mutator_store(gc, vm, &mut this, to, v) {
                                    vm.set_pending_exception(e);
                                    return Value::undefined();
                                }
                            }
                            Err(e) => {
                                vm.set_pending_exception(e);
                                return Value::undefined();
                            }
                        }
                    } else if let Err(e) = mutator_delete(gc, vm, &mut this, to) {
                        vm.set_pending_exception(e);
                        return Value::undefined();
                    }
                    k += 1;
                }
                // Trailing deletes [new_len, len) — inside the quiet span.
                let mut t = new_len;
                while t < len {
                    if let Err(e) = mutator_delete(gc, vm, &mut this, t) {
                        vm.set_pending_exception(e);
                        return Value::undefined();
                    }
                    t += 1;
                }
            } else {
                let mut k = hi;
                while k > lo {
                    k -= 1;
                    let from = k + actual_delete;
                    let to = k + item_count;
                    let key = index_key(gc, from);
                    this = refresh_value(this);
                    if seq_has(this, key) {
                        let mut o = this;
                        match seq_read(gc, vm, &mut o, from) {
                            Ok(v) => {
                                this = o;
                                if let Err(e) = mutator_store(gc, vm, &mut this, to, v) {
                                    vm.set_pending_exception(e);
                                    return Value::undefined();
                                }
                            }
                            Err(e) => {
                                vm.set_pending_exception(e);
                                return Value::undefined();
                            }
                        }
                    } else if let Err(e) = mutator_delete(gc, vm, &mut this, to) {
                        vm.set_pending_exception(e);
                        return Value::undefined();
                    }
                }
            }
        }
    }
    // Step 17: items via Set (throw=true — strings reject here).
    for (i, &raw_item) in args.iter().skip(2).enumerate() {
        let item = refresh_value(raw_item);
        if let Err(e) = mutator_store(gc, vm, &mut this, actual_start + i as u64, item) {
            vm.set_pending_exception(e);
            return Value::undefined();
        }
    }
    // Step 18: length (dense RangeError past 2^32-1 = array exotic invariant).
    if let Err(e) = set_length_checked(gc, vm, &mut this, new_len) {
        vm.set_pending_exception(e);
        return Value::undefined();
    }
    deleted
}

/// Resolve a possibly-forwarded GC pointer to its current address (single hop).
/// After a collection, local Value copies hold stale from-space addresses whose
/// headers carry a forwarding pointer to the live to-space copy.
fn resolve_forwarded(ptr: *mut u8) -> *mut u8 {
    unsafe {
        let h = ptr as *const GcHeader;
        if (*h).is_forwarded() {
            (*h).forwarding_addr()
        } else {
            ptr
        }
    }
}

/// Re-resolve a local Value copy after an allocation may have triggered a GC
/// that moved its heap object. Roots were rewritten by the collector itself;
/// only Rust-local copies need refreshing.
fn refresh_value(v: Value) -> Value {
    match v.heap_ptr() {
        Some(p) => {
            let r = resolve_forwarded(p);
            if std::ptr::eq(r, p) {
                v
            } else {
                Value::from_heap_ptr(r)
            }
        }
        None => v,
    }
}

/// Grow a dense array, keeping VM roots in sync (same discipline as array_push).
/// Returns the live pointer to use for subsequent element access.
unsafe fn grow_dense_array(gc: &mut SemiSpace, vm: &mut Vm, cur: *mut RuneArray) -> *mut RuneArray {
    unsafe {
        let (resolved_old, new_arr) = RuneArray::grow(gc, cur);
        if resolved_old != new_arr as *mut u8 {
            vm.update_heap_reference(resolved_old, new_arr as *mut u8);
        }
        new_arr
    }
}

/// Array.prototype.indexOf(searchElement, fromIndex) — returns index of first match, -1 if not found.
pub fn array_index_of(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    array_index_search(gc, vm, this, args, crate::vm::ArrayOpKind::IndexOf)
}

/// B1b shared setup for indexOf/lastIndexOf/includes: coercible → length →
/// clamped start → resumable search step. Direction and found/not-found
/// values come from the kind.
fn array_index_search(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    this: Value,
    args: &[Value],
    kind: crate::vm::ArrayOpKind,
) -> Value {
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    // B1c: symbol lengths throw (see array_iter_prologue).
    if let Some(ptr) = this.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_OBJECT {
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            if let Some(slot) = shape.lookup(&PropertyKey::from_string("length")) {
                let lv = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                if lv.is_symbol() {
                    vm.set_pending_exception(crate::errors::error_object(
                        gc,
                        &vm.error_protos,
                        crate::errors::ErrorKind::TypeError,
                        "Cannot convert a Symbol value to a number",
                    ));
                    return Value::undefined();
                }
            }
        }
    }
    let search = args.first().copied().unwrap_or(Value::undefined());
    let len = crate::vm::array_like_length(this).unwrap_or(0);
    let backward = kind == crate::vm::ArrayOpKind::LastIndexOf;
    // Clamp the start index (§23.1.3.16-kanon: indexOf/includes forward
    // from max(n,0)/immediate miss; lastIndexOf backward from min(n,len-1)).
    let start: Option<usize> = if backward {
        if len == 0 {
            None
        } else if args.len() < 2 || args[1].is_undefined() {
            Some(len as usize - 1)
        } else {
            let n = to_integer_or_infinity(args[1]);
            if n >= 0.0 {
                Some((n as usize).min(len as usize - 1))
            } else {
                let k = len as f64 + n;
                if k < 0.0 { None } else { Some(k as usize) }
            }
        }
    } else {
        let n = if args.len() < 2 {
            0.0
        } else {
            to_integer_or_infinity(args[1])
        };
        if n >= len as f64 {
            None
        } else if n < 0.0 {
            Some((len as f64 + n).max(0.0) as usize)
        } else {
            Some(n as usize)
        }
    };
    let Some(start) = start else {
        // Empty range: immediate miss without starting a machine.
        return match kind {
            crate::vm::ArrayOpKind::Includes => Value::boolean(false),
            _ => Value::smi(-1),
        };
    };
    let mut op = crate::vm::ArrayOpState {
        kind,
        source: this.heap_ptr().unwrap_or(std::ptr::null_mut()),
        result: std::ptr::null_mut(),
        callback: Value::undefined(),
        this_val: Value::undefined(),
        source_val: this,
        index: start,
        length: len,
        source_frame_depth: 0,
        accumulator: Some(search),
        awaiting_element: None,
        awaiting_acc: false,
    };
    // First step runs inline; only a JS element getter parks the machine.
    match crate::vm::array_search_step(vm, gc, &mut op, None) {
        crate::vm::SearchStepOut::Done(v) => v,
        crate::vm::SearchStepOut::Wait => {
            vm.pending_array_op = Some(op);
            vm.rebase_pending_depths();
            Value::undefined()
        }
        crate::vm::SearchStepOut::SyncErr(e) => {
            vm.set_pending_exception(e);
            Value::undefined()
        }
    }
}

/// Array.prototype.lastIndexOf(search, fromIndex) — reverse search (B1b).
/// Strict equality, holes included, getters dispatched via the search step.
pub fn array_last_index_of(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    array_index_search(gc, vm, this, args, crate::vm::ArrayOpKind::LastIndexOf)
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
    array_index_search(gc, vm, this, args, crate::vm::ArrayOpKind::Includes)
}

/// Array.prototype.forEach(callback, thisArg) — same state machine, no result array.
/// B1a shared prologue for iterative Array methods (map/filter/forEach/
/// find/findIndex/some/every/flatMap/reduce). Spec order per method:
/// RequireObjectCoercible → LengthOfArrayLike (data path) → IsCallable.
/// Returns (length, callback, this_arg, source heap pointer or null for
/// primitives). On failure sets a pending TypeError and returns None (the
/// caller returns its kind-specific default).
fn array_iter_prologue(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    this: Value,
    args: &[Value],
    method: &str,
) -> Option<(u32, Value, Value, *mut u8)> {
    if !require_object_coercible(this, vm, gc) {
        return None;
    }
    // B1f-3: LengthOfArrayLike via the B1f-1 chain reader (own + inherited data
    // lengths, ToLength-clamped u64; symbol lengths throw anywhere on the chain
    // — subsumes the old inline own-slot symbol check). JS-driven lengths read
    // as pair values → 0 (B1f-6 dispatches them). Primitive strings stay 0
    // (baseline): full String-exotic iteration (lengths, char reads, boxed
    // callback receivers) is B2 — serving lengths here exposed broken reads
    // and unboxed receivers (reduce 1-7).
    let is_prim_string = this
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_STRING);
    let (len_u64, _) = if is_prim_string {
        (0, 0.0)
    } else {
        match mutator_length(gc, vm, this) {
            Ok(v) => v,
            Err(e) => {
                vm.set_pending_exception(e);
                return None;
            }
        }
    };
    // B1f-3: map species-creates with `length` (spec step 4, before the walk),
    // so ArrayCreate throws RangeError past 2^32-1 (3-28 — the old u32
    // saturation hung instead). Other kinds species-create empty / walk.
    if method == "Array.prototype.map" && len_u64 > MAX_ARRAY_LENGTH {
        vm.set_pending_exception(sort_range_error(gc, vm, "Invalid array length"));
        return None;
    }
    let length = len_u64.min(u32::MAX as u64) as u32;
    let callback = args.first().copied().unwrap_or(Value::undefined());
    let callable = callback.as_smi().is_some_and(|s| s < 0)
        || callback
            .heap_ptr()
            .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_FUNC });
    if !callable {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            &format!("{method} requires a callback function"),
        ));
        return None;
    }
    let this_arg = args.get(1).copied().unwrap_or(Value::undefined());
    let source_ptr = this.heap_ptr().unwrap_or(std::ptr::null_mut());
    Some((length, callback, this_arg, source_ptr))
}

/// First present index in [0, len) per HasProperty semantics
/// (B1a: own slots AND proto chain — a setter-only proto accessor counts as
/// present, reading as undefined). Dense arrays have no holes
/// (index < length always present).
fn first_existing_index(this: Value, len: u32) -> Option<usize> {
    next_existing_index(this, 0, len)
}

/// First present index in [from, len) (HasProperty semantics, see above).
fn next_existing_index(this: Value, from: usize, len: u32) -> Option<usize> {
    (from..len as usize).find(|&i| crate::vm::has_property(this, Value::smi(i as i32), None))
}

/// Last present index in [0, len) (B1c: backward iteration seeds).
fn last_existing_index(this: Value, len: u32) -> Option<usize> {
    prev_existing_index(this, len as usize)
}

/// Last present index in [0, before) (B1c).
fn prev_existing_index(this: Value, before: usize) -> Option<usize> {
    (0..before)
        .rev()
        .find(|&i| crate::vm::has_property(this, Value::smi(i as i32), None))
}

pub fn array_for_each(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some((length, callback, this_arg, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.forEach")
    else {
        return Value::undefined();
    };
    let Some(first) = first_existing_index(this, length) else {
        return Value::undefined();
    };
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::ForEach,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: first,
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    // B1a: accessor elements dispatch their getter (Wait records the
    // await; SyncErr routes through the normal pending-exception path).
    match crate::vm::array_element_value(vm, gc, this, first) {
        crate::vm::ArrayElemOut::Ready(element) => {
            vm.push_callback_call(
                gc,
                callback,
                this_arg,
                vec![element, Value::smi(first as i32), this],
            );
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(first);
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
    Value::undefined()
}

/// Array.prototype.filter(callback, thisArg) — set up state machine iteration.
pub fn array_filter(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some((length, callback, this_arg, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.filter")
    else {
        return Value::undefined();
    };
    let result_arr = RuneArray::allocate(gc, &[]);
    unsafe {
        let ptr = result_arr as *mut u8;
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
            *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(ptr.add(24) as *mut *mut u8) = proto;
        }
    }
    let Some(first) = first_existing_index(this, length) else {
        return Value::from_heap_ptr(result_arr as *mut u8);
    };
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Filter,
        source: source_ptr,
        result: result_arr as *mut u8,
        callback,
        this_val: this_arg,
        source_val: this,
        index: first,
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    // B1a: accessor elements dispatch their getter (Wait records the
    // await; SyncErr routes through the normal pending-exception path).
    match crate::vm::array_element_value(vm, gc, this, first) {
        crate::vm::ArrayElemOut::Ready(element) => {
            vm.push_callback_call(
                gc,
                callback,
                this_arg,
                vec![element, Value::smi(first as i32), this],
            );
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(first);
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
    Value::undefined()
}

/// Array.prototype.map(callback, thisArg) — set up state machine iteration.
pub fn array_map(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some((length, callback, this_arg, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.map")
    else {
        return Value::undefined();
    };
    let result_arr = RuneArray::allocate(gc, &[]);
    unsafe {
        let ptr = result_arr as *mut u8;
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
            *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(ptr.add(24) as *mut *mut u8) = proto;
        }
    }
    let Some(first) = first_existing_index(this, length) else {
        // B1f-3: map presizes the species length (holes stay holes — the walk
        // never visits them). Unallocated tail slots read as holes by
        // construction (B1e capacity discipline), so no fill is needed.
        unsafe { RuneArray::set_length(result_arr, length) };
        return Value::from_heap_ptr(result_arr as *mut u8);
    };
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Map,
        source: source_ptr,
        result: result_arr as *mut u8,
        callback,
        this_val: this_arg,
        source_val: this,
        index: first,
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    // B1a: accessor elements dispatch their getter (Wait records the
    // await; SyncErr routes through the normal pending-exception path).
    match crate::vm::array_element_value(vm, gc, this, first) {
        crate::vm::ArrayElemOut::Ready(element) => {
            vm.push_callback_call(
                gc,
                callback,
                this_arg,
                vec![element, Value::smi(first as i32), this],
            );
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(first);
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
    Value::undefined()
}

/// Array.prototype.reduce(callback, initialValue) — set up state machine.
/// Without initialValue the accumulator starts at the first PRESENT element
/// (holes skipped); no present elements at all → TypeError.
pub fn array_reduce(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some((length, callback, _, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.reduce")
    else {
        return Value::undefined();
    };
    let has_initial = args.len() > 1;
    let initial = args.get(1).copied().unwrap_or(Value::undefined());
    if has_initial {
        let Some(first) = first_existing_index(this, length) else {
            return initial;
        };
        vm.pending_array_op = Some(crate::vm::ArrayOpState {
            kind: crate::vm::ArrayOpKind::Reduce,
            source: source_ptr,
            result: std::ptr::null_mut(),
            callback,
            this_val: Value::undefined(),
            source_val: this,
            index: first,
            length,
            source_frame_depth: 0,
            accumulator: Some(initial),
            awaiting_element: None,
            awaiting_acc: false,
        });
        // B1a: the first element may itself be an accessor (getter runs).
        match crate::vm::array_element_value(vm, gc, this, first) {
            crate::vm::ArrayElemOut::Ready(element) => {
                vm.push_callback_call(
                    gc,
                    callback,
                    Value::undefined(),
                    vec![initial, element, Value::smi(first as i32), this],
                );
            }
            crate::vm::ArrayElemOut::Wait => {
                if let Some(ref mut op) = vm.pending_array_op {
                    op.awaiting_element = Some(first);
                }
                vm.rebase_pending_depths();
            }
            crate::vm::ArrayElemOut::SyncErr(e) => {
                vm.pending_array_op = None;
                vm.set_pending_exception(e);
            }
        }
        return Value::undefined();
    }
    // No initial value: the accumulator is the first present element
    // (resolved through getters like any element); without any present
    // element, throw. The next index is precomputed so the await-resume
    // path can finish setup.
    let Some(first) = first_existing_index(this, length) else {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "reduce of empty array with no initial value",
        ));
        return Value::undefined();
    };
    let next = next_existing_index(this, first + 1, length);
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Reduce,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: Value::undefined(),
        source_val: this,
        index: next.unwrap_or(usize::MAX),
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    match crate::vm::array_element_value(vm, gc, this, first) {
        crate::vm::ArrayElemOut::Ready(acc) => {
            let mut op = vm.pending_array_op.take().unwrap();
            op.accumulator = Some(acc);
            match next {
                Some(n) => {
                    // Resolve the first iterated element (may itself await).
                    match crate::vm::array_element_value(vm, gc, this, n) {
                        crate::vm::ArrayElemOut::Ready(element) => {
                            op.index = n;
                            vm.pending_array_op = Some(op);
                            vm.push_callback_call(
                                gc,
                                callback,
                                Value::undefined(),
                                vec![acc, element, Value::smi(n as i32), this],
                            );
                        }
                        crate::vm::ArrayElemOut::Wait => {
                            op.awaiting_element = Some(n);
                            vm.pending_array_op = Some(op);
                            vm.rebase_pending_depths();
                        }
                        crate::vm::ArrayElemOut::SyncErr(e) => {
                            vm.set_pending_exception(e);
                        }
                    }
                }
                // Single present element and a sync accumulator: done.
                None => {
                    vm.pending_array_op = None;
                    return acc;
                }
            }
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(first);
                op.awaiting_acc = true;
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
    Value::undefined()
}

/// Array.prototype.reduceRight(callback, initialValue) — backward mirror
/// of reduce (B1c): seeds from the last present element, iterates down.
pub fn array_reduce_right(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some((length, callback, _, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.reduceRight")
    else {
        return Value::undefined();
    };
    let has_initial = args.len() > 1;
    let initial = args.get(1).copied().unwrap_or(Value::undefined());
    if has_initial {
        let Some(last) = last_existing_index(this, length) else {
            return initial;
        };
        vm.pending_array_op = Some(crate::vm::ArrayOpState {
            kind: crate::vm::ArrayOpKind::ReduceRight,
            source: source_ptr,
            result: std::ptr::null_mut(),
            callback,
            this_val: Value::undefined(),
            source_val: this,
            index: last,
            length,
            source_frame_depth: 0,
            accumulator: Some(initial),
            awaiting_element: None,
            awaiting_acc: false,
        });
        match crate::vm::array_element_value(vm, gc, this, last) {
            crate::vm::ArrayElemOut::Ready(element) => {
                vm.push_callback_call(
                    gc,
                    callback,
                    Value::undefined(),
                    vec![initial, element, Value::smi(last as i32), this],
                );
            }
            crate::vm::ArrayElemOut::Wait => {
                if let Some(ref mut op) = vm.pending_array_op {
                    op.awaiting_element = Some(last);
                }
                vm.rebase_pending_depths();
            }
            crate::vm::ArrayElemOut::SyncErr(e) => {
                vm.pending_array_op = None;
                vm.set_pending_exception(e);
            }
        }
        return Value::undefined();
    }
    // No initial value: seed from the last present element (resolved
    // through getters like any element), then iterate downward.
    let Some(last) = last_existing_index(this, length) else {
        vm.set_pending_exception(crate::errors::error_object(
            gc,
            &vm.error_protos,
            crate::errors::ErrorKind::TypeError,
            "reduce of empty array with no initial value",
        ));
        return Value::undefined();
    };
    let prev = prev_existing_index(this, last);
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::ReduceRight,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: Value::undefined(),
        source_val: this,
        index: prev.unwrap_or(usize::MAX),
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    match crate::vm::array_element_value(vm, gc, this, last) {
        crate::vm::ArrayElemOut::Ready(acc) => {
            let mut op = vm.pending_array_op.take().unwrap();
            op.accumulator = Some(acc);
            match prev {
                Some(n) => match crate::vm::array_element_value(vm, gc, this, n) {
                    crate::vm::ArrayElemOut::Ready(element) => {
                        op.index = n;
                        vm.pending_array_op = Some(op);
                        vm.push_callback_call(
                            gc,
                            callback,
                            Value::undefined(),
                            vec![acc, element, Value::smi(n as i32), this],
                        );
                    }
                    crate::vm::ArrayElemOut::Wait => {
                        op.awaiting_element = Some(n);
                        vm.pending_array_op = Some(op);
                        vm.rebase_pending_depths();
                    }
                    crate::vm::ArrayElemOut::SyncErr(e) => {
                        vm.set_pending_exception(e);
                    }
                },
                None => {
                    vm.pending_array_op = None;
                    return acc;
                }
            }
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(last);
                op.awaiting_acc = true;
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
    Value::undefined()
}

/// Array.prototype.findLast(callback, thisArg) — backward find (B1c).
pub fn array_find_last(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some((length, callback, this_arg, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.findLast")
    else {
        return Value::undefined();
    };
    let Some(last) = last_existing_index(this, length) else {
        return Value::undefined();
    };
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::FindLast,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: last,
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    match crate::vm::array_element_value(vm, gc, this, last) {
        crate::vm::ArrayElemOut::Ready(element) => {
            vm.push_callback_call(
                gc,
                callback,
                this_arg,
                vec![element, Value::smi(last as i32), this],
            );
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(last);
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
    Value::undefined()
}

/// Array.prototype.findLastIndex(callback, thisArg) — backward findIndex (B1c).
pub fn array_find_last_index(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    let Some((length, callback, this_arg, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.findLastIndex")
    else {
        return Value::smi(-1);
    };
    let Some(last) = last_existing_index(this, length) else {
        return Value::smi(-1);
    };
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::FindLastIndex,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: last,
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    match crate::vm::array_element_value(vm, gc, this, last) {
        crate::vm::ArrayElemOut::Ready(element) => {
            vm.push_callback_call(
                gc,
                callback,
                this_arg,
                vec![element, Value::smi(last as i32), this],
            );
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(last);
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
    Value::undefined()
}

/// Array.prototype.find(callback, thisArg) — set up state machine iteration.
pub fn array_find(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some((length, callback, this_arg, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.find")
    else {
        return Value::undefined();
    };
    let Some(first) = first_existing_index(this, length) else {
        return Value::undefined();
    };
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Find,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: first,
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    // B1a: accessor elements dispatch their getter (Wait records the
    // await; SyncErr routes through the normal pending-exception path).
    match crate::vm::array_element_value(vm, gc, this, first) {
        crate::vm::ArrayElemOut::Ready(element) => {
            vm.push_callback_call(
                gc,
                callback,
                this_arg,
                vec![element, Value::smi(first as i32), this],
            );
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(first);
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
    Value::undefined()
}

/// Array.prototype.findIndex(callback, thisArg) — set up state machine iteration.
pub fn array_find_index(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some((length, callback, this_arg, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.findIndex")
    else {
        return Value::smi(-1);
    };
    let Some(first) = first_existing_index(this, length) else {
        return Value::smi(-1);
    };
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::FindIndex,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: first,
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    // B1a: accessor elements dispatch their getter (Wait records the
    // await; SyncErr routes through the normal pending-exception path).
    match crate::vm::array_element_value(vm, gc, this, first) {
        crate::vm::ArrayElemOut::Ready(element) => {
            vm.push_callback_call(
                gc,
                callback,
                this_arg,
                vec![element, Value::smi(first as i32), this],
            );
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(first);
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
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
                        // B1e: holes read as undefined (Get semantics).
                        let flat_elem = if flat_elem == Value::empty_sentinel() {
                            Value::undefined()
                        } else {
                            flat_elem
                        };
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

/// ================= B1e: sort / toSorted =================
///
/// Array.prototype.sort + Array.prototype.toSorted (§23.1.3.30/.34) on the
/// PendingSortOp machine (vm.rs): snapshot reads (SortIndexedProperties —
/// HasProperty+Get per index with getter dispatch), stable bottom-up merge
/// with one comparator round-trip per comparison, then writeback with
/// setter dispatch (sort) or into a fresh dense array (toSorted).
/// Comparator/length/key errors follow spec order; the Call skip-list,
/// rebase, rooting and unwind-drop sites treat the machine like the
/// array-iteration one (F4).
/// Outcome of one sort-machine step.
pub(crate) enum SortStepOut {
    /// Made synchronous progress; the driver loops again.
    Progress,
    /// A JS frame was pushed; store the op and bail (Call skip-list owns pc).
    Wait,
    /// Sort complete with the final value.
    Done(Value),
    /// Raise this error value (setup: pending_exception; Return: handle_throw).
    Raise(Value),
}

/// Record the just-pushed frame's callee as this machine's awaited
/// callee (the Return arm only fires on a callee match — nested foreign
/// pushes share depths after rebase but never callees).
fn sort_arm_await(vm: &Vm, sop: &mut crate::vm::PendingSortOp) {
    sop.await_callee = vm.last_pushed_callee;
}

/// ToPrimitive outcome for sort keys/lengths.
enum SortPrimOut {
    Ready(Value),
    Wait(Value, Value),
    Raise(Value),
}

fn sort_type_error(gc: &mut SemiSpace, vm: &Vm, msg: &str) -> Value {
    crate::errors::error_object(
        gc,
        &vm.error_protos,
        crate::errors::ErrorKind::TypeError,
        msg,
    )
}

fn sort_range_error(gc: &mut SemiSpace, vm: &Vm, msg: &str) -> Value {
    crate::errors::error_object(
        gc,
        &vm.error_protos,
        crate::errors::ErrorKind::RangeError,
        msg,
    )
}

/// Primitive for ToPrimitive purposes (heap strings count as primitive).
/// Shared by the sort machine and the pending_call ToPrimitive resume.
pub(crate) fn toprim_is_primitive(v: Value) -> bool {
    if !v.is_heap_object() {
        return true;
    }
    let tag = unsafe { (*(v.heap_ptr().unwrap() as *const GcHeader)).tag() };
    tag == TAG_STRING || tag == TAG_STRING_OBJ
}

/// ToPrimitive(value, hint) with synchronous builtin dispatch. `tried_other`
/// selects the second method (after the first returned non-primitive). Raw
/// method lookup: an accessor in method position counts as absent
/// (sync-gap policy, documented).
fn sort_to_prim(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    value: Value,
    string_hint: bool,
    tried_other: bool,
) -> SortPrimOut {
    if toprim_is_primitive(value) {
        return SortPrimOut::Ready(value);
    }
    let tag = unsafe { (*(value.heap_ptr().unwrap() as *const GcHeader)).tag() };
    // Dates default to the string hint (matches value_to_js_string).
    let string_first = string_hint || tag == TAG_DATE;
    let name = match (string_first, tried_other) {
        (true, false) => "toString",
        (false, false) => "valueOf",
        (true, true) => "valueOf",
        (false, true) => "toString",
    };
    let name_val = Value::from_heap_ptr(HeapString::allocate(gc, name) as *mut u8);
    let method = crate::vm::load_property_recursive(value, name_val, None, gc);
    if let Some(smi) = method.as_smi() {
        if smi < 0 {
            let id = ((-smi) as usize) - 1;
            if id < vm.builtins.len() {
                let result = (vm.builtins[id].func)(gc, value, &[], vm);
                if let Some(exc) = vm.pending_exception.take() {
                    return SortPrimOut::Raise(exc);
                }
                if toprim_is_primitive(result) {
                    return SortPrimOut::Ready(result);
                }
                if !tried_other {
                    return sort_to_prim(vm, gc, value, string_hint, true);
                }
                return SortPrimOut::Raise(sort_type_error(
                    gc,
                    vm,
                    "Cannot convert object to primitive value",
                ));
            }
        }
    }
    if method
        .heap_ptr()
        .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_FUNC)
    {
        return SortPrimOut::Wait(method, value);
    }
    if !tried_other {
        return sort_to_prim(vm, gc, value, string_hint, true);
    }
    SortPrimOut::Raise(sort_type_error(
        gc,
        vm,
        "Cannot convert object to primitive value",
    ))
}

/// Default-comparator key of an already-primitive value: UTF-16 code units
/// (spec CompareArrayElements compares code units — byte order differs for
/// astral text). ToString(symbol) throws (§7.1.19).
fn sort_key_of_primitive(v: Value) -> Result<Vec<u16>, ()> {
    if v.is_symbol() {
        return Err(());
    }
    Ok(string_from_value(v).encode_utf16().collect())
}

/// ToLength of a primitive for sort lengths (§7.1.22; callers convert
/// objects via sort_to_prim first). Symbol → Err (ToNumber abrupt).
fn sort_length_of_primitive(v: Value) -> Result<u64, ()> {
    if v.is_symbol() {
        return Err(());
    }
    let n = crate::vm::to_number(v);
    if n.is_nan() || n <= 0.0 {
        return Ok(0);
    }
    Ok(n.min(9_007_199_254_740_991.0) as u64)
}

/// LengthOfArrayLike value step shared by data lengths and length-getter
/// results (B1e). May arm a valueOf await (Wait).
fn sort_length_of_value(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    sop: &mut crate::vm::PendingSortOp,
    v: Value,
) -> SortStepOut {
    if v.is_symbol() {
        return SortStepOut::Raise(sort_type_error(
            gc,
            vm,
            "Cannot convert a Symbol value to a number",
        ));
    }
    if toprim_is_primitive(v) {
        match sort_length_of_primitive(v) {
            Ok(len) => {
                sop.len = len;
                sop.len_pending = false;
                SortStepOut::Progress
            }
            Err(()) => SortStepOut::Raise(sort_type_error(
                gc,
                vm,
                "Cannot convert a Symbol value to a number",
            )),
        }
    } else {
        match sort_to_prim(vm, gc, v, false, false) {
            SortPrimOut::Ready(p) => {
                if p.is_symbol() {
                    return SortStepOut::Raise(sort_type_error(
                        gc,
                        vm,
                        "Cannot convert a Symbol value to a number",
                    ));
                }
                match sort_length_of_primitive(p) {
                    Ok(len) => {
                        sop.len = len;
                        sop.len_pending = false;
                        SortStepOut::Progress
                    }
                    Err(()) => SortStepOut::Raise(sort_type_error(
                        gc,
                        vm,
                        "Cannot convert a Symbol value to a number",
                    )),
                }
            }
            SortPrimOut::Wait(f, recv) => {
                crate::vm::push_accessor_frame(vm, f, recv, None);
                sort_arm_await(vm, sop);
                sop.await_prim = true;
                sop.prim_value = v;
                sop.prim_tried_other = false;
                sop.prim_purpose = crate::vm::SortPrimPurpose::Length;
                SortStepOut::Wait
            }
            SortPrimOut::Raise(e) => SortStepOut::Raise(e),
        }
    }
}

/// LengthOfArrayLike for sort/toSorted: fixed once, symbol lengths throw,
/// object lengths coerce via valueOf, length getters dispatch. Sets sop.len
/// or arms a prim await (Wait).
fn sort_begin_length(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    sop: &mut crate::vm::PendingSortOp,
) -> SortStepOut {
    let obj = sop.source_val;
    // Dense arrays: magic length, always data.
    if let Some(ptr) = obj.heap_ptr() {
        if unsafe { (*(ptr as *const GcHeader)).tag() } == TAG_ARRAY {
            sop.len = unsafe { RuneArray::length(ptr as *mut RuneArray) } as u64;
            return SortStepOut::Progress;
        }
    }
    let name_val = Value::from_heap_ptr(HeapString::allocate(gc, "length") as *mut u8);
    let raw = crate::vm::load_property_recursive(obj, name_val, None, gc);
    if let Some(aptr) = raw.heap_ptr() {
        if unsafe { (*(aptr as *const GcHeader)).tag() } == TAG_ACCESSOR {
            let getter = unsafe { rune_core::accessor::AccessorPair::getter(aptr) };
            if getter.is_undefined() || getter.is_null() {
                sop.len = 0;
                return SortStepOut::Progress;
            }
            if let Some(smi) = getter.as_smi() {
                if smi < 0 {
                    let id = ((-smi) as usize) - 1;
                    if id < vm.builtins.len() {
                        let result = (vm.builtins[id].func)(gc, obj, &[], vm);
                        if let Some(exc) = vm.pending_exception.take() {
                            return SortStepOut::Raise(exc);
                        }
                        return sort_length_of_value(vm, gc, sop, result);
                    }
                }
                sop.len = 0;
                return SortStepOut::Progress;
            }
            if getter
                .heap_ptr()
                .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_FUNC)
            {
                crate::vm::push_accessor_frame(vm, getter, obj, None);
                sort_arm_await(vm, sop);
                sop.await_prim = true;
                sop.prim_value = obj;
                sop.prim_tried_other = false;
                sop.prim_purpose = crate::vm::SortPrimPurpose::LengthGet;
                return SortStepOut::Wait;
            }
            sop.len = 0;
            return SortStepOut::Progress;
        }
    }
    sort_length_of_value(vm, gc, sop, raw)
}

/// Resume a ToPrimitive round-trip (length coercion or string-key).
fn sort_prim_resume(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    sop: &mut crate::vm::PendingSortOp,
    result: Value,
) -> SortStepOut {
    let purpose = std::mem::replace(&mut sop.prim_purpose, crate::vm::SortPrimPurpose::Length);
    match purpose {
        crate::vm::SortPrimPurpose::LengthGet => sort_length_of_value(vm, gc, sop, result),
        crate::vm::SortPrimPurpose::Length => {
            if !toprim_is_primitive(result) {
                if !sop.prim_tried_other {
                    match sort_to_prim(vm, gc, sop.prim_value, false, true) {
                        SortPrimOut::Ready(p) => {
                            return sort_length_of_value(vm, gc, sop, p);
                        }
                        SortPrimOut::Wait(f, recv) => {
                            crate::vm::push_accessor_frame(vm, f, recv, None);
                            sort_arm_await(vm, sop);
                            sop.await_prim = true;
                            sop.prim_tried_other = true;
                            sop.prim_purpose = crate::vm::SortPrimPurpose::Length;
                            return SortStepOut::Wait;
                        }
                        SortPrimOut::Raise(e) => return SortStepOut::Raise(e),
                    }
                }
                return SortStepOut::Raise(sort_type_error(
                    gc,
                    vm,
                    "Cannot convert object to primitive value",
                ));
            }
            sort_length_of_value(vm, gc, sop, result)
        }
        crate::vm::SortPrimPurpose::Key { is_left } => {
            if !toprim_is_primitive(result) {
                if !sop.prim_tried_other {
                    match sort_to_prim(vm, gc, sop.prim_value, true, true) {
                        SortPrimOut::Ready(p) => {
                            return sort_key_resume(gc, vm, sop, is_left, p);
                        }
                        SortPrimOut::Wait(f, recv) => {
                            crate::vm::push_accessor_frame(vm, f, recv, None);
                            sort_arm_await(vm, sop);
                            sop.await_prim = true;
                            sop.prim_tried_other = true;
                            sop.prim_purpose = crate::vm::SortPrimPurpose::Key { is_left };
                            return SortStepOut::Wait;
                        }
                        SortPrimOut::Raise(e) => return SortStepOut::Raise(e),
                    }
                }
                return SortStepOut::Raise(sort_type_error(
                    gc,
                    vm,
                    "Cannot convert object to primitive value",
                ));
            }
            sort_key_resume(gc, vm, sop, is_left, result)
        }
    }
}

/// Store a resolved key primitive for one merge side.
fn sort_key_resume(
    gc: &mut SemiSpace,
    vm: &mut Vm,
    sop: &mut crate::vm::PendingSortOp,
    is_left: bool,
    p: Value,
) -> SortStepOut {
    match sort_key_of_primitive(p) {
        Ok(k) => {
            let idx = if is_left { sop.i } else { sop.j };
            sop.items[idx].key = Some(k);
            SortStepOut::Progress
        }
        Err(()) => SortStepOut::Raise(sort_type_error(
            gc,
            vm,
            "Cannot convert a Symbol value to a string",
        )),
    }
}

/// Resolve (and cache) the default-comparator key for one merge side.
/// Keys are computed lazily: with 0/1 elements no ToString ever runs.
fn sort_key_step(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    sop: &mut crate::vm::PendingSortOp,
    is_left: bool,
) -> SortStepOut {
    let idx = if is_left { sop.i } else { sop.j };
    if sop.items[idx].key.is_some() {
        return SortStepOut::Progress;
    }
    let value = sop.items[idx].value;
    match sort_to_prim(vm, gc, value, true, false) {
        SortPrimOut::Ready(p) => sort_key_resume(gc, vm, sop, is_left, p),
        SortPrimOut::Wait(f, recv) => {
            crate::vm::push_accessor_frame(vm, f, recv, None);
            sort_arm_await(vm, sop);
            sop.await_prim = true;
            sop.prim_value = value;
            sop.prim_tried_other = false;
            sop.prim_purpose = crate::vm::SortPrimPurpose::Key { is_left };
            SortStepOut::Wait
        }
        SortPrimOut::Raise(e) => SortStepOut::Raise(e),
    }
}

/// Comparator result → f64 (ToNumber; NaN → +0 per CompareArrayElements;
/// symbol → abrupt).
fn sort_cmp_number(v: Value) -> Result<f64, ()> {
    if v.is_symbol() {
        return Err(());
    }
    let n = crate::vm::to_number(v);
    Ok(if n.is_nan() { 0.0 } else { n })
}

/// Place one merge element by ordering (Equal takes LEFT — stability).
fn sort_merge_place(sop: &mut crate::vm::PendingSortOp, ord: std::cmp::Ordering) {
    let take_left = !matches!(ord, std::cmp::Ordering::Greater);
    let from = if take_left { sop.i } else { sop.j };
    let moved = std::mem::replace(
        &mut sop.items[from],
        crate::vm::SortItem {
            value: Value::undefined(),
            key: None,
        },
    );
    sop.aux[sop.k] = moved;
    if take_left {
        sop.i += 1;
    } else {
        sop.j += 1;
    }
    sop.k += 1;
    if sop.k >= sop.pair_end {
        sop.base = sop.pair_end;
        if !sort_advance(sop) {
            sop.phase = crate::vm::SortPhase::Write;
            sop.write_idx = 0;
        }
    }
}

/// Set up the next merge pair (or next pass). False = sorting complete.
fn sort_advance(sop: &mut crate::vm::PendingSortOp) -> bool {
    let n = sop.items.len();
    loop {
        if sop.base >= n {
            std::mem::swap(&mut sop.items, &mut sop.aux);
            sop.width *= 2;
            if sop.width >= n {
                return false;
            }
            sop.base = 0;
        }
        let mid = (sop.base + sop.width).min(n);
        let end = (sop.base + 2 * sop.width).min(n);
        if mid >= end {
            // No right run: carry the left run over verbatim.
            for t in sop.base..mid {
                let moved = std::mem::replace(
                    &mut sop.items[t],
                    crate::vm::SortItem {
                        value: Value::undefined(),
                        key: None,
                    },
                );
                sop.aux[t] = moved;
            }
            sop.base = end;
            continue;
        }
        sop.i = sop.base;
        sop.j = mid;
        sop.k = sop.base;
        sop.left_end = mid;
        sop.right_end = end;
        sop.pair_end = end;
        return true;
    }
}

/// One merge comparison (or exhaustion drain): places exactly one element
/// synchronously, or pushes a comparator/key frame (Wait).
fn sort_compare_step(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    sop: &mut crate::vm::PendingSortOp,
) -> SortStepOut {
    let x = sop.items[sop.i].value;
    let y = sop.items[sop.j].value;
    if !sop.comparator.is_undefined() {
        // CompareArrayElements steps 1-3 precede the comparator Call:
        // undefined sorts after everything without invoking it.
        let xu = x.is_undefined();
        let yu = y.is_undefined();
        if xu && yu {
            sort_merge_place(sop, std::cmp::Ordering::Equal);
            return SortStepOut::Progress;
        } else if xu {
            sort_merge_place(sop, std::cmp::Ordering::Greater);
            return SortStepOut::Progress;
        } else if yu {
            sort_merge_place(sop, std::cmp::Ordering::Less);
            return SortStepOut::Progress;
        }
        let cmp = sop.comparator;
        if let Some(smi) = cmp.as_smi() {
            if smi < 0 {
                let id = ((-smi) as usize) - 1;
                if id < vm.builtins.len() {
                    let result = (vm.builtins[id].func)(gc, Value::undefined(), &[x, y], vm);
                    if let Some(exc) = vm.pending_exception.take() {
                        return SortStepOut::Raise(exc);
                    }
                    match sort_cmp_number(result) {
                        Ok(n) => {
                            sort_merge_place(
                                sop,
                                n.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal),
                            );
                            return SortStepOut::Progress;
                        }
                        Err(()) => {
                            return SortStepOut::Raise(sort_type_error(
                                gc,
                                vm,
                                "Cannot convert a Symbol value to a number",
                            ));
                        }
                    }
                }
            }
            return SortStepOut::Raise(sort_type_error(
                gc,
                vm,
                "sort comparator is not a function",
            ));
        }
        if cmp
            .heap_ptr()
            .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() } == TAG_FUNC)
        {
            vm.push_callback_call(gc, cmp, Value::undefined(), vec![x, y]);
            sort_arm_await(vm, sop);
            sop.await_cmp = true;
            return SortStepOut::Wait;
        }
        return SortStepOut::Raise(sort_type_error(gc, vm, "sort comparator is not a function"));
    }
    // Default comparator: undefined sorts last (CompareArrayElements 1-3),
    // else UTF-16 lexicographic by lazily resolved keys.
    let xu = x.is_undefined();
    let yu = y.is_undefined();
    if xu && yu {
        sort_merge_place(sop, std::cmp::Ordering::Equal);
    } else if xu {
        sort_merge_place(sop, std::cmp::Ordering::Greater);
    } else if yu {
        sort_merge_place(sop, std::cmp::Ordering::Less);
    } else {
        match sort_key_step(vm, gc, sop, true) {
            SortStepOut::Progress => {}
            other => return other,
        }
        match sort_key_step(vm, gc, sop, false) {
            SortStepOut::Progress => {}
            other => return other,
        }
        let ord = sop.items[sop.i]
            .key
            .as_ref()
            .unwrap()
            .cmp(sop.items[sop.j].key.as_ref().unwrap());
        sort_merge_place(sop, ord);
    }
    SortStepOut::Progress
}

/// One merge step: drain exhausted runs, then compare.
fn sort_merge_step(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    sop: &mut crate::vm::PendingSortOp,
) -> SortStepOut {
    loop {
        if sop.k >= sop.pair_end {
            sop.base = sop.pair_end;
            if !sort_advance(sop) {
                sop.phase = crate::vm::SortPhase::Write;
                sop.write_idx = 0;
            }
            return SortStepOut::Progress;
        }
        if sop.i < sop.left_end && sop.j < sop.right_end {
            break;
        }
        // One side exhausted: carry the other over (no comparison).
        let from = if sop.i >= sop.left_end {
            let t = sop.j;
            sop.j += 1;
            t
        } else {
            let t = sop.i;
            sop.i += 1;
            t
        };
        let moved = std::mem::replace(
            &mut sop.items[from],
            crate::vm::SortItem {
                value: Value::undefined(),
                key: None,
            },
        );
        sop.aux[sop.k] = moved;
        sop.k += 1;
    }
    sort_compare_step(vm, gc, sop)
}

/// Snapshot reads (SortIndexedProperties): HasProperty+Get per index with
/// getter dispatch; toSorted reads through holes. Length fixed at entry.
fn sort_read_step(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    sop: &mut crate::vm::PendingSortOp,
) -> SortStepOut {
    let walk = sop.len.min(u32::MAX as u64);
    while sop.read_idx < walk {
        let idx = sop.read_idx;
        if !sop.read_all && !crate::vm::has_property(sop.source_val, Value::smi(idx as i32), None) {
            sop.read_idx += 1;
            continue;
        }
        match crate::vm::array_element_value(vm, gc, sop.source_val, idx as usize) {
            crate::vm::ArrayElemOut::Ready(v) => {
                sop.items.push(crate::vm::SortItem {
                    value: v,
                    key: None,
                });
                sop.read_idx += 1;
            }
            crate::vm::ArrayElemOut::Wait => {
                sop.await_read = true;
                sort_arm_await(vm, sop);
                return SortStepOut::Wait;
            }
            crate::vm::ArrayElemOut::SyncErr(e) => return SortStepOut::Raise(e),
        }
    }
    let n = sop.items.len();
    if n > 1 {
        sop.aux = (0..n)
            .map(|_| crate::vm::SortItem {
                value: Value::undefined(),
                key: None,
            })
            .collect();
        sop.width = 1;
        sop.base = 0;
        sop.phase = crate::vm::SortPhase::Merge;
        sort_advance(sop);
    } else {
        sop.phase = crate::vm::SortPhase::Write;
        sop.write_idx = 0;
    }
    SortStepOut::Progress
}

/// Setter-scan outcome for one sort write.
enum SortStoreOut {
    Done,
    WaitSetter(Value),
    Raise(Value),
}

/// Find the own-or-inherited accessor pair governing a sort write index
/// (dense overlay for arrays, shape slots for objects, then the proto
/// chain — tags guarded, unlike the legacy funnel loop).
fn sort_find_accessor(gc: &mut SemiSpace, obj: Value, idx: u64) -> Option<Value> {
    let mut current = obj;
    let mut depth = 0;
    loop {
        if depth >= crate::vm::MAX_PROTOTYPE_DEPTH {
            return None;
        }
        depth += 1;
        let ptr = current.heap_ptr()?;
        let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
        if tag == TAG_ARRAY {
            if let Some(pair) = crate::vm::array_overlay_accessor(
                ptr as *mut RuneArray,
                idx.min(usize::MAX as u64) as usize,
            ) {
                return Some(pair);
            }
            let proto = unsafe { JSObject::prototype(ptr as *mut JSObject) };
            if proto.is_null() {
                return None;
            }
            current = Value::from_heap_ptr(proto);
        } else if tag == TAG_OBJECT {
            if let Some(key) = crate::vm::value_to_prop_key(index_key(gc, idx)) {
                let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
                if let Some(slot) = shape.lookup(&key) {
                    let v = unsafe { JSObject::get_slot(ptr as *mut JSObject, slot) };
                    if v.heap_ptr().is_some_and(|vp| unsafe {
                        (*(vp as *const GcHeader)).tag() == TAG_ACCESSOR
                    }) {
                        return Some(v);
                    }
                }
            }
            let proto = unsafe { JSObject::prototype(ptr as *mut JSObject) };
            if proto.is_null() {
                return None;
            }
            current = Value::from_heap_ptr(proto);
        } else {
            return None;
        }
    }
}

/// One mutating write for sort writeback (spec Set with throw=true):
/// setter dispatch (builtin inline, JS via frame), else a raw store with
/// strict failure reporting. Never calls handle_throw (setup-safe).
fn sort_store_one(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    obj: Value,
    idx: usize,
    val: Value,
) -> SortStoreOut {
    let key = index_key(gc, idx as u64);
    match classify_store(gc, obj, idx) {
        StoreTarget::Data => {
            if crate::vm::do_store_property(obj, key, val, gc, vm) {
                SortStoreOut::Done
            } else {
                // Strict Set failure. SameValue stores succeed silently (spec
                // ValidateAndApply); anything else throws.
                let cur = crate::vm::load_property_recursive(obj, key, None, gc);
                if same_value(cur, val) {
                    SortStoreOut::Done
                } else {
                    SortStoreOut::Raise(sort_type_error(
                        gc,
                        vm,
                        "Cannot assign to read-only property",
                    ))
                }
            }
        }
        StoreTarget::SetterBuiltin(setter) => {
            let id = ((-setter.as_smi().unwrap()) as usize) - 1;
            if id < vm.builtins.len() {
                (vm.builtins[id].func)(gc, obj, &[val], vm);
                if let Some(exc) = vm.pending_exception.take() {
                    return SortStoreOut::Raise(exc);
                }
                return SortStoreOut::Done;
            }
            SortStoreOut::Raise(sort_type_error(gc, vm, "setter is not a function"))
        }
        StoreTarget::SetterJs(setter) => SortStoreOut::WaitSetter(setter),
        StoreTarget::GetterOnly => SortStoreOut::Raise(sort_type_error(
            gc,
            vm,
            "Cannot set property with only a getter",
        )),
        StoreTarget::SetterInvalid => {
            SortStoreOut::Raise(sort_type_error(gc, vm, "setter is not a function"))
        }
    }
}

/// One trailing delete for sort writeback (DeletePropertyOrThrow):
/// dense arrays punch holes, objects remove configurable props.
fn sort_delete_one(gc: &mut SemiSpace, vm: &Vm, obj: Value, idx: u64) -> Result<(), Value> {
    let ptr = match obj.heap_ptr() {
        Some(p) => p,
        None => return Ok(()),
    };
    let tag = unsafe { (*(ptr as *const GcHeader)).tag() };
    if tag == TAG_ARRAY {
        let len = unsafe { RuneArray::length(ptr as *mut RuneArray) } as u64;
        let cap = unsafe { RuneArray::capacity(ptr as *mut RuneArray) } as u64;
        if idx < len && idx < cap {
            unsafe {
                RuneArray::set_element(ptr as *mut RuneArray, idx as usize, Value::empty_sentinel())
            };
        }
        Ok(())
    } else if tag == TAG_OBJECT {
        if let Some(key) = crate::vm::value_to_prop_key(index_key(gc, idx)) {
            let shape = unsafe { JSObject::shape_ptr(ptr as *mut JSObject) };
            if let Some(s) = shape.lookup(&key) {
                if shape.attr_at(s) & rune_core::shape::ATTR_CONFIGURABLE == 0 {
                    return Err(sort_type_error(
                        gc,
                        vm,
                        "Cannot delete a non-configurable property",
                    ));
                }
                unsafe { JSObject::remove_property(ptr as *mut JSObject, &key) };
            }
        }
        Ok(())
    } else {
        Ok(())
    }
}

/// Writeback: toSorted densifies into a fresh array; sort Sets each present
/// index (setter dispatch) then deletes the trailing range.
fn sort_write_step(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    sop: &mut crate::vm::PendingSortOp,
) -> SortStepOut {
    if sop.is_copy {
        let vals: Vec<Value> = sop.items.iter().map(|it| it.value).collect();
        return SortStepOut::Done(build_array(gc, &vals, vm));
    }
    while sop.write_idx < sop.items.len() {
        let idx = sop.write_idx;
        let val = sop.items[idx].value;
        match sort_store_one(vm, gc, sop.source_val, idx, val) {
            SortStoreOut::Done => sop.write_idx += 1,
            SortStoreOut::WaitSetter(setter) => {
                crate::vm::push_accessor_frame(vm, setter, sop.source_val, Some(val));
                sort_arm_await(vm, sop);
                sop.await_set = true;
                return SortStepOut::Wait;
            }
            SortStoreOut::Raise(e) => return SortStepOut::Raise(e),
        }
    }
    let n = sop.items.len() as u64;
    let walk = sop.len.min(u32::MAX as u64);
    let mut j = n;
    while j < walk {
        if let Err(e) = sort_delete_one(gc, vm, sop.source_val, j) {
            return SortStepOut::Raise(e);
        }
        j += 1;
    }
    SortStepOut::Done(sop.source_val)
}

/// Drive the machine synchronously until it waits, finishes, or raises.
/// Never returns Progress (loops internally).
pub(crate) fn sort_drive(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    sop: &mut crate::vm::PendingSortOp,
) -> SortStepOut {
    loop {
        if sop.len_pending {
            match sort_begin_length(vm, gc, sop) {
                SortStepOut::Progress => {
                    // toSorted ArrayCreates up front: the RangeError precedes
                    // every element Get (length-exceeding-array-length-limit).
                    if sop.is_copy && sop.len > u32::MAX as u64 {
                        return SortStepOut::Raise(sort_range_error(
                            gc,
                            vm,
                            "Invalid array length",
                        ));
                    }
                    sop.len_pending = false;
                }
                other => return other,
            }
        }
        match sop.phase {
            crate::vm::SortPhase::Read => match sort_read_step(vm, gc, sop) {
                SortStepOut::Progress => {}
                other => return other,
            },
            crate::vm::SortPhase::Merge => match sort_merge_step(vm, gc, sop) {
                SortStepOut::Progress => {}
                other => return other,
            },
            crate::vm::SortPhase::Write => return sort_write_step(vm, gc, sop),
        }
    }
}

/// Resume the machine with a JS frame's return value (getter element,
/// ToPrimitive value, comparator number, or ignored setter result).
pub(crate) fn sort_resume(
    vm: &mut Vm,
    gc: &mut SemiSpace,
    sop: &mut crate::vm::PendingSortOp,
    result: Value,
) -> SortStepOut {
    if sop.await_read {
        sop.await_read = false;
        sop.items.push(crate::vm::SortItem {
            value: result,
            key: None,
        });
        sop.read_idx += 1;
        SortStepOut::Progress
    } else if sop.await_cmp {
        sop.await_cmp = false;
        match sort_cmp_number(result) {
            Ok(n) => {
                sort_merge_place(
                    sop,
                    n.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal),
                );
                SortStepOut::Progress
            }
            Err(()) => SortStepOut::Raise(sort_type_error(
                gc,
                vm,
                "Cannot convert a Symbol value to a number",
            )),
        }
    } else if sop.await_set {
        sop.await_set = false;
        sop.write_idx += 1;
        SortStepOut::Progress
    } else if sop.await_prim {
        sop.await_prim = false;
        sort_prim_resume(vm, gc, sop, result)
    } else {
        // No await armed — a foreign nested return at our depth. Re-drive
        // (every step advances or terminates, so this cannot spin).
        SortStepOut::Progress
    }
}

/// Shared sort/toSorted entry: spec-order comparator check, RequireObject-
/// Coercible, then the machine. `is_copy` selects toSorted writeback.
fn array_sort_entry(
    gc: &mut SemiSpace,
    this: Value,
    args: &[Value],
    vm: &mut Vm,
    is_copy: bool,
) -> Value {
    // Step 1 precedes ToObject/LengthOfArrayLike (comparefn-not-a-function).
    let comparator = args.first().copied().unwrap_or(Value::undefined());
    if !comparator.is_undefined() && !is_callable_value(comparator) {
        vm.set_pending_exception(sort_type_error(
            gc,
            vm,
            "The comparison function must be either a function or undefined",
        ));
        return Value::undefined();
    }
    if !require_object_coercible(this, vm, gc) {
        return Value::undefined();
    }
    let mut sop = crate::vm::PendingSortOp {
        source_frame_depth: 0,
        source_val: this,
        write_val: Value::undefined(),
        comparator,
        is_copy,
        read_all: is_copy,
        len: 0,
        len_pending: true,
        phase: crate::vm::SortPhase::Read,
        read_idx: 0,
        await_read: false,
        await_prim: false,
        prim_value: Value::undefined(),
        prim_tried_other: false,
        prim_purpose: crate::vm::SortPrimPurpose::Length,
        items: Vec::new(),
        aux: Vec::new(),
        width: 1,
        base: 0,
        i: 0,
        j: 0,
        k: 0,
        left_end: 0,
        right_end: 0,
        pair_end: 0,
        await_cmp: false,
        write_idx: 0,
        await_set: false,
        await_callee: Value::undefined(),
    };
    match sort_drive(vm, gc, &mut sop) {
        SortStepOut::Wait => {
            // A frame was pushed (rebase missed the local op): stamp its
            // index manually, mirroring the collection-foreach arm.
            sop.source_frame_depth = vm.frame_depth() - 1;
            vm.pending_sort_op = Some(sop);
            Value::undefined()
        }
        SortStepOut::Done(v) => v,
        SortStepOut::Raise(e) => {
            vm.set_pending_exception(e);
            Value::undefined()
        }
        SortStepOut::Progress => unreachable!("sort_drive never returns Progress"),
    }
}

/// Array.prototype.sort(comparator) — stable, observable (§23.1.3.30).
pub fn array_sort(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    array_sort_entry(gc, this, args, vm, false)
}

/// Array.prototype.toSorted(comparator) — stable copy (§23.1.3.34).
pub fn array_to_sorted(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    array_sort_entry(gc, this, args, vm, true)
}

/// Array.prototype.flatMap(callback, thisArg) — set up state machine iteration, spreading array results.
pub fn array_flat_map(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some((length, callback, this_arg, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.flatMap")
    else {
        return Value::undefined();
    };
    let result_arr = RuneArray::allocate(gc, &[]);
    unsafe {
        let ptr = result_arr as *mut u8;
        *(ptr.add(8) as *mut *const rune_core::shape::Shape) =
            *DENSE_ARRAY_SHAPE as *const rune_core::shape::Shape;
        if let Some(proto) = vm.array_prototype.heap_ptr() {
            *(ptr.add(24) as *mut *mut u8) = proto;
        }
    }
    let Some(first) = first_existing_index(this, length) else {
        return Value::from_heap_ptr(result_arr as *mut u8);
    };
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::FlatMap,
        source: source_ptr,
        result: result_arr as *mut u8,
        callback,
        this_val: this_arg,
        source_val: this,
        index: first,
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    // B1a: accessor elements dispatch their getter (Wait records the
    // await; SyncErr routes through the normal pending-exception path).
    match crate::vm::array_element_value(vm, gc, this, first) {
        crate::vm::ArrayElemOut::Ready(element) => {
            vm.push_callback_call(
                gc,
                callback,
                this_arg,
                vec![element, Value::smi(first as i32), this],
            );
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(first);
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
    Value::undefined()
}

/// Array.prototype.some(callback, thisArg) — set up state machine iteration.
pub fn array_some(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some((length, callback, this_arg, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.some")
    else {
        return Value::boolean(false);
    };
    let Some(first) = first_existing_index(this, length) else {
        return Value::boolean(false);
    };
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Some,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: first,
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    // B1a: accessor elements dispatch their getter (Wait records the
    // await; SyncErr routes through the normal pending-exception path).
    match crate::vm::array_element_value(vm, gc, this, first) {
        crate::vm::ArrayElemOut::Ready(element) => {
            vm.push_callback_call(
                gc,
                callback,
                this_arg,
                vec![element, Value::smi(first as i32), this],
            );
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(first);
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
    Value::undefined()
}

/// Array.prototype.every(callback, thisArg) — set up state machine iteration.
pub fn array_every(gc: &mut SemiSpace, this: Value, args: &[Value], vm: &mut Vm) -> Value {
    let Some((length, callback, this_arg, source_ptr)) =
        array_iter_prologue(gc, vm, this, args, "Array.prototype.every")
    else {
        return Value::boolean(true);
    };
    let Some(first) = first_existing_index(this, length) else {
        return Value::boolean(true);
    };
    vm.pending_array_op = Some(crate::vm::ArrayOpState {
        kind: crate::vm::ArrayOpKind::Every,
        source: source_ptr,
        result: std::ptr::null_mut(),
        callback,
        this_val: this_arg,
        source_val: this,
        index: first,
        length,
        source_frame_depth: 0,
        accumulator: None,
        awaiting_element: None,
        awaiting_acc: false,
    });
    // B1a: accessor elements dispatch their getter (Wait records the
    // await; SyncErr routes through the normal pending-exception path).
    match crate::vm::array_element_value(vm, gc, this, first) {
        crate::vm::ArrayElemOut::Ready(element) => {
            vm.push_callback_call(
                gc,
                callback,
                this_arg,
                vec![element, Value::smi(first as i32), this],
            );
        }
        crate::vm::ArrayElemOut::Wait => {
            if let Some(ref mut op) = vm.pending_array_op {
                op.awaiting_element = Some(first);
            }
            vm.rebase_pending_depths();
        }
        crate::vm::ArrayElemOut::SyncErr(e) => {
            vm.pending_array_op = None;
            vm.set_pending_exception(e);
        }
    }
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
            length: 1,
            name: "Generator_prototype_next",
            func: generator_next_builtin,
        },
        Builtin {
            length: 1,
            name: "Generator_prototype_return",
            func: generator_return_builtin,
        },
        Builtin {
            length: 1,
            name: "Generator_prototype_throw",
            func: generator_throw_builtin,
        },
        Builtin {
            length: 0,
            name: "Generator_prototype_symbol_iterator",
            func: generator_symbol_iterator_builtin,
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
            length: 2,
            name: "Object_assign",
            func: object_assign,
        },
        Builtin {
            length: 2,
            name: "Object_is",
            func: object_is,
        },
        Builtin {
            length: 1,
            name: "Object_getOwnPropertyNames",
            func: object_get_own_property_names,
        },
        Builtin {
            length: 1,
            name: "Object_fromEntries",
            func: object_from_entries,
        },
        Builtin {
            length: 2,
            name: "Object_hasOwn",
            func: object_has_own,
        },
        Builtin {
            length: 2,
            name: "Object_setPrototypeOf",
            func: object_set_prototype_of,
        },
        Builtin {
            length: 3,
            name: "Object_defineProperty",
            func: object_define_property,
        },
        Builtin {
            length: 2,
            name: "Object_defineProperties",
            func: object_define_properties,
        },
        Builtin {
            length: 2,
            name: "Object_getOwnPropertyDescriptor",
            func: object_get_own_property_descriptor,
        },
        Builtin {
            length: 1,
            name: "Object_preventExtensions",
            func: object_prevent_extensions,
        },
        Builtin {
            length: 1,
            name: "Object_seal",
            func: object_seal,
        },
        Builtin {
            length: 1,
            name: "Object_freeze",
            func: object_freeze,
        },
        Builtin {
            length: 1,
            name: "Object_isExtensible",
            func: object_is_extensible,
        },
        Builtin {
            length: 1,
            name: "Object_isSealed",
            func: object_is_sealed,
        },
        Builtin {
            length: 1,
            name: "Object_isFrozen",
            func: object_is_frozen,
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
            length: 0,
            name: "Array_prototype_reverse",
            func: array_reverse,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_concat",
            func: array_concat,
        },
        Builtin {
            length: 0,
            name: "Array_prototype_shift",
            func: array_shift,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_unshift",
            func: array_unshift,
        },
        Builtin {
            length: 2,
            name: "Array_prototype_splice",
            func: array_splice,
        },
        Builtin {
            length: 2,
            name: "Array_prototype_copyWithin",
            func: array_copy_within,
        },
        Builtin {
            length: 2,
            name: "Array_prototype_toSpliced",
            func: array_to_spliced,
        },
        Builtin {
            length: 2,
            name: "Array_prototype_with",
            func: array_with,
        },
        Builtin {
            length: 1,
            name: "Array_of",
            func: array_of,
        },
        Builtin {
            length: 1,
            name: "Array_constructor",
            func: array_constructor,
        },
        Builtin {
            length: 2,
            name: "Array_prototype_slice",
            func: array_slice,
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
        Builtin {
            length: 1,
            name: "Math_round",
            func: math_round,
        },
        Builtin {
            length: 1,
            name: "Math_trunc",
            func: math_trunc,
        },
        Builtin {
            length: 1,
            name: "Math_sign",
            func: math_sign,
        },
        Builtin {
            length: 2,
            name: "Math_hypot",
            func: math_hypot,
        },
        Builtin {
            length: 1,
            name: "Math_clz32",
            func: math_clz32,
        },
        Builtin {
            length: 2,
            name: "Math_imul",
            func: math_imul,
        },
        Builtin {
            length: 1,
            name: "Math_cbrt",
            func: math_cbrt,
        },
        Builtin {
            length: 1,
            name: "Math_log",
            func: math_log,
        },
        Builtin {
            length: 1,
            name: "Math_log2",
            func: math_log2,
        },
        Builtin {
            length: 1,
            name: "Math_log10",
            func: math_log10,
        },
        Builtin {
            length: 1,
            name: "Math_exp",
            func: math_exp,
        },
        Builtin {
            length: 1,
            name: "Math_sin",
            func: math_sin,
        },
        Builtin {
            length: 1,
            name: "Math_cos",
            func: math_cos,
        },
        Builtin {
            length: 1,
            name: "Math_tan",
            func: math_tan,
        },
        Builtin {
            length: 1,
            name: "Math_asin",
            func: math_asin,
        },
        Builtin {
            length: 1,
            name: "Math_acos",
            func: math_acos,
        },
        Builtin {
            length: 1,
            name: "Math_atan",
            func: math_atan,
        },
        Builtin {
            length: 2,
            name: "Math_atan2",
            func: math_atan2,
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
        Builtin {
            length: 1,
            name: "isNaN",
            func: is_nan_builtin,
        },
        Builtin {
            length: 1,
            name: "isFinite",
            func: is_finite_builtin,
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
            name: "Array_prototype_reduceRight",
            func: array_reduce_right,
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
            length: 2,
            name: "Array_prototype_lastIndexOf",
            func: array_last_index_of,
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
            name: "Array_prototype_findLast",
            func: array_find_last,
        },
        Builtin {
            length: 1,
            name: "Array_prototype_findLastIndex",
            func: array_find_last_index,
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
            name: "Array_prototype_toSorted",
            func: array_to_sorted,
        },
        Builtin {
            length: 1,
            name: "Function_prototype_call",
            func: call_builtin,
        },
        Builtin {
            length: 2,
            name: "Function_prototype_apply",
            func: apply_builtin,
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
            name: "assert_compareArray",
            func: assert_compare_array,
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

pub(crate) fn make_error(gc: &mut SemiSpace, protos: &[Value], msg: &str) -> Value {
    // A2: all call sites now pass clean (prefix-free) messages — the legacy
    // `"Kind: rest"` split stays as a safety net for any missed caller, and
    // uncaught-error rendering (error_to_string) reattaches the kind name.
    let (kind, rest) = crate::errors::ErrorKind::split_legacy(msg);
    crate::errors::error_object(gc, protos, kind, rest)
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

/// Render a thrown value for uncaught-error output, following
/// Error.prototype.toString (§20.5.3.4): strings render whole; objects
/// render `name: message` (either part defaulting per spec). Used by the
/// Context eval-error paths so flipped error objects print with their kind.
pub fn error_to_string(gc: &mut SemiSpace, val: Value) -> Option<String> {
    let ptr = val.heap_ptr()?;
    unsafe {
        if (*(ptr as *const GcHeader)).tag() == TAG_STRING {
            return Some(HeapString::to_string(ptr as *mut HeapString));
        }
    }
    // Error.prototype.toString needs name/message reads with proto-chain
    // lookup (name usually lives on the prototype, message own).
    let name_key = Value::from_heap_ptr(HeapString::allocate(gc, "name") as *mut u8);
    let msg_key = Value::from_heap_ptr(HeapString::allocate(gc, "message") as *mut u8);
    let name_val = load_property_recursive(val, name_key, None, gc);
    let msg_val = load_property_recursive(val, msg_key, None, gc);
    let name_str = if name_val.is_undefined() {
        "Error".to_string()
    } else {
        value_to_js_string(name_val)
    };
    let msg_str = if msg_val.is_undefined() {
        String::new()
    } else {
        value_to_js_string(msg_val)
    };
    Some(if name_str.is_empty() {
        msg_str
    } else if msg_str.is_empty() {
        name_str
    } else {
        format!("{name_str}: {msg_str}")
    })
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
        let err = make_error(gc, &_vm.error_protos, &msg);
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
        let err = make_error(gc, &_vm.error_protos, &msg);
        _vm.set_pending_exception(err);
    }
    Value::undefined()
}

/// assert.compareArray(actual, expected, message) — length equality plus
/// SameValue per index (mirrors test262's assert.js; the builtin assert
/// lacked it, failing every suite test that compares array results).
/// Reads go through load_property_recursive (proto consult like Get; JS
/// element getters don't dispatch — sync gap, no suite test needs it).
pub fn assert_compare_array(
    gc: &mut SemiSpace,
    _this: Value,
    args: &[Value],
    vm: &mut Vm,
) -> Value {
    vm.assert_called = true;
    let actual = args.first().copied().unwrap_or(Value::undefined());
    let expected = args.get(1).copied().unwrap_or(Value::undefined());
    let desc = args.get(2).map(|v| value_to_debug(*v)).unwrap_or_default();
    let prefix = if desc.is_empty() {
        String::new()
    } else {
        format!("{desc} ")
    };
    macro_rules! fail {
        ($detail:expr) => {{
            vm.set_pending_exception(make_error(
                gc,
                &vm.error_protos,
                &format!("{prefix}assert.compareArray: {}", $detail),
            ));
            return Value::undefined();
        }};
    }
    let is_prim = |v: Value| {
        if !v.is_heap_object() {
            return true;
        }
        v.heap_ptr()
            .is_some_and(|p| unsafe { (*(p as *const GcHeader)).tag() == TAG_STRING })
    };
    if is_prim(actual) || is_prim(expected) {
        fail!("arguments shouldn't be primitive");
    }
    let len_of = |v: Value| crate::vm::array_like_length(v).unwrap_or(0) as usize;
    let (alen, blen) = (len_of(actual), len_of(expected));
    if alen != blen {
        fail!(format!("lengths differ ({alen} vs {blen})"));
    }
    for i in 0..alen {
        let a = crate::vm::load_property_recursive(actual, Value::smi(i as i32), None, gc);
        let b = crate::vm::load_property_recursive(expected, Value::smi(i as i32), None, gc);
        if !same_value(a, b) {
            fail!(format!(
                "index {i} differs ({} vs {})",
                value_to_debug(a),
                value_to_debug(b)
            ));
        }
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
        let err = make_error(gc, &_vm.error_protos, &full_msg);
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
            &vm.error_protos,
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
                    &vm.error_protos,
                    "SyntaxError: Invalid regular expression flags",
                ));
                return Value::undefined();
            }
        };
        if seen & bit != 0 {
            vm.set_pending_exception(make_error(
                gc,
                &vm.error_protos,
                "SyntaxError: Duplicate regular expression flag",
            ));
            return Value::undefined();
        }
        seen |= bit;
    }

    // §22.2.3.3: pattern must parse, else SyntaxError.
    if rune_regex::parse_regex(&pattern_str).is_err() {
        vm.set_pending_exception(make_error(
            gc,
            &vm.error_protos,
            "SyntaxError: Invalid regular expression",
        ));
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
