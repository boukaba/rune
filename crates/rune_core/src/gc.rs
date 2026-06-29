use std::alloc::Layout;
use std::sync::atomic::{AtomicU64, Ordering};

/// Object type tags (stored in low 4 bits of header word).
pub const TAG_OBJECT: u64 = 0;
pub const TAG_STRING: u64 = 1;
pub const TAG_FUNC: u64 = 2;
pub const TAG_FLOAT64: u64 = 3;
pub const TAG_ARRAY: u64 = 4;
pub const TAG_ENV: u64 = 5;
pub const TAG_STRING_OBJ: u64 = 6;
pub const TAG_FORWARDED: u64 = 7;
pub const TAG_PROMISE: u64 = 8;
pub const TAG_REGEXP: u64 = 9;
pub const TAG_ACCESSOR: u64 = 10;

/// Tag bits mask for GC header tag.
pub const GC_TAG_MASK: u64 = 0b1111;

/// Float Self-Tagging constants (mirrors value.rs).
const FST_QNAN_TOP: u64 = 0x7FF8;
const FST_TAG_SHIFT: u64 = 45;
const FST_TAG_MASK: u64 = 0x7;
const FST_HEAP_PTR_TAG: u64 = 1;
const FST_PAYLOAD_MASK: u64 = (1 << 45) - 1;

/// Per-object GC header. Every GC-allocated object starts with this.
#[repr(C)]
pub struct GcHeader {
    pub word: AtomicU64,
}

impl GcHeader {
    pub fn new(tag: u64) -> Self {
        GcHeader {
            word: AtomicU64::new(tag),
        }
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
        self.word
            .store((to as u64) | TAG_FORWARDED, Ordering::Release);
    }
}

/// Size of each semispace region in bytes.
const SEMISPACE_SIZE: usize = 16 * 1024 * 1024; // 16 MiB
/// Offset in bytes from object start to prototype pointer (matches object.rs OBJECT_HEADER_PROTOTYPE)
const OBJECT_PROTOTYPE_OFFSET: usize = 24;
/// Offset in bytes from object start to first property slot (matches object.rs OBJECT_HEADER_END)
const OBJECT_SLOTS_OFFSET: usize = 32;
/// StringObject layout constants (matches string_object.rs).
const STRING_OBJ_PROTOTYPE_OFFSET: usize = 8;
const STRING_OBJ_STRING_PTR_OFFSET: usize = 16;
const STRING_OBJ_TOTAL_SIZE: usize = 24;

/// Trait for providing current GC root slots.
/// Implemented by Vm to register stack/frame/locals roots.
pub trait RootProvider {
    fn register_roots(&mut self, gc: &mut SemiSpace);
}

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
    semispace_size: usize,
    /// Optional root provider called before each collection to refresh
    /// the root set (handles Vec reallocation invalidating root pointers).
    pub root_provider: Option<*mut dyn RootProvider>,
}

unsafe impl Send for SemiSpace {}

impl SemiSpace {
    pub fn new() -> Self {
        Self::with_size(SEMISPACE_SIZE)
    }

    /// Create a SemiSpace with a custom semispace size.
    /// Each of the two semispaces is `size` bytes (total allocation = 2 * size).
    pub fn with_size(size: usize) -> Self {
        let r0 = unsafe { alloc_zeroed(size) };
        let r1 = unsafe { alloc_zeroed(size) };
        SemiSpace {
            regions: [r0, r1],
            active: 0,
            bump: r0,
            limit: unsafe { r0.add(size) },
            scan: r0,
            roots: Vec::new(),
            semispace_size: size,
            root_provider: None,
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
            if let Some(provider) = self.root_provider {
                unsafe {
                    (*provider).register_roots(self);
                }
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
            self.limit = unsafe { self.bump.add(self.semispace_size) };
            self.scan = self.bump;
            return;
        }

        let to = self.regions[1 - self.active];
        self.scan = to;
        self.bump = to;
        self.active = 1 - self.active;
        self.limit = unsafe { to.add(self.semispace_size) };

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
                        // Object layout: offset +16 = capacity, offset +20 = slot_count
                        let cap = *(scan_ptr.add(16) as *const u32) as usize;
                        for i in 0..cap {
                            self.forward_value(slots_ptr.add(i));
                        }
                    }
                    TAG_ARRAY => {
                        // Forward prototype pointer (if non-null)
                        let proto_ptr = scan_ptr.add(OBJECT_PROTOTYPE_OFFSET) as *mut u64;
                        self.forward_value(proto_ptr);
                        // Forward array elements
                        let slots_ptr = scan_ptr.add(OBJECT_SLOTS_OFFSET) as *mut u64;
                        // Array layout: offset +16 = length, offset +20 = capacity
                        let cap = *(scan_ptr.add(20) as *const u32) as usize;
                        for i in 0..cap {
                            self.forward_value(slots_ptr.add(i));
                        }
                    }
                    TAG_FUNC => {
                        // Forward the prototype pointer (byte offset 24 from object start)
                        let proto_ptr = scan_ptr.add(size_of::<GcHeader>() + 16) as *mut u64;
                        self.forward_value(proto_ptr);
                        // Forward the environment pointer (byte offset 40 from object start)
                        let env_ptr = scan_ptr.add(40) as *mut u64;
                        self.forward_value(env_ptr);
                        // Forward the superclass pointer (byte offset 56 from object start)
                        let super_ptr = scan_ptr.add(56) as *mut u64;
                        self.forward_value(super_ptr);
                        // Forward the extra_props pointer (byte offset 64 from object start)
                        let props_ptr = scan_ptr.add(64) as *mut u64;
                        self.forward_value(props_ptr);
                    }
                    TAG_ENV => {
                        // Forward parent pointer at byte offset 16 from object start
                        let parent_ptr = scan_ptr.add(16) as *mut u64;
                        self.forward_value(parent_ptr);
                        // Forward each slot (slots start at offset 24)
                        let count = *(scan_ptr.add(size_of::<GcHeader>()) as *const u32) as usize;
                        let slots_ptr = scan_ptr.add(24) as *mut u64;
                        for i in 0..count {
                            self.forward_value(slots_ptr.add(i));
                        }
                    }
                    TAG_STRING => {}
                    TAG_STRING_OBJ => {
                        let str_ptr = scan_ptr.add(STRING_OBJ_STRING_PTR_OFFSET) as *mut u64;
                        self.forward_value(str_ptr);
                        let proto_ptr = scan_ptr.add(STRING_OBJ_PROTOTYPE_OFFSET) as *mut u64;
                        self.forward_value(proto_ptr);
                    }
                    TAG_PROMISE => {
                        let result_ptr = scan_ptr.add(size_of::<GcHeader>() + 8) as *mut u64;
                        self.forward_value(result_ptr);
                        let proto_ptr = scan_ptr.add(size_of::<GcHeader>() + 16) as *mut u64;
                        self.forward_value(proto_ptr);
                        let reactions_ptr = scan_ptr.add(size_of::<GcHeader>() + 24) as *mut u64;
                        self.forward_value(reactions_ptr);
                    }
                    TAG_REGEXP => {
                        let pattern_ptr = scan_ptr.add(size_of::<GcHeader>()) as *mut u64;
                        self.forward_value(pattern_ptr);
                        let proto_ptr = scan_ptr.add(24) as *mut u64;
                        self.forward_value(proto_ptr);
                    }
                    TAG_ACCESSOR => {
                        let getter_ptr = scan_ptr.add(size_of::<GcHeader>()) as *mut u64;
                        self.forward_value(getter_ptr);
                        let setter_ptr = scan_ptr.add(size_of::<GcHeader>() + 8) as *mut u64;
                        self.forward_value(setter_ptr);
                    }
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
                    // Func layout: GcHeader(8) + func_idx(8) + prog_ptr(8) + prototype(8)
                    //   + call_count(4) + flags(4) + env_ptr(8) + jit_entry(8)
                    //   + superclass(8) + extra_props(8) = 72 bytes
                    obj_start.add(72)
                }
                TAG_FLOAT64 => obj_start.add(size_of::<GcHeader>() + 8),
                TAG_OBJECT => {
                    let capacity_ptr = obj_start.add(16) as *const u32;
                    let capacity = *capacity_ptr as usize;
                    let total = OBJECT_SLOTS_OFFSET + capacity * size_of::<u64>();
                    obj_start.add(align_up(total, 8))
                }
                TAG_ARRAY => {
                    // Array layout: offset +20 = capacity
                    let capacity_ptr = obj_start.add(20) as *const u32;
                    let capacity = *capacity_ptr as usize;
                    let total = OBJECT_SLOTS_OFFSET + capacity * size_of::<u64>();
                    obj_start.add(align_up(total, 8))
                }
                TAG_ENV => {
                    let count = *(obj_start.add(size_of::<GcHeader>()) as *const u32) as usize;
                    let total = 24 + count * size_of::<u64>();
                    obj_start.add(align_up(total, 8))
                }
                TAG_STRING_OBJ => obj_start.add(STRING_OBJ_TOTAL_SIZE),
                TAG_PROMISE => obj_start.add(crate::promise::PROMISE_SIZE),
                TAG_REGEXP => obj_start.add(32),
                TAG_ACCESSOR => obj_start.add(crate::accessor::ACCESSOR_SIZE),
                _ => obj_start.add(8),
            }
        }
    }

    /// Forward a Value slot: if it points to a heap object, copy it to to-space
    /// and update the slot with the new address.
    unsafe fn forward_value(&mut self, slot: *mut u64) {
        unsafe {
            let raw = *slot;
            // Check for Float Self-Tagging encoded heap pointer
            if (raw >> 48) == FST_QNAN_TOP
                && ((raw >> FST_TAG_SHIFT) & FST_TAG_MASK) == FST_HEAP_PTR_TAG
            {
                let obj = ((raw & FST_PAYLOAD_MASK) << 3) as *mut GcHeader;
                let new_addr = self.forward_object(obj);
                *slot = 0x7FF8_0000_0000_0000u64
                    | (FST_HEAP_PTR_TAG << FST_TAG_SHIFT)
                    | ((new_addr as u64) >> 3);
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

pub(crate) fn align_up(size: usize, align: usize) -> usize {
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
