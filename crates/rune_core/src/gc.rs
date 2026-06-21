use std::alloc::Layout;
use std::sync::atomic::{AtomicU64, Ordering};

/// Object type tags (stored in low 3 bits of header word).
pub const TAG_OBJECT: u64 = 0;
pub const TAG_STRING: u64 = 1;
pub const TAG_FUNC: u64 = 2;
pub const TAG_FLOAT64: u64 = 3;
pub const TAG_FORWARDED: u64 = 7;

/// Tag bits mask for GC header tag.
const GC_TAG_MASK: u64 = 0b111;

/// Tag bits mask for full-word tagging (used in Value checks).
const TAG_MASK: u64 = 0x03;

/// Per-object GC header. Every GC-allocated object starts with this.
#[repr(C)]
pub struct GcHeader {
    pub word: AtomicU64,
}

impl GcHeader {
    pub fn new(tag: u64) -> Self {
        GcHeader { word: AtomicU64::new(tag) }
    }

    pub fn tag(&self) -> u64 {
        self.word.load(Ordering::Relaxed) & GC_TAG_MASK
    }

    pub fn is_forwarded(&self) -> bool {
        self.word.load(Ordering::Relaxed) & GC_TAG_MASK == TAG_FORWARDED
    }

    pub fn forwarding_addr(&self) -> *mut u8 {
        (self.word.load(Ordering::Relaxed) & !GC_TAG_MASK) as *mut u8
    }

    pub fn set_forwarding(&self, to: *mut u8) {
        self.word.store((to as u64) | TAG_FORWARDED, Ordering::Release);
    }
}

/// Size of each semispace region in bytes.
const SEMISPACE_SIZE: usize = 4 * 1024 * 1024; // 4 MiB
/// Offset in bytes from object start to prototype pointer (matches object.rs OBJECT_HEADER_PROTOTYPE)
const OBJECT_PROTOTYPE_OFFSET: usize = 24;
/// Offset in bytes from object start to first property slot (matches object.rs OBJECT_HEADER_END)
const OBJECT_SLOTS_OFFSET: usize = 32;

/// A simple Cheney-style semispace copying GC.
///
/// GC is always manual — call `collect()` after registering roots
/// via `push_root()`. Alloc panics if the semispace runs out of room
/// and GC has not been called.
pub struct SemiSpace {
    regions: [*mut u8; 2],
    active: usize,
    bump: *mut u8,
    limit: *mut u8,
    scan: *mut u8,
    roots: Vec<*mut u64>,
}

unsafe impl Send for SemiSpace {}

impl SemiSpace {
    pub fn new() -> Self {
        let r0 = unsafe { alloc_zeroed(SEMISPACE_SIZE) };
        let r1 = unsafe { alloc_zeroed(SEMISPACE_SIZE) };
        SemiSpace {
            regions: [r0, r1],
            active: 0,
            bump: r0,
            limit: unsafe { r0.add(SEMISPACE_SIZE) },
            scan: r0,
            roots: Vec::new(),
        }
    }

    /// Register a root slot (pointer to a Value's raw u64).
    /// The caller must ensure the slot remains valid until the next `collect()`.
    pub fn push_root(&mut self, slot: *mut u64) {
        self.roots.push(slot);
    }

    pub fn pop_root(&mut self) {
        self.roots.pop();
    }

    /// Clear all registered roots.
    pub fn clear_roots(&mut self) {
        self.roots.clear();
    }

    /// Bytes remaining in the active semispace.
    pub fn remaining(&self) -> usize {
        unsafe { self.limit.offset_from(self.bump) as usize }
    }

    /// Allocate `size` bytes in the active semispace.
    /// Automatically triggers a Cheney-style copying collection if there's
    /// insufficient space. The caller must have registered roots via `push_root()`
    /// before any allocation that may trigger GC.
    pub fn alloc(&mut self, size: usize) -> *mut u8 {
        let aligned = align_up(size, 8);
        let ptr = self.bump;
        let next = unsafe { ptr.add(aligned) };
        if next > self.limit {
            if self.roots.is_empty() {
                panic!(
                    "GC: out of memory (need {aligned} bytes, {} remaining) and no roots registered.",
                    self.remaining()
                );
            }
            self.collect();
            let ptr2 = self.bump;
            let next2 = unsafe { ptr2.add(aligned) };
            if next2 > self.limit {
                panic!(
                    "GC: still out of memory after collection (need {aligned} bytes, have {} remaining).",
                    self.remaining()
                );
            }
            self.bump = next2;
            return ptr2;
        }
        self.bump = next;
        ptr
    }

    /// Cheney's copying GC algorithm.
    /// Root slots are updated in-place, so they remain valid after GC.
    pub fn collect(&mut self) {
        if self.roots.is_empty() {
            // Nothing to preserve; reset bump pointer
            self.active = 1 - self.active;
            self.bump = self.regions[self.active];
            self.limit = unsafe { self.bump.add(SEMISPACE_SIZE) };
            self.scan = self.bump;
            return;
        }

        let to = self.regions[1 - self.active];
        self.scan = to;
        self.bump = to;
        self.active = 1 - self.active;
        self.limit = unsafe { to.add(SEMISPACE_SIZE) };

        unsafe {
            // Forward all root objects; also update root slots in-place
            let root_slots: Vec<*mut u64> = self.roots.clone();
            for &slot in &root_slots {
                if slot.is_null() {
                    continue;
                }
                self.forward_value(slot);
            }

            // Cheney scan: iterate over objects in to-space
            while self.scan < self.bump {
                let scan_ptr = self.scan;
                let header = &*(scan_ptr as *const GcHeader);
                let tag = header.tag();
                let obj_end = self.scan_end(scan_ptr, tag);

                match tag {
                    TAG_OBJECT => {
                        // Forward prototype pointer (if non-null)
                        let proto_ptr = scan_ptr.add(OBJECT_PROTOTYPE_OFFSET) as *mut u64;
                        self.forward_value(proto_ptr);
                        // Forward property slots
                        let slots_ptr = scan_ptr.add(OBJECT_SLOTS_OFFSET) as *mut u64;
                        let capacity_ptr = scan_ptr.add(16) as *const u32;
                        let cap = *capacity_ptr as usize;
                        for i in 0..cap {
                            self.forward_value(slots_ptr.add(i));
                        }
                    }
                    TAG_STRING => {}
                    _ => {}
                }

                self.scan = obj_end;
            }
        }
    }

    /// Compute the end address of an object given its start and type tag.
    unsafe fn scan_end(&self, obj_start: *mut u8, tag: u64) -> *mut u8 {
        unsafe {
            match tag {
                TAG_STRING => {
                    let len_ptr = obj_start.add(8) as *const u32;
                    let len = *len_ptr as usize;
                    let total = size_of::<GcHeader>() + 4 + len * 2;
                    obj_start.add(align_up(total, 8))
                }
                TAG_FUNC => {
                    obj_start.add(align_up(16, 8))
                }
                TAG_FLOAT64 => {
                    obj_start.add(size_of::<GcHeader>() + 8)
                }
                TAG_OBJECT => {
                    let capacity_ptr = obj_start.add(16) as *const u32;
                    let capacity = *capacity_ptr as usize;
                    let total = OBJECT_SLOTS_OFFSET + capacity * size_of::<u64>();
                    obj_start.add(align_up(total, 8))
                }
                _ => obj_start.add(8),
            }
        }
    }

    /// Forward a Value slot: if it points to a heap object, copy it to to-space
    /// and update the slot with the new address.
    unsafe fn forward_value(&mut self, slot: *mut u64) {
        unsafe {
            let raw = *slot;
            if raw & TAG_MASK == 0 && raw != 0 && raw != 2 {
                let obj = raw as *mut GcHeader;
                let new_addr = self.forward_object(obj);
                *slot = new_addr as u64;
            }
        }
    }

    /// Copy an object from from-space to to-space if not already forwarded.
    unsafe fn forward_object(&mut self, obj: *mut GcHeader) -> *mut u8 {
        unsafe {
            if (*obj).is_forwarded() {
                return (*obj).forwarding_addr();
            }

            let obj_addr = obj as *mut u8;
            let tag = (*obj).tag();
            let obj_size = self.scan_end(obj_addr, tag) as usize - obj_addr as usize;
            let aligned = align_up(obj_size, 8);

            let to_addr = self.bump;
            let end = to_addr.add(aligned);
            if end > self.limit {
                panic!("GC: to-space exhausted during collection");
            }
            std::ptr::copy_nonoverlapping(obj_addr, to_addr, obj_size);
            self.bump = end;

            (*obj).set_forwarding(to_addr);

            to_addr
        }
    }

    /// After GC, return a pointer to the to-space for verification.
    #[allow(dead_code)]
    pub fn current_region(&self) -> *mut u8 {
        self.regions[self.active]
    }
}

unsafe fn alloc_zeroed(size: usize) -> *mut u8 {
    unsafe {
        let layout = Layout::from_size_align(size, 4096).unwrap();
        let ptr = std::alloc::alloc_zeroed(layout);
        if ptr.is_null() {
            panic!("GC: failed to allocate {} bytes", size);
        }
        ptr
    }
}

fn align_up(size: usize, align: usize) -> usize {
    (size + align - 1) & !(align - 1)
}

pub const fn size_of<T>() -> usize {
    std::mem::size_of::<T>()
}

impl Default for SemiSpace {
    fn default() -> Self {
        Self::new()
    }
}
