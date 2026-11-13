//! Self-contained application bundles for `rune build` / `rune run`.
//!
//! `RUNEAPP2` layout:
//!   magic(8) | u32 flags | u32 source_len | source
//!   [flags & 2] u32 module_count, then per module: u32 spec_len | spec |
//!               u32 src_len | source
//!   [flags & 1] AFPC cache bytes (to end of file)
//!
//! Script bundles carry compiled bytecode/native code and execute without
//! parsing source. Module bundles carry the entry source plus a specifier →
//! source map for the import graph and recompile at run time.

use crate::afpc::{self, AfpcCache};
use std::io;
use std::path::Path;

const MAGIC: &[u8; 8] = b"RUNEAPP2";
const FLAG_CACHE: u32 = 1;
const FLAG_MODULES: u32 = 2;

pub struct Bundle {
    pub source: String,
    pub cache: Option<AfpcCache>,
    pub modules: Vec<(String, String)>,
}

fn write_section(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn read_section<'a>(data: &'a [u8], cursor: &mut usize) -> Option<&'a [u8]> {
    if *cursor + 4 > data.len() {
        return None;
    }
    let len = u32::from_le_bytes(data[*cursor..*cursor + 4].try_into().ok()?) as usize;
    *cursor += 4;
    let end = cursor.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    let section = &data[*cursor..end];
    *cursor = end;
    Some(section)
}

fn write_bundle(
    path: &Path,
    source: &str,
    cache: Option<&AfpcCache>,
    modules: &[(String, String)],
) -> io::Result<usize> {
    let cache_bytes = match cache {
        Some(cache) => Some(
            afpc::serialize_afpc_cache(cache)
                .ok_or_else(|| io::Error::other("AFPC serialization failed"))?,
        ),
        None => None,
    };
    let mut flags = 0u32;
    if cache_bytes.is_some() {
        flags |= FLAG_CACHE;
    }
    if !modules.is_empty() {
        flags |= FLAG_MODULES;
    }
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&flags.to_le_bytes());
    write_section(&mut out, source.as_bytes());
    if flags & FLAG_MODULES != 0 {
        out.extend_from_slice(&(modules.len() as u32).to_le_bytes());
        for (spec, src) in modules {
            write_section(&mut out, spec.as_bytes());
            write_section(&mut out, src.as_bytes());
        }
    }
    if let Some(bytes) = cache_bytes {
        out.extend_from_slice(&bytes);
    }
    std::fs::write(path, &out)?;
    Ok(out.len())
}

/// Write a compiled script bundle (source + AFPC cache).
pub fn save_bundle<P: AsRef<Path>>(path: P, source: &str, cache: &AfpcCache) -> io::Result<usize> {
    write_bundle(path.as_ref(), source, Some(cache), &[])
}

/// Write a source module bundle (entry + specifier→source map).
pub fn save_module_bundle<P: AsRef<Path>>(
    path: P,
    source: &str,
    modules: &[(String, String)],
) -> io::Result<usize> {
    write_bundle(path.as_ref(), source, None, modules)
}

pub fn load_bundle<P: AsRef<Path>>(path: P) -> Option<Bundle> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 16 || &data[..8] != MAGIC {
        return None;
    }
    let flags = u32::from_le_bytes(data[8..12].try_into().ok()?);
    let mut cursor = 12usize;
    let source = String::from_utf8(read_section(&data, &mut cursor)?.to_vec()).ok()?;
    let mut modules = Vec::new();
    if flags & FLAG_MODULES != 0 {
        if cursor + 4 > data.len() {
            return None;
        }
        let count = u32::from_le_bytes(data[cursor..cursor + 4].try_into().ok()?) as usize;
        cursor += 4;
        for _ in 0..count {
            let spec = String::from_utf8(read_section(&data, &mut cursor)?.to_vec()).ok()?;
            let src = String::from_utf8(read_section(&data, &mut cursor)?.to_vec()).ok()?;
            modules.push((spec, src));
        }
    }
    let cache = if flags & FLAG_CACHE != 0 {
        let cache = afpc::deserialize_afpc_cache(&data[cursor..])?;
        if cache.source_hash != afpc::source_hash(&source) {
            return None;
        }
        Some(cache)
    } else {
        None
    };
    Some(Bundle {
        source,
        cache,
        modules,
    })
}
