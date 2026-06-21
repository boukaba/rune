use std::collections::HashMap;

/// Entry in an inline cache for a specific shape.
#[derive(Copy, Clone, Debug)]
pub struct IcEntry {
    /// Slot offset in the object (or prototype at proto_depth).
    pub offset: usize,
    /// True if the property is on the object itself, false if inherited.
    pub is_own: bool,
    /// How many prototype hops to reach the property (0 = own).
    pub proto_depth: u8,
}

/// Shape-Indexed Dispatch Table — per-callsite inline cache.
///
/// Maps shape.id → IcEntry for O(1) property access.
/// Unlike V8's 4-state IC, this has NO megamorphic fallback.
/// The table grows unboundedly — 10 shapes, 100 shapes, all O(1).
#[derive(Clone, Debug)]
pub struct InlineCache {
    pub entries: HashMap<u64, IcEntry>,
}

impl InlineCache {
    pub fn new() -> Self {
        InlineCache { entries: HashMap::new() }
    }
}

/// Aggregate IC statistics across all callsites.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct IcStats {
    pub lookups: u64,
    pub hits: u64,
    pub misses: u64,
}
