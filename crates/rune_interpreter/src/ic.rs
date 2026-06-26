/// Entry in an inline cache for a specific (shape, key) pair.
#[derive(Copy, Clone, Debug, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
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

    /// Look up a cached entry by (shape_id, key_hash).
    /// Uses SIMD: SSE4.1 on x86-64, NEON on aarch64, scalar fallback elsewhere.
    #[inline]
    #[allow(clippy::needless_return)]
    pub fn get(&self, shape_id: u64, key_hash: u64) -> Option<IcEntry> {
        #[cfg(target_arch = "x86_64")]
        {
            return self.get_simd(shape_id, key_hash);
        }
        #[cfg(target_arch = "aarch64")]
        {
            return self.get_neon(shape_id, key_hash);
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            self.get_scalar(shape_id, key_hash)
        }
    }

    /// Scalar linear scan fallback (used as universal fallback on x86-64 without SSE4.1).
    fn get_scalar(&self, shape_id: u64, key_hash: u64) -> Option<IcEntry> {
        self.entries
            .iter()
            .find(|(k, _)| k.shape_id == shape_id && k.key_hash == key_hash)
            .map(|(_, e)| *e)
    }

    /// ARM NEON SIMD shape compare: 2 shape_ids compared in 1 instruction.
    /// Uses `vceqq_u64` + `vgetq_lane_u64`. IcKey layout is 16 bytes = uint64x2_t.
    /// Each entry is (IcKey, IcEntry) = 32 bytes = 2 × uint64x2_t, so consecutive
    /// IcKeys are at stride 2 (ptr.add(2) skips the IcEntry of entry i).
    #[cfg(target_arch = "aarch64")]
    fn get_neon(&self, shape_id: u64, key_hash: u64) -> Option<IcEntry> {
        use std::arch::aarch64::*;
        unsafe {
            let entries = &self.entries;
            let mut i = 0;
            while i + 1 < entries.len() {
                let base = entries.as_ptr().add(i) as *const uint64x2_t;
                let key0: uint64x2_t = *base;        // entry i          → IcKey of entry i
                let key1: uint64x2_t = *base.add(2);  // entry i + 32 bytes → IcKey of entry i+1
                let target = vdupq_n_u64(shape_id);
                let cmp0 = vceqq_u64(key0, target);
                let cmp1 = vceqq_u64(key1, target);
                if vgetq_lane_u64(cmp0, 0) == u64::MAX {
                    let e = entries[i];
                    if e.0.key_hash == key_hash {
                        return Some(e.1);
                    }
                }
                if vgetq_lane_u64(cmp1, 0) == u64::MAX {
                    let e = entries[i + 1];
                    if e.0.key_hash == key_hash {
                        return Some(e.1);
                    }
                }
                i += 2;
            }
            if i < entries.len() {
                let e = entries[i];
                if e.0.shape_id == shape_id && e.0.key_hash == key_hash {
                    return Some(e.1);
                }
            }
            None
        }
    }

    /// SIMD shape compare: on x86-64 with SSE4.1, compares 2 shape_ids in 1 instruction.
    /// Falls back to scalar linear scan on other platforms or if SSE4.1 unavailable.
    /// Each entry is (IcKey, IcEntry) = 32 bytes = 2 × __m128i, so consecutive
    /// IcKeys are at stride 2 (ptr.add(2) skips the IcEntry of entry i).
    #[cfg(target_arch = "x86_64")]
    fn get_simd(&self, shape_id: u64, key_hash: u64) -> Option<IcEntry> {
        if is_x86_feature_detected!("sse4.1") {
            use core::arch::x86_64::*;
            unsafe {
                let entries = &self.entries;
                let mut i = 0;
                while i + 1 < entries.len() {
                    let base = entries.as_ptr().add(i) as *const __m128i;
                    let key0 = _mm_loadu_si128(base);        // entry i → IcKey of entry i
                    let key1 = _mm_loadu_si128(base.add(2));  // entry i + 32 bytes → IcKey of entry i+1
                    let target = _mm_set1_epi64x(shape_id as i64);
                    let cmp0 = _mm_cmpeq_epi64(key0, target);
                    let cmp1 = _mm_cmpeq_epi64(key1, target);
                    if _mm_extract_epi64(cmp0, 0) as u64 == u64::MAX {
                        let e = entries[i];
                        if e.0.key_hash == key_hash {
                            return Some(e.1);
                        }
                    }
                    if _mm_extract_epi64(cmp1, 0) as u64 == u64::MAX {
                        let e = entries[i + 1];
                        if e.0.key_hash == key_hash {
                            return Some(e.1);
                        }
                    }
                    i += 2;
                }
                if i < entries.len() {
                    let e = entries[i];
                    if e.0.shape_id == shape_id && e.0.key_hash == key_hash {
                        return Some(e.1);
                    }
                }
                None
            }
        } else {
            self.get_scalar(shape_id, key_hash)
        }
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
        // No cap — SIDT means no megamorphic cliff.
        // With SIMD, each iteration compares 2 entries; 50 shapes = 25 SIMD ops.
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

/// A recorded opcode from a hot-loop trace.
#[derive(Clone, Debug)]
pub struct TraceOp {
    /// The opcode executed.
    pub opcode: u8,
    /// The operands of the instruction.
    pub operands: Vec<i64>,
    /// PC in the original program (used for bailout PC translation).
    pub original_pc: usize,
    /// Shape ID hit during LoadProperty (0 if not a property access or miss).
    pub shape_id: u64,
    /// Number of times this opcode would dispatch in the interpreter.
    pub cost: u32,
}

/// A recorded trace of one hot-loop iteration.
#[derive(Clone, Debug, Default)]
pub struct LoopTrace {
    pub target_pc: usize,
    pub ops: Vec<TraceOp>,
    /// Total iteration count when trace was recorded.
    pub total_iterations: u64,
    /// Unique shape_ids seen (for monomorphism check).
    pub shape_ids: Vec<u64>,
    /// Compiled native code for this trace (null if not yet compiled).
    pub compiled_entry: *const u8,
    /// The bytecode index after the loop (JumpIfFalse fallthrough target).
    /// Set when the trace is compiled; used to resume the interpreter after
    /// the native trace exits.
    pub exit_pc: usize,
    /// Leaked BytecodeProgram pointer that compiled_entry references.
    /// Kept alive for the lifetime of the trace. Dropped when the Vm is dropped.
    pub compiled_prog: *mut u8,
    /// Maps trace instruction index → original program PC.
    /// Used to translate bailout PCs when the trace bails.
    pub trace_to_original_pc: Vec<usize>,
    /// Bailout table produced by trace compilation (metadata only).
    /// Mirrors the function JIT's bailout_tables pattern; not queried at
    /// runtime (bailout works through jit_bailout.pending).
    pub bailout_table: Option<Box<rune_jit_baseline::BailoutTable>>,
}

impl LoopTrace {
    pub fn is_monomorphic(&self) -> bool {
        self.shape_ids.len() <= 1
    }

    pub fn estimated_interpreter_cost(&self) -> u32 {
        // Each opcode: ~10 cycles for dispatch + execution
        self.ops.len() as u32 * 10
    }

    pub fn estimated_native_cost(&self) -> u32 {
        // Native: ~1-2 cycles per opcode (register-based, no dispatch overhead)
        self.ops.len() as u32 * 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for P16: SIMD IC stride bug.
    /// Both NEON (aarch64) and SSE4.1 (x86_64) hot paths used `ptr.add(1)`
    /// instead of `ptr.add(2)` to advance between IcEntry's (32 bytes each).
    /// This caused every odd-indexed entry to read 16 bytes of garbage,
    /// making shape_id comparisons fail for odd indices.
    #[test]
    fn test_ic_simd_odd_entries() {
        let mut ic = InlineCache::new();
        // Insert 10 entries — tests the SIMD 2-at-a-time stride path
        for i in 0..10u64 {
            let entry = IcEntry { offset: i as usize, is_own: true, proto_depth: 0 };
            ic.insert(i + 100, i * 31, entry);
        }
        // Every entry must be findable — including odd indices
        for i in 0..10u64 {
            let e = ic
                .get(i + 100, i * 31)
                .unwrap_or_else(|| panic!("entry {} missing", i));
            assert_eq!(e.offset, i as usize, "entry {} offset mismatch", i);
        }
    }

    /// IC correctly returns None for non-existent entries
    #[test]
    fn test_ic_miss() {
        let mut ic = InlineCache::new();
        ic.insert(42, 99, IcEntry { offset: 0, is_own: true, proto_depth: 0 });
        assert!(ic.get(42, 100).is_none(), "wrong key_hash should miss");
        assert!(ic.get(43, 99).is_none(), "wrong shape_id should miss");
        assert!(ic.get(0, 0).is_none(), "empty entry should miss");
    }
}
