use crate::gc::{GcHeader, SemiSpace, TAG_ENV, size_of, align_up};
use crate::value::Value;

/// A GC-allocated lexical environment object for closure capture.
///
/// Memory layout:
///   [GcHeader(8) | slot_count(4) | _pad(4) | parent(8) | slots(slot_count * 8)]
///
/// - parent: pointer to parent EnvObject (null for the global env)
/// - slots: array of Value slots for captured variables
pub struct EnvObject;

impl EnvObject {
    /// Allocate an EnvObject with the given number of slots and parent pointer.
    /// Layout: GcHeader(8) | slot_count(4) | _pad(4) | parent(8) | slots[slot_count]
    pub fn allocate(
        ss: &mut SemiSpace,
        slot_count: usize,
        parent: *mut EnvObject,
    ) -> *mut EnvObject {
        let total = align_up(24 + slot_count * 8, 8);
        let ptr = ss.alloc(total);
        unsafe {
            let header = &mut *(ptr as *mut GcHeader);
            header.word = std::sync::atomic::AtomicU64::new(TAG_ENV);
            let count_ptr = ptr.add(size_of::<GcHeader>()) as *mut u32;
            *count_ptr = slot_count as u32;
            // parent at offset 16 from allocation start
            let parent_ptr = ptr.add(16) as *mut u64;
            *parent_ptr = parent as u64;
        }
        ptr as *mut EnvObject
    }

    /// Get the number of slots in this environment.
    pub unsafe fn slot_count(env: *mut EnvObject) -> usize {
        unsafe {
            let ptr = env as *mut u8;
            *(ptr.add(size_of::<GcHeader>()) as *const u32) as usize
        }
    }

    /// Get the parent environment pointer (may be null).
    pub unsafe fn parent(env: *mut EnvObject) -> *mut EnvObject {
        unsafe {
            let ptr = env as *mut u8;
            let raw = *(ptr.add(size_of::<GcHeader>() + 8) as *const u64);
            raw as *mut EnvObject
        }
    }

    /// Set the parent environment pointer.
    pub unsafe fn set_parent(env: *mut EnvObject, parent: *mut EnvObject) {
        unsafe {
            let ptr = env as *mut u8;
            let parent_ptr = ptr.add(size_of::<GcHeader>() + 8) as *mut u64;
            *parent_ptr = parent as u64;
        }
    }

    /// Get the pointer to the start of the slots array (offset 24 from allocation start).
    unsafe fn slots_ptr(env: *mut EnvObject) -> *mut Value {
        unsafe {
            let ptr = env as *mut u8;
            ptr.add(24) as *mut Value
        }
    }

    /// Read a slot value by index.
    pub unsafe fn get_slot(env: *mut EnvObject, index: usize) -> Value {
        unsafe {
            *Self::slots_ptr(env).add(index)
        }
    }

    /// Write a slot value by index.
    pub unsafe fn set_slot(env: *mut EnvObject, index: usize, val: Value) {
        unsafe {
            *Self::slots_ptr(env).add(index) = val;
        }
    }

    /// Walk up the environment chain by `depth` steps and return the env at that level.
    /// Returns null if depth exceeds the chain.
    pub unsafe fn ancestor(env: *mut EnvObject, depth: usize) -> *mut EnvObject {
        unsafe {
            let mut current = env;
            for _ in 0..depth {
                if current.is_null() {
                    return std::ptr::null_mut();
                }
                current = Self::parent(current);
            }
            current
        }
    }
}
