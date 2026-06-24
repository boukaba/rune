/// Entry in an inline cache for a specific (shape, key) pair.
#[derive(Copy, Clone, Debug, Default)]
pub struct IcEntry {
    /// Slot offset in the object (or prototype at proto_depth).
    pub offset: usize,
    /// True if the property is on the object itself, false if inherited.
    pub is_own: bool,
    /// How many prototype hops to reach the property (0 = own).
    pub proto_depth: u8,
}

/// Cache key stored alongside IcEntry for linear-scan matching.
#[derive(Copy, Clone, Debug, Default)]
pub struct IcKey {
    pub shape_id: u64,
    pub key_hash: u64,
}

/// Shape-Indexed Dispatch Table — per-callsite inline cache.
///
/// Uses a flat Vec of (key, entry) pairs, linear-scanned by shape_id
/// and key_hash. With ≤8 entries (99% of real-world callsites), linear
/// scan is faster than HashMap hashing. The flat layout is SIMD-ready:
/// shape_ids can be loaded into a vector register and compared in parallel.
#[derive(Clone, Debug)]
pub struct InlineCache {
    pub entries: Vec<(IcKey, IcEntry)>,
}

impl Default for InlineCache {
    fn default() -> Self {
        Self::new()
    }
}

impl InlineCache {
    pub fn new() -> Self {
        InlineCache {
            entries: Vec::new(),
        }
    }

    /// Look up a cached entry by (shape_id, key_hash). Linear scan.
    pub fn get(&self, shape_id: u64, key_hash: u64) -> Option<IcEntry> {
        self.entries
            .iter()
            .find(|(k, _)| k.shape_id == shape_id && k.key_hash == key_hash)
            .map(|(_, e)| *e)
    }

    /// Insert or update a cached entry. Caps at 8 entries (evicts oldest if full).
    pub fn insert(&mut self, shape_id: u64, key_hash: u64, entry: IcEntry) {
        let key = IcKey { shape_id, key_hash };
        // Update existing entry for same key
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|(k, _)| k.shape_id == shape_id && k.key_hash == key_hash)
        {
            existing.1 = entry;
            return;
        }
        // Insert new entry; cap at 8 (evict LRU)
        if self.entries.len() >= 8 {
            self.entries.remove(0);
        }
        self.entries.push((key, entry));
    }
}

/// Aggregate IC statistics across all callsites.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct IcStats {
    pub lookups: u64,
    pub hits: u64,
    pub misses: u64,
}
