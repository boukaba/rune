/// GC integration module.
/// Exports the concrete GC type (SemiSpace) used by the runtime.
pub use crate::gc::SemiSpace;

pub type Handle = *mut u64;
