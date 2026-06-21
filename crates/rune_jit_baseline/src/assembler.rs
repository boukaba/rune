/// RW→RX memory management for JIT code.

pub struct Assembler;

impl Assembler {
    pub fn new() -> Self {
        Assembler
    }

    pub fn allocate(&self, _size: usize) -> *mut u8 {
        std::ptr::null_mut()
    }

    pub fn finalize(&self, _ptr: *mut u8, _size: usize) {}
}
