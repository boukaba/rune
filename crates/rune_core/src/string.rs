use crate::gc::{GcHeader, SemiSpace, TAG_STRING, size_of};
use std::fmt;
use std::ptr;

/// A GC-allocated flat UTF-16 string.
///
/// Memory layout:
///   [GcHeader | len: u32 | u16 data... ]
pub struct HeapString;

impl HeapString {
    pub fn allocate(ss: &mut SemiSpace, text: &str) -> *mut HeapString {
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let len = utf16.len();
        let obj_size = size_of::<GcHeader>() + size_of::<u32>() + len * 2;
        let ptr = ss.alloc(obj_size) as *mut u8;
        unsafe {
            let header = &mut *(ptr as *mut GcHeader);
            header.word = std::sync::atomic::AtomicU64::new(TAG_STRING);

            let len_ptr = ptr.add(size_of::<GcHeader>()) as *mut u32;
            *len_ptr = len as u32;

            let data_ptr = ptr.add(size_of::<GcHeader>() + size_of::<u32>()) as *mut u16;
            ptr::copy_nonoverlapping(utf16.as_ptr(), data_ptr, len);
        }
        ptr as *mut HeapString
    }

    pub unsafe fn from_ptr(ptr: *mut HeapString) -> &'static Self {
        unsafe { &*ptr }
    }

    pub unsafe fn len(ptr: *mut HeapString) -> usize {
        unsafe {
            let ptr = ptr as *mut u8;
            let len_ptr = ptr.add(size_of::<GcHeader>()) as *const u32;
            *len_ptr as usize
        }
    }

    pub unsafe fn data(ptr: *mut HeapString) -> *const u16 {
        unsafe {
            let ptr = ptr as *mut u8;
            ptr.add(size_of::<GcHeader>() + size_of::<u32>()) as *const u16
        }
    }

    pub unsafe fn to_string(ptr: *mut HeapString) -> String {
        unsafe {
            let len = Self::len(ptr);
            let data = Self::data(ptr);
            let mut s = String::with_capacity(len);
            let mut i = 0;
            while i < len {
                let cp = *data.add(i) as u32;
                if (0xD800..=0xDBFF).contains(&cp) && i + 1 < len {
                    let low = *data.add(i + 1) as u32;
                    if (0xDC00..=0xDFFF).contains(&low) {
                        let code_point = 0x10000 + (cp - 0xD800) * 0x400 + (low - 0xDC00);
                        if let Some(c) = char::from_u32(code_point) {
                            s.push(c);
                        }
                        i += 2;
                        continue;
                    }
                }
                if !(0xDC00..=0xDFFF).contains(&cp) {
                    if let Some(c) = char::from_u32(cp) {
                        s.push(c);
                    }
                }
                i += 1;
            }
            s
        }
    }
}

impl fmt::Debug for HeapString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<HeapString>")
    }
}

impl fmt::Display for HeapString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<HeapString>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gc::SemiSpace;

    #[test]
    fn test_alloc_string() {
        let mut ss = SemiSpace::new();
        let s = HeapString::allocate(&mut ss, "hello");
        unsafe {
            assert_eq!(HeapString::len(s), 5);
            assert_eq!(HeapString::to_string(s), "hello");
        }
    }

    #[test]
    fn test_alloc_unicode() {
        let mut ss = SemiSpace::new();
        let s = HeapString::allocate(&mut ss, "héllo 🔥");
        unsafe {
            assert!(HeapString::len(s) > 5);
            let roundtrip = HeapString::to_string(s);
            assert_eq!(roundtrip, "héllo 🔥");
        }
    }
}
