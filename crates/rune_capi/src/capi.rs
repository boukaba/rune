//! Stable C ABI for embedding Rune.
//!
//! The boundary uses only opaque pointers, C strings, integers, and status
//! codes. Panics are caught so they never unwind across FFI.

use crate::value::{HostCallback, RuneValue, retained_to_c, value_from_c, value_to_c};
use rune_core::gc::{GcHeader, TAG_FUNC, TAG_STRING};
use rune_core::value::Value;
use rune_embed::Context;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

const STATUS_OK: i32 = 0;
const STATUS_ERROR: i32 = 1;
const STATUS_INVALID: i32 = -1;

thread_local! {
    /// Runtime jobs are kept beside (not inside) the opaque Context pointer.
    /// The C API is documented as single-threaded, so a per-thread table keyed
    /// by the context address is sufficient.
    static RUNTIME_JOBS: RefCell<HashMap<usize, rune_runtime::RuntimeJobs>> =
        RefCell::new(HashMap::new());
    static FETCHERS: RefCell<HashMap<usize, (usize, RuneFetchCallback)>> =
        RefCell::new(HashMap::new());
}

/// Response filled by a C fetch transport. Strings are borrowed for the call
/// and copied by the engine.
#[repr(C)]
pub struct RuneFetchResponse {
    pub status: u32,
    pub body: *const c_char,
    pub status_text: *const c_char,
    /// Header blob: one `name<TAB>value<LF>` per header.
    pub headers: *const c_char,
}

/// C fetch transport. Return 0 on success, non-zero to reject the fetch.
pub type RuneFetchCallback = unsafe extern "C" fn(
    user: *mut c_void,
    method: *const c_char,
    url: *const c_char,
    headers: *const c_char,
    body: *const c_char,
    response: *mut RuneFetchResponse,
) -> i32;

fn into_c_string(text: String) -> *mut c_char {
    CString::new(text.replace('\0', "\\0"))
        .unwrap_or_else(|_| CString::new("error").unwrap())
        .into_raw()
}

unsafe fn write_out(slot: *mut *mut c_char, value: *mut c_char) {
    if !slot.is_null() {
        unsafe { *slot = value };
    } else if !value.is_null() {
        unsafe {
            let _ = CString::from_raw(value);
        }
    }
}

/// Engine version (static storage).
#[unsafe(no_mangle)]
pub extern "C" fn rune_version() -> *const c_char {
    static VERSION: &[u8] = concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes();
    VERSION.as_ptr() as *const c_char
}

/// Create a context with the default heap (16 MiB).
#[unsafe(no_mangle)]
pub extern "C" fn rune_context_create() -> *mut c_void {
    Box::into_raw(Box::new(Context::new())) as *mut c_void
}

/// Create a context with an explicit semispace size. Returns NULL if zero.
#[unsafe(no_mangle)]
pub extern "C" fn rune_context_create_with_heap(bytes: usize) -> *mut c_void {
    if bytes == 0 {
        return std::ptr::null_mut();
    }
    let ctx = Context::new_with_heap_size(bytes);
    Box::into_raw(Box::new(ctx)) as *mut c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn rune_context_destroy(ctx: *mut c_void) {
    if !ctx.is_null() {
        RUNTIME_JOBS.with(|jobs| {
            jobs.borrow_mut().remove(&(ctx as usize));
        });
        FETCHERS.with(|fetchers| {
            fetchers.borrow_mut().remove(&(ctx as usize));
        });
        unsafe {
            let _ = Box::from_raw(ctx as *mut Context);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rune_context_set_instruction_budget(ctx: *mut c_void, instructions: u64) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    ctx.set_instruction_budget(instructions);
}

#[unsafe(no_mangle)]
pub extern "C" fn rune_context_set_time_budget_ms(ctx: *mut c_void, milliseconds: u64) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    ctx.set_time_budget(std::time::Duration::from_millis(milliseconds));
}

#[unsafe(no_mangle)]
pub extern "C" fn rune_context_set_jit(ctx: *mut c_void, enabled: i32) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    ctx.enable_jit = enabled != 0;
}

/// Register a host function backed by a C callback. Arguments are passed as
/// borrowed `RuneValue`s; the return value is copied immediately. Returns 0 on
/// success and -1 for invalid pointers/names.
///
/// # Safety
/// `ctx` must be a live context. `name` must be a NUL-terminated UTF-8 string.
/// `callback` (when non-NULL) must remain valid for the context's lifetime and
/// `user` must be safe to pass to it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_register_host_fn(
    ctx: *mut c_void,
    name: *const c_char,
    length: u32,
    user: *mut c_void,
    callback: Option<HostCallback>,
) -> i32 {
    if ctx.is_null() || name.is_null() {
        return STATUS_INVALID;
    }
    let Some(callback) = callback else {
        return STATUS_INVALID;
    };
    let name = unsafe { CStr::from_ptr(name) };
    let Ok(name) = name.to_str() else {
        return STATUS_INVALID;
    };
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    ctx.register_host_fn(name, length, move |gc, _this, args, vm| {
        let mut strings = Vec::new();
        let c_args: Vec<RuneValue> = args
            .iter()
            .map(|value| {
                let is_string = value
                    .heap_ptr()
                    .is_some_and(|ptr| unsafe { (*(ptr as *const GcHeader)).tag() == TAG_STRING });
                if value.heap_ptr().is_some() && !is_string {
                    let id = vm.retain_value(*value);
                    retained_to_c(*value, id)
                } else {
                    value_to_c(*value, &mut strings)
                }
            })
            .collect();
        let result = unsafe { callback(user, c_args.len(), c_args.as_ptr()) };
        if result.handle != 0 {
            vm.retained_value(result.handle as usize - 1)
        } else {
            value_from_c(gc, result)
        }
    });
    STATUS_OK
}

fn value_to_output(ctx: &mut Context, value: Value) -> RuneValue {
    if value.heap_ptr().is_some() {
        let id = ctx.retain_value(value);
        retained_to_c(value, id)
    } else {
        let mut strings = Vec::new();
        value_to_c(value, &mut strings)
    }
}

fn primitive_from_c(gc: &mut rune_core::gc::SemiSpace, value: RuneValue) -> Result<Value, i32> {
    match value.kind {
        crate::value::RUNE_UNDEFINED
        | crate::value::RUNE_NULL
        | crate::value::RUNE_BOOLEAN
        | crate::value::RUNE_NUMBER
        | crate::value::RUNE_STRING => Ok(value_from_c(gc, value)),
        _ => Err(STATUS_INVALID),
    }
}

/// Evaluate source and return a retained value handle for heap results.
///
/// # Safety
/// Same pointer/contract requirements as `rune_context_eval`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_eval_value(
    ctx: *mut c_void,
    source: *const c_char,
    result: *mut RuneValue,
    error: *mut *mut c_char,
) -> i32 {
    if !error.is_null() {
        unsafe { *error = std::ptr::null_mut() };
    }
    if ctx.is_null() || source.is_null() || result.is_null() {
        return STATUS_INVALID;
    }
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    let source = unsafe { CStr::from_ptr(source) };
    let Ok(source) = source.to_str() else {
        unsafe {
            write_out(
                error,
                into_c_string("source is not valid UTF-8".to_string()),
            )
        };
        return STATUS_ERROR;
    };
    match catch_unwind(AssertUnwindSafe(|| ctx.eval(source))) {
        Ok(Ok(value)) => {
            let out = value_to_output(ctx, value);
            unsafe { *result = out };
            STATUS_OK
        }
        Ok(Err(message)) => {
            unsafe { write_out(error, into_c_string(message)) };
            STATUS_ERROR
        }
        Err(_) => {
            unsafe { write_out(error, into_c_string("engine panic".to_string())) };
            STATUS_ERROR
        }
    }
}

/// Stringify a primitive or retained value.
///
/// # Safety
/// `ctx` must be live; `out` must be a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_value_to_string(
    ctx: *mut c_void,
    value: RuneValue,
    out: *mut *mut c_char,
) -> i32 {
    if !out.is_null() {
        unsafe { *out = std::ptr::null_mut() };
    }
    if ctx.is_null() || out.is_null() {
        return STATUS_INVALID;
    }
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    let value = if value.handle != 0 {
        ctx.retained_value(value.handle as usize - 1)
    } else {
        value_from_c(ctx.gc(), value)
    };
    let text = rune_interpreter::builtins::value_to_js_string(value);
    unsafe { write_out(out, into_c_string(text)) };
    STATUS_OK
}

/// Read a data property from a retained object/array. JS getters are rejected
/// because the C API cannot suspend the interpreter.
///
/// # Safety
/// `ctx` must be live; `object` must be a retained heap value; `key` must be a
/// NUL-terminated UTF-8 string; `result` must be a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_get_property(
    ctx: *mut c_void,
    object: RuneValue,
    key: *const c_char,
    result: *mut RuneValue,
    error: *mut *mut c_char,
) -> i32 {
    if !error.is_null() {
        unsafe { *error = std::ptr::null_mut() };
    }
    if ctx.is_null() || key.is_null() || result.is_null() || object.handle == 0 {
        return STATUS_INVALID;
    }
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    let key = unsafe { CStr::from_ptr(key) };
    let Ok(key) = key.to_str() else {
        return STATUS_INVALID;
    };
    let object = ctx.retained_value(object.handle as usize - 1);
    match catch_unwind(AssertUnwindSafe(|| ctx.get_property(object, key))) {
        Ok(Ok(value)) => {
            let out = value_to_output(ctx, value);
            unsafe { *result = out };
            STATUS_OK
        }
        Ok(Err(message)) => {
            unsafe { write_out(error, into_c_string(message)) };
            STATUS_ERROR
        }
        Err(_) => {
            unsafe { write_out(error, into_c_string("engine panic".to_string())) };
            STATUS_ERROR
        }
    }
}

/// Write a data property on a retained object/array. JS setters are rejected
/// because the C API cannot suspend the interpreter.
///
/// # Safety
/// `ctx` must be live; `object` must be a retained heap value; `key` must be a
/// NUL-terminated UTF-8 string; `value` must be a primitive or retained handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_set_property(
    ctx: *mut c_void,
    object: RuneValue,
    key: *const c_char,
    value: RuneValue,
    error: *mut *mut c_char,
) -> i32 {
    if !error.is_null() {
        unsafe { *error = std::ptr::null_mut() };
    }
    if ctx.is_null() || key.is_null() || object.handle == 0 {
        return STATUS_INVALID;
    }
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    let key = unsafe { CStr::from_ptr(key) };
    let Ok(key) = key.to_str() else {
        return STATUS_INVALID;
    };
    let object = ctx.retained_value(object.handle as usize - 1);
    let value = if value.handle != 0 {
        ctx.retained_value(value.handle as usize - 1)
    } else {
        match primitive_from_c(ctx.gc(), value) {
            Ok(value) => value,
            Err(status) => return status,
        }
    };
    match catch_unwind(AssertUnwindSafe(|| ctx.set_property(object, key, value))) {
        Ok(Ok(())) => STATUS_OK,
        Ok(Err(message)) => {
            unsafe { write_out(error, into_c_string(message)) };
            STATUS_ERROR
        }
        Err(_) => {
            unsafe { write_out(error, into_c_string("engine panic".to_string())) };
            STATUS_ERROR
        }
    }
}

/// Release a retained heap value handle.
///
/// # Safety
/// `ctx` must be live; values with `handle == 0` are a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_release_value(ctx: *mut c_void, value: RuneValue) -> i32 {
    if ctx.is_null() {
        return STATUS_INVALID;
    }
    if value.handle == 0 {
        return STATUS_OK;
    }
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    ctx.release_value(value.handle as usize - 1);
    STATUS_OK
}

/// Call a retained JavaScript function with primitive/retained arguments.
///
/// # Safety
/// `ctx` must be live; `func` must be a function value returned by this API;
/// `args` must point to `argc` RuneValues.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_call_value(
    ctx: *mut c_void,
    func: RuneValue,
    args: *const RuneValue,
    argc: usize,
    result: *mut RuneValue,
    error: *mut *mut c_char,
) -> i32 {
    if !error.is_null() {
        unsafe { *error = std::ptr::null_mut() };
    }
    if ctx.is_null() || result.is_null() {
        return STATUS_INVALID;
    }
    if func.handle == 0 {
        return STATUS_INVALID;
    }
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    let function = ctx.retained_value(func.handle as usize - 1);
    let is_func = function
        .heap_ptr()
        .is_some_and(|ptr| unsafe { (*(ptr as *const GcHeader)).tag() == TAG_FUNC });
    if !is_func {
        return STATUS_INVALID;
    }
    let mut call_args = Vec::with_capacity(argc);
    if argc > 0 && args.is_null() {
        return STATUS_INVALID;
    }
    for i in 0..argc {
        let arg = unsafe { *args.add(i) };
        if arg.handle != 0 {
            call_args.push(ctx.retained_value(arg.handle as usize - 1));
            continue;
        }
        match primitive_from_c(ctx.gc(), arg) {
            Ok(value) => call_args.push(value),
            Err(status) => return status,
        }
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| ctx.call_value(function, &call_args)));
    match outcome {
        Ok(Ok(value)) => {
            let out = value_to_output(ctx, value);
            unsafe { *result = out };
            STATUS_OK
        }
        Ok(Err(message)) => {
            unsafe { write_out(error, into_c_string(message)) };
            STATUS_ERROR
        }
        Err(_) => {
            unsafe { write_out(error, into_c_string("engine panic".to_string())) };
            STATUS_ERROR
        }
    }
}

/// Install timers/promise jobs for this context (idempotent).
///
/// # Safety
/// `ctx` must be a live context pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_install_runtime(ctx: *mut c_void) -> i32 {
    if ctx.is_null() {
        return STATUS_INVALID;
    }
    let ctx_ref = unsafe { &mut *(ctx as *mut Context) };
    RUNTIME_JOBS.with(|jobs| {
        let mut jobs = jobs.borrow_mut();
        if let std::collections::hash_map::Entry::Vacant(entry) = jobs.entry(ctx as usize) {
            entry.insert(rune_runtime::install(ctx_ref));
        }
    });
    STATUS_OK
}

/// Install a C HTTP transport used by `fetch` jobs. Requires
/// `rune_context_install_runtime` to have been called.
///
/// # Safety
/// `ctx` must be live; `callback` and `user` must remain valid until the
/// context is destroyed or another transport is installed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_set_fetcher(
    ctx: *mut c_void,
    user: *mut c_void,
    callback: Option<RuneFetchCallback>,
) -> i32 {
    if ctx.is_null() {
        return STATUS_INVALID;
    }
    let Some(callback) = callback else {
        return STATUS_INVALID;
    };
    let key = ctx as usize;
    let jobs = RUNTIME_JOBS.with(|slot| slot.borrow_mut().remove(&key));
    let Some(mut jobs) = jobs else {
        return STATUS_INVALID;
    };
    FETCHERS.with(|slot| {
        slot.borrow_mut().insert(key, (user as usize, callback));
    });
    jobs.set_fetcher(move |request| {
        let (user, callback) = FETCHERS
            .with(|slot| slot.borrow().get(&key).copied())
            .ok_or_else(|| "C fetch transport is not installed".to_string())?;
        let method = CString::new(request.method.as_str()).unwrap_or_default();
        let url = CString::new(request.url.as_str()).unwrap_or_default();
        let headers =
            CString::new(crate::value::headers_blob(&request.headers)).unwrap_or_default();
        let body = CString::new(request.body.clone().unwrap_or_default()).unwrap_or_default();
        let mut out = RuneFetchResponse {
            status: 0,
            body: std::ptr::null(),
            status_text: std::ptr::null(),
            headers: std::ptr::null(),
        };
        let rc = unsafe {
            callback(
                user as *mut c_void,
                method.as_ptr(),
                url.as_ptr(),
                headers.as_ptr(),
                body.as_ptr(),
                &mut out,
            )
        };
        if rc != 0 {
            return Err("C fetch transport failed".to_string());
        }
        let text = |ptr: *const c_char| -> String {
            if ptr.is_null() {
                String::new()
            } else {
                unsafe { CStr::from_ptr(ptr) }
                    .to_string_lossy()
                    .into_owned()
            }
        };
        Ok(rune_runtime::FetchResponse {
            status: out.status.min(u16::MAX as u32) as u16,
            status_text: text(out.status_text),
            headers: crate::value::parse_headers_blob(&text(out.headers)),
            body: text(out.body),
        })
    });
    RUNTIME_JOBS.with(|slot| slot.borrow_mut().insert(key, jobs));
    STATUS_OK
}

/// Run due timers and queued promise jobs. Returns 1 and fills `error` on a
/// callback failure. Requires `rune_context_install_runtime`.
///
/// # Safety
/// `ctx` must be a live context pointer; `error` may be NULL or a writable slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_run_jobs(ctx: *mut c_void, error: *mut *mut c_char) -> i32 {
    if !error.is_null() {
        unsafe { *error = std::ptr::null_mut() };
    }
    if ctx.is_null() {
        return STATUS_INVALID;
    }
    let mut jobs = RUNTIME_JOBS.with(|jobs| jobs.borrow_mut().remove(&(ctx as usize)));
    let Some(mut jobs) = jobs.take() else {
        return STATUS_INVALID;
    };
    let outcome = {
        let ctx_ref = unsafe { &mut *(ctx as *mut Context) };
        jobs.run_jobs(ctx_ref)
    };
    RUNTIME_JOBS.with(|slot| slot.borrow_mut().insert(ctx as usize, jobs));
    match outcome {
        Ok(()) => STATUS_OK,
        Err(message) => {
            unsafe { write_out(error, into_c_string(message)) };
            STATUS_ERROR
        }
    }
}

/// Evaluate source. See `include/rune.h` for the status/out-param contract.
///
/// # Safety
/// `ctx` must be a live context pointer. `source` must be a NUL-terminated
/// UTF-8 string. Output pointers, when non-NULL, must be valid writable slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_eval(
    ctx: *mut c_void,
    source: *const c_char,
    result: *mut *mut c_char,
    error: *mut *mut c_char,
) -> i32 {
    if !result.is_null() {
        unsafe { *result = std::ptr::null_mut() };
    }
    if !error.is_null() {
        unsafe { *error = std::ptr::null_mut() };
    }
    if ctx.is_null() || source.is_null() {
        return STATUS_INVALID;
    }
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    let source = unsafe { CStr::from_ptr(source) };
    let Ok(source) = source.to_str() else {
        unsafe {
            write_out(
                error,
                into_c_string("source is not valid UTF-8".to_string()),
            )
        };
        return STATUS_ERROR;
    };

    let outcome = catch_unwind(AssertUnwindSafe(|| ctx.eval(source)));
    match outcome {
        Ok(Ok(value)) => {
            let text = rune_interpreter::builtins::value_to_js_string(value);
            unsafe { write_out(result, into_c_string(text)) };
            STATUS_OK
        }
        Ok(Err(message)) => {
            unsafe { write_out(error, into_c_string(message)) };
            STATUS_ERROR
        }
        Err(_) => {
            unsafe { write_out(error, into_c_string("engine panic".to_string())) };
            STATUS_ERROR
        }
    }
}

/// # Safety
/// `text` must be a pointer returned by this API that has not been freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_free_string(text: *mut c_char) {
    if !text.is_null() {
        unsafe {
            let _ = CString::from_raw(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(ctx: *mut c_void, source: &str) -> (i32, Option<String>, Option<String>) {
        let source = CString::new(source).unwrap();
        let mut result: *mut c_char = std::ptr::null_mut();
        let mut error: *mut c_char = std::ptr::null_mut();
        let status = unsafe {
            rune_context_eval(
                ctx,
                source.as_ptr(),
                &mut result as *mut *mut c_char,
                &mut error as *mut *mut c_char,
            )
        };
        let take = |ptr: *mut c_char| -> Option<String> {
            if ptr.is_null() {
                None
            } else {
                let text = unsafe { CStr::from_ptr(ptr) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { rune_free_string(ptr) };
                Some(text)
            }
        };
        (status, take(result), take(error))
    }

    #[test]
    fn c_api_evaluates_and_reports_errors() {
        let ctx = rune_context_create();
        assert!(!ctx.is_null());
        let (status, result, error) = eval(ctx, "1 + 2");
        assert_eq!(status, 0);
        assert_eq!(result.as_deref(), Some("3"));
        assert!(error.is_none());

        let (status, result, error) = eval(ctx, "throw new Error('boom')");
        assert_eq!(status, 1);
        assert!(result.is_none());
        assert!(error.as_deref().unwrap_or("").contains("boom"));

        let (status, result, _) = eval(ctx, "'hello' + ' ' + 'rune'");
        assert_eq!(status, 0);
        assert_eq!(result.as_deref(), Some("hello rune"));
        rune_context_destroy(ctx);
    }

    #[test]
    fn c_api_enforces_budget_and_handles_null() {
        let ctx = rune_context_create();
        rune_context_set_instruction_budget(ctx, 20_000);
        let (status, _, error) = eval(ctx, "while (true) {}");
        assert_eq!(status, 1);
        assert!(error.unwrap_or_default().contains("budget"));
        rune_context_destroy(ctx);

        let source = CString::new("1").unwrap();
        assert_eq!(
            unsafe {
                rune_context_eval(
                    std::ptr::null_mut(),
                    source.as_ptr(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            -1
        );
    }

    #[test]
    fn c_api_heap_and_version() {
        assert!(!rune_version().is_null());
        assert!(rune_context_create_with_heap(0).is_null());
        let ctx = rune_context_create_with_heap(2 * 1024 * 1024);
        assert!(!ctx.is_null());
        let (status, result, _) = eval(ctx, "[1,2,3].length");
        assert_eq!(status, 0);
        assert_eq!(result.as_deref(), Some("3"));
        rune_context_destroy(ctx);
    }
}

#[cfg(test)]
mod host_fn_tests {
    use super::*;
    use crate::value::{RUNE_BOOLEAN, RUNE_NUMBER, RUNE_STRING};
    use std::ffi::CStr;

    unsafe extern "C" fn add_cb(
        _user: *mut c_void,
        argc: usize,
        argv: *const RuneValue,
    ) -> RuneValue {
        let mut total = 0.0;
        for i in 0..argc {
            let arg = unsafe { *argv.add(i) };
            if arg.kind == RUNE_NUMBER {
                total += arg.number;
            }
        }
        RuneValue {
            kind: RUNE_NUMBER,
            boolean: 0,
            number: total,
            string: std::ptr::null(),
            handle: 0,
        }
    }

    unsafe extern "C" fn greet_cb(
        _user: *mut c_void,
        _argc: usize,
        _argv: *const RuneValue,
    ) -> RuneValue {
        RuneValue {
            kind: RUNE_STRING,
            boolean: 0,
            number: 0.0,
            string: c"from-c".as_ptr(),
            handle: 0,
        }
    }

    unsafe extern "C" fn string_len_cb(
        _user: *mut c_void,
        argc: usize,
        argv: *const RuneValue,
    ) -> RuneValue {
        let mut total = 0;
        for i in 0..argc {
            let arg = unsafe { *argv.add(i) };
            if arg.kind == RUNE_STRING && !arg.string.is_null() {
                total += unsafe { CStr::from_ptr(arg.string) }.to_bytes().len();
            }
        }
        RuneValue {
            kind: RUNE_NUMBER,
            boolean: 0,
            number: total as f64,
            string: std::ptr::null(),
            handle: 0,
        }
    }

    unsafe extern "C" fn truth_cb(
        _user: *mut c_void,
        _argc: usize,
        _argv: *const RuneValue,
    ) -> RuneValue {
        RuneValue {
            kind: RUNE_BOOLEAN,
            boolean: 1,
            number: 0.0,
            string: std::ptr::null(),
            handle: 0,
        }
    }

    fn eval(ctx: *mut c_void, source: &str) -> (i32, Option<String>) {
        let source = CString::new(source).unwrap();
        let mut result: *mut c_char = std::ptr::null_mut();
        let mut error: *mut c_char = std::ptr::null_mut();
        let status = unsafe {
            rune_context_eval(
                ctx,
                source.as_ptr(),
                &mut result as *mut *mut c_char,
                &mut error as *mut *mut c_char,
            )
        };
        let text = if result.is_null() {
            if error.is_null() {
                None
            } else {
                let text = unsafe { CStr::from_ptr(error) }
                    .to_string_lossy()
                    .into_owned();
                unsafe { rune_free_string(error) };
                Some(text)
            }
        } else {
            let text = unsafe { CStr::from_ptr(result) }
                .to_string_lossy()
                .into_owned();
            unsafe { rune_free_string(result) };
            Some(text)
        };
        (status, text)
    }

    #[test]
    fn c_host_functions_receive_and_return_values() {
        let ctx = rune_context_create();
        assert_eq!(
            unsafe {
                rune_context_register_host_fn(
                    ctx,
                    c"cadd".as_ptr(),
                    2,
                    std::ptr::null_mut(),
                    Some(add_cb),
                )
            },
            0
        );
        assert_eq!(eval(ctx, "cadd(20, 22)"), (0, Some("42".to_string())));

        unsafe {
            rune_context_register_host_fn(
                ctx,
                c"greet".as_ptr(),
                0,
                std::ptr::null_mut(),
                Some(greet_cb),
            );
            rune_context_register_host_fn(
                ctx,
                c"arglen".as_ptr(),
                2,
                std::ptr::null_mut(),
                Some(string_len_cb),
            );
            rune_context_register_host_fn(
                ctx,
                c"truth".as_ptr(),
                0,
                std::ptr::null_mut(),
                Some(truth_cb),
            );
        }
        assert_eq!(eval(ctx, "greet()"), (0, Some("from-c".to_string())));
        assert_eq!(eval(ctx, "arglen('ab', 'cde')"), (0, Some("5".to_string())));
        assert_eq!(eval(ctx, "truth()"), (0, Some("true".to_string())));
        rune_context_destroy(ctx);
    }
}

#[cfg(test)]
mod value_handle_tests {
    use super::*;
    use crate::value::{RUNE_FUNCTION, RUNE_NUMBER, RUNE_STRING, RuneValue};

    fn number(n: f64) -> RuneValue {
        RuneValue {
            kind: RUNE_NUMBER,
            boolean: 0,
            number: n,
            string: std::ptr::null(),
            handle: 0,
        }
    }

    unsafe fn eval_value(ctx: *mut c_void, source: &str) -> (i32, RuneValue) {
        let source = CString::new(source).unwrap();
        let mut out = RuneValue::undefined();
        let mut error: *mut c_char = std::ptr::null_mut();
        let status = unsafe {
            rune_context_eval_value(
                ctx,
                source.as_ptr(),
                &mut out as *mut RuneValue,
                &mut error as *mut *mut c_char,
            )
        };
        if !error.is_null() {
            unsafe { rune_free_string(error) };
        }
        (status, out)
    }

    unsafe fn to_string(ctx: *mut c_void, value: RuneValue) -> Option<String> {
        let mut out: *mut c_char = std::ptr::null_mut();
        let status = unsafe { rune_context_value_to_string(ctx, value, &mut out) };
        if status != 0 || out.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(out) }
            .to_string_lossy()
            .into_owned();
        unsafe { rune_free_string(out) };
        Some(text)
    }

    #[test]
    fn handles_call_js_functions_and_release() {
        let ctx = rune_context_create();
        let (status, function) = unsafe { eval_value(ctx, "(function (a, b) { return a + b; })") };
        assert_eq!(status, 0);
        assert_eq!(function.kind, RUNE_FUNCTION);
        assert_ne!(function.handle, 0);

        let args = [number(20.0), number(22.0)];
        let mut result = RuneValue::undefined();
        let mut error: *mut c_char = std::ptr::null_mut();
        let status = unsafe {
            rune_context_call_value(
                ctx,
                function,
                args.as_ptr(),
                args.len(),
                &mut result as *mut RuneValue,
                &mut error as *mut *mut c_char,
            )
        };
        assert_eq!(status, 0);
        assert_eq!(result.kind, RUNE_NUMBER);
        assert_eq!(result.number, 42.0);

        let (status, string) = unsafe { eval_value(ctx, "'hello' + ' ' + 'rune'") };
        assert_eq!(status, 0);
        assert_eq!(string.kind, RUNE_STRING);
        assert_eq!(
            unsafe { to_string(ctx, string) }.as_deref(),
            Some("hello rune")
        );
        unsafe {
            rune_context_release_value(ctx, string);
            rune_context_release_value(ctx, function);
            rune_context_release_value(ctx, result);
        }
        rune_context_destroy(ctx);
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::value::{RUNE_NUMBER, RUNE_OBJECT, RuneValue};
    use std::sync::atomic::{AtomicU64, Ordering};

    static CAPTURED: AtomicU64 = AtomicU64::new(0);

    unsafe extern "C" fn identity_cb(
        _user: *mut c_void,
        argc: usize,
        argv: *const RuneValue,
    ) -> RuneValue {
        if argc == 0 {
            return RuneValue::undefined();
        }
        unsafe { *argv }
    }

    unsafe extern "C" fn capture_cb(
        _user: *mut c_void,
        argc: usize,
        argv: *const RuneValue,
    ) -> RuneValue {
        if argc > 0 {
            let arg = unsafe { *argv };
            CAPTURED.store(arg.handle, Ordering::SeqCst);
        }
        RuneValue::undefined()
    }

    unsafe fn eval_value(ctx: *mut c_void, source: &str) -> RuneValue {
        let source = CString::new(source).unwrap();
        let mut out = RuneValue::undefined();
        let mut error: *mut c_char = std::ptr::null_mut();
        let status = unsafe {
            rune_context_eval_value(
                ctx,
                source.as_ptr(),
                &mut out as *mut RuneValue,
                &mut error as *mut *mut c_char,
            )
        };
        assert_eq!(status, 0, "eval failed");
        if !error.is_null() {
            unsafe { rune_free_string(error) };
        }
        out
    }

    unsafe fn get(ctx: *mut c_void, object: RuneValue, key: &str) -> RuneValue {
        let key = CString::new(key).unwrap();
        let mut out = RuneValue::undefined();
        let mut error: *mut c_char = std::ptr::null_mut();
        let status = unsafe {
            rune_context_get_property(
                ctx,
                object,
                key.as_ptr(),
                &mut out as *mut RuneValue,
                &mut error as *mut *mut c_char,
            )
        };
        assert_eq!(status, 0, "get_property failed");
        if !error.is_null() {
            unsafe { rune_free_string(error) };
        }
        out
    }

    #[test]
    fn properties_and_callback_object_handles() {
        let ctx = rune_context_create();
        unsafe {
            rune_context_register_host_fn(
                ctx,
                c"capture".as_ptr(),
                1,
                std::ptr::null_mut(),
                Some(capture_cb),
            );
        }
        // Build the object inside JS and hand it to the callback.
        let source = CString::new("capture({ x: 42 })").unwrap();
        let mut error: *mut c_char = std::ptr::null_mut();
        let status =
            unsafe { rune_context_eval(ctx, source.as_ptr(), std::ptr::null_mut(), &mut error) };
        assert_eq!(status, 0);
        if !error.is_null() {
            unsafe { rune_free_string(error) };
        }
        let handle = CAPTURED.load(Ordering::SeqCst);
        assert_ne!(handle, 0);
        let obj = RuneValue {
            kind: RUNE_OBJECT,
            boolean: 0,
            number: 0.0,
            string: std::ptr::null(),
            handle,
        };
        let x = unsafe { get(ctx, obj, "x") };
        assert_eq!(x.kind, RUNE_NUMBER);
        assert_eq!(x.number, 42.0);

        let array = unsafe { eval_value(ctx, "[10, 20, 30]") };
        let length = unsafe { get(ctx, array, "length") };
        assert_eq!(length.number, 3.0);
        let second = unsafe { get(ctx, array, "1") };
        assert_eq!(second.number, 20.0);

        let key = CString::new("1").unwrap();
        let value = RuneValue {
            kind: RUNE_NUMBER,
            boolean: 0,
            number: 99.0,
            string: std::ptr::null(),
            handle: 0,
        };
        let mut error: *mut c_char = std::ptr::null_mut();
        let status = unsafe {
            rune_context_set_property(
                ctx,
                array,
                key.as_ptr(),
                value,
                &mut error as *mut *mut c_char,
            )
        };
        assert_eq!(status, 0);
        let changed = unsafe { get(ctx, array, "1") };
        assert_eq!(changed.number, 99.0);

        unsafe {
            rune_context_register_host_fn(
                ctx,
                c"identity".as_ptr(),
                1,
                std::ptr::null_mut(),
                Some(identity_cb),
            );
        }
        let source = CString::new("var o = { y: 7 }; identity(o) === o;").unwrap();
        let mut answer: *mut c_char = std::ptr::null_mut();
        let status = unsafe { rune_context_eval(ctx, source.as_ptr(), &mut answer, &mut error) };
        assert_eq!(status, 0);
        let text = unsafe { CStr::from_ptr(answer) }
            .to_string_lossy()
            .into_owned();
        unsafe { rune_free_string(answer) };
        assert_eq!(text, "true");

        unsafe {
            rune_context_release_value(ctx, obj);
            rune_context_release_value(ctx, array);
            rune_context_release_value(ctx, length);
            rune_context_release_value(ctx, second);
        }
        rune_context_destroy(ctx);
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::*;

    #[test]
    fn installs_and_runs_timers() {
        let ctx = rune_context_create();
        assert_eq!(unsafe { rune_context_install_runtime(ctx) }, 0);
        let source =
            CString::new("var done = 0; setTimeout(function () { done = 1; }, 0);").unwrap();
        let mut error: *mut c_char = std::ptr::null_mut();
        let status =
            unsafe { rune_context_eval(ctx, source.as_ptr(), std::ptr::null_mut(), &mut error) };
        assert_eq!(status, 0);
        if !error.is_null() {
            unsafe { rune_free_string(error) };
        }
        assert_eq!(unsafe { rune_context_run_jobs(ctx, &mut error) }, 0);
        if !error.is_null() {
            unsafe { rune_free_string(error) };
        }

        let check = CString::new("done").unwrap();
        let mut result: *mut c_char = std::ptr::null_mut();
        let status = unsafe { rune_context_eval(ctx, check.as_ptr(), &mut result, &mut error) };
        assert_eq!(status, 0);
        let text = unsafe { CStr::from_ptr(result) }
            .to_string_lossy()
            .into_owned();
        unsafe { rune_free_string(result) };
        assert_eq!(text, "1");
        rune_context_destroy(ctx);
    }
}

#[cfg(test)]
mod fetch_tests {
    use super::*;

    unsafe extern "C" fn fetch_cb(
        _user: *mut c_void,
        _method: *const c_char,
        _url: *const c_char,
        _headers: *const c_char,
        _body: *const c_char,
        response: *mut RuneFetchResponse,
    ) -> i32 {
        unsafe {
            (*response).status = 200;
            (*response).body = c"hello".as_ptr();
            (*response).status_text = c"OK".as_ptr();
            (*response).headers = c"X-Test\t1\n".as_ptr();
        }
        0
    }

    #[test]
    fn c_fetch_transport_resolves_promises() {
        let ctx = rune_context_create();
        assert_eq!(unsafe { rune_context_install_runtime(ctx) }, 0);
        assert_eq!(
            unsafe { rune_context_set_fetcher(ctx, std::ptr::null_mut(), Some(fetch_cb)) },
            0
        );
        let source = CString::new(
            "var out = ''; fetch('https://example.com/x').then(function (r) { return r.text().then(function (t) { out = r.status + ':' + t; }); });",
        )
        .unwrap();
        let mut error: *mut c_char = std::ptr::null_mut();
        let status =
            unsafe { rune_context_eval(ctx, source.as_ptr(), std::ptr::null_mut(), &mut error) };
        assert_eq!(status, 0);
        if !error.is_null() {
            unsafe { rune_free_string(error) };
        }
        assert_eq!(unsafe { rune_context_run_jobs(ctx, &mut error) }, 0);
        if !error.is_null() {
            unsafe { rune_free_string(error) };
        }
        let check = CString::new("out").unwrap();
        let mut result: *mut c_char = std::ptr::null_mut();
        let status = unsafe { rune_context_eval(ctx, check.as_ptr(), &mut result, &mut error) };
        assert_eq!(status, 0);
        let text = unsafe { CStr::from_ptr(result) }
            .to_string_lossy()
            .into_owned();
        unsafe { rune_free_string(result) };
        assert_eq!(text, "200:hello");
        rune_context_destroy(ctx);
    }
}
