use rune_embed::Context;
use std::os::raw::c_char;

/// Opaque handle to a Rune context.
/// Must be freed with `rune_context_destroy`.
#[unsafe(no_mangle)]
pub extern "C" fn rune_context_create() -> *mut std::ffi::c_void {
    let ctx = Box::new(Context::new());
    Box::into_raw(ctx) as *mut std::ffi::c_void
}

#[unsafe(no_mangle)]
pub extern "C" fn rune_context_destroy(ctx: *mut std::ffi::c_void) {
    if !ctx.is_null() {
        unsafe {
            let _ = Box::from_raw(ctx as *mut Context);
        }
    }
}

/// Evaluate JavaScript source code and return the result as a C string.
/// Caller must free with `rune_free_string`.
///
/// # Safety
/// `ctx` must be a valid pointer returned by `rune_context_create`.
/// `source` must be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_context_eval(
    ctx: *mut std::ffi::c_void,
    source: *const c_char,
) -> *const c_char {
    if ctx.is_null() || source.is_null() {
        return std::ptr::null();
    }
    let ctx = unsafe { &mut *(ctx as *mut Context) };
    let src = unsafe { std::ffi::CStr::from_ptr(source) };
    let src_str = src.to_string_lossy().to_string();

    let result = match ctx.eval(&src_str) {
        Ok(val) => format!("{:?}", val),
        Err(e) => format!("Error: {e}"),
    };

    let c_str = std::ffi::CString::new(result).unwrap_or_default();
    c_str.into_raw()
}

/// Free a string returned by `rune_context_eval`.
///
/// # Safety
/// `s` must be a valid pointer returned by `rune_context_eval` that has not been freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rune_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe { let _ = std::ffi::CString::from_raw(s); }
    }
}
