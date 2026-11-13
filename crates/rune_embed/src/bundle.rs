//! Self-contained application bundles for `rune build` / `rune run`.
//!
//! Layout: `RUNEAPP1` magic + u32 source length + UTF-8 source + AFPC cache
//! bytes. The cache already carries a source hash and engine/ABI tag, so a
//! stale or foreign bundle is rejected without parsing the source.

use crate::afpc::{self, AfpcCache};
use std::io;
use std::path::Path;

const MAGIC: &[u8; 8] = b"RUNEAPP1";

pub struct Bundle {
    pub source: String,
    pub cache: AfpcCache,
}

pub fn save_bundle<P: AsRef<Path>>(path: P, source: &str, cache: &AfpcCache) -> io::Result<usize> {
    let cache_bytes = afpc::serialize_afpc_cache(cache)
        .ok_or_else(|| io::Error::other("AFPC serialization failed"))?;
    let src = source.as_bytes();
    let mut out = Vec::with_capacity(12 + src.len() + cache_bytes.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(src.len() as u32).to_le_bytes());
    out.extend_from_slice(src);
    out.extend_from_slice(&cache_bytes);
    std::fs::write(path, &out)?;
    Ok(out.len())
}

pub fn load_bundle<P: AsRef<Path>>(path: P) -> Option<Bundle> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 12 || &data[..8] != MAGIC {
        return None;
    }
    let source_len = u32::from_le_bytes(data[8..12].try_into().ok()?) as usize;
    let source_end = 12usize.checked_add(source_len)?;
    if source_end > data.len() {
        return None;
    }
    let source = String::from_utf8(data[12..source_end].to_vec()).ok()?;
    let cache = afpc::deserialize_afpc_cache(&data[source_end..])?;
    if cache.source_hash != afpc::source_hash(&source) {
        return None;
    }
    Some(Bundle { source, cache })
}
