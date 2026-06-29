use crate::gc::{GcHeader, SemiSpace, TAG_REGEXP};
use std::sync::atomic::Ordering;

pub const REGEXP_SIZE: usize = 24;

/// Heap-allocated RegExp object.
/// Layout: [GcHeader(8) | pattern_ptr(8) | flags:u32(4) | pad(4)] = 24 bytes
#[repr(C)]
pub struct RegExp {
    header: GcHeader,
    pattern: *mut u8,
    flags: u32,
    _pad: u32,
}

impl RegExp {
    pub fn allocate(gc: &mut SemiSpace, pattern: *mut u8, flags: u32) -> *mut u8 {
        let ptr = gc.alloc(REGEXP_SIZE);
        unsafe {
            let hdr = ptr as *mut GcHeader;
            (*hdr).word.store(TAG_REGEXP, Ordering::Relaxed);
            let re = ptr as *mut RegExp;
            (*re).pattern = pattern;
            (*re).flags = flags;
            (*re)._pad = 0;
        }
        ptr
    }

    pub unsafe fn pattern(ptr: *mut u8) -> *mut u8 {
        unsafe { (*(ptr as *mut RegExp)).pattern }
    }

    pub unsafe fn flags(ptr: *mut u8) -> u32 {
        unsafe { (*(ptr as *mut RegExp)).flags }
    }

    pub unsafe fn has_flag(ptr: *mut u8, flag: u8) -> bool {
        (unsafe { (*(ptr as *mut RegExp)).flags } & (1u32 << (flag as u32))) != 0
    }
}
