//! Stable C ABI for embedding Rune.
//!
//! The boundary uses only opaque pointers, C strings, integers, and status
//! codes. Panics are caught so they never unwind across FFI.

use rune_embed::Context;
use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

const STATUS_OK: i32 = 0;
const STATUS_ERROR: i32 = 1;
const STATUS_INVALID: i32 = -1;

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
