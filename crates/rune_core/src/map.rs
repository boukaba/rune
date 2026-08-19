use crate::gc::{GcHeader, SemiSpace, TAG_MAP, TAG_SET};
use std::sync::atomic::Ordering;

pub const MAP_SIZE: usize = 32;
pub const SET_SIZE: usize = 32;

/// Heap-allocated Map object.
/// Layout: [GcHeader(8) | entries_ptr(8) | size:u32(4) | pad(4) | prototype(8)] = 32 bytes
/// `entries` is a RuneArray whose elements are 2-slot RuneArrays [key, value];
/// a deleted entry has its key slot set to `Value::empty_sentinel()`.
#[repr(C)]
pub struct RuneMap {
    header: GcHeader,
    entries: *mut u8,
    size: u32,
    _pad: u32,
    prototype: *mut u8,
}

/// Heap-allocated Set object.
/// Layout: [GcHeader(8) | entries_ptr(8) | size:u32(4) | pad(4) | prototype(8)] = 32 bytes
/// `entries` is a RuneArray of element Values; a deleted entry is the
/// `Value::empty_sentinel()` marker.
#[repr(C)]
pub struct RuneSet {
    header: GcHeader,
    entries: *mut u8,
    size: u32,
    _pad: u32,
    prototype: *mut u8,
}

impl RuneMap {
    pub fn allocate(gc: &mut SemiSpace, prototype: *mut u8) -> *mut u8 {
        let ptr = gc.alloc(MAP_SIZE);
        unsafe {
            let hdr = ptr as *mut GcHeader;
            (*hdr).word.store(TAG_MAP, Ordering::Relaxed);
            let m = ptr as *mut RuneMap;
            (*m).entries = std::ptr::null_mut();
            (*m).size = 0;
            (*m)._pad = 0;
            (*m).prototype = prototype;
        }
        ptr
    }

    pub unsafe fn entries(ptr: *mut u8) -> *mut u8 {
        unsafe { (*(ptr as *mut RuneMap)).entries }
    }

    pub unsafe fn set_entries(ptr: *mut u8, entries: *mut u8) {
        unsafe {
            (*(ptr as *mut RuneMap)).entries = entries;
        }
    }

    pub unsafe fn size(ptr: *mut u8) -> u32 {
        unsafe { (*(ptr as *mut RuneMap)).size }
    }

    pub unsafe fn set_size(ptr: *mut u8, size: u32) {
        unsafe {
            (*(ptr as *mut RuneMap)).size = size;
        }
    }

    pub unsafe fn prototype(ptr: *mut u8) -> *mut u8 {
        unsafe { (*(ptr as *mut RuneMap)).prototype }
    }

    pub unsafe fn set_prototype(ptr: *mut u8, proto: *mut u8) {
        unsafe {
            (*(ptr as *mut RuneMap)).prototype = proto;
        }
    }
}

impl RuneSet {
    pub fn allocate(gc: &mut SemiSpace, prototype: *mut u8) -> *mut u8 {
        let ptr = gc.alloc(SET_SIZE);
        unsafe {
            let hdr = ptr as *mut GcHeader;
            (*hdr).word.store(TAG_SET, Ordering::Relaxed);
            let s = ptr as *mut RuneSet;
            (*s).entries = std::ptr::null_mut();
            (*s).size = 0;
            (*s)._pad = 0;
            (*s).prototype = prototype;
        }
        ptr
    }

    pub unsafe fn entries(ptr: *mut u8) -> *mut u8 {
        unsafe { (*(ptr as *mut RuneSet)).entries }
    }

    pub unsafe fn set_entries(ptr: *mut u8, entries: *mut u8) {
        unsafe {
            (*(ptr as *mut RuneSet)).entries = entries;
        }
    }

    pub unsafe fn size(ptr: *mut u8) -> u32 {
        unsafe { (*(ptr as *mut RuneSet)).size }
    }

    pub unsafe fn set_size(ptr: *mut u8, size: u32) {
        unsafe {
            (*(ptr as *mut RuneSet)).size = size;
        }
    }

    pub unsafe fn prototype(ptr: *mut u8) -> *mut u8 {
        unsafe { (*(ptr as *mut RuneSet)).prototype }
    }

    pub unsafe fn set_prototype(ptr: *mut u8, proto: *mut u8) {
        unsafe {
            (*(ptr as *mut RuneSet)).prototype = proto;
        }
    }
}
