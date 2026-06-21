/// Inline cache stubs (monomorphic + polymorphic fallback).
pub struct InlineCache;

impl Default for InlineCache {
    fn default() -> Self {
        Self::new()
    }
}

impl InlineCache {
    pub fn new() -> Self {
        InlineCache
    }
}
