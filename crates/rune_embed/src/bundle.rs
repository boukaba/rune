//! Self-contained application bundles for `rune build` / `rune run`.
//!
//! `RUNEAPP2` layout:
//!   magic(8) | u32 flags | u32 source_len | source
//!   [flags & 2] u32 module_count, then per module: u32 spec_len | spec |
//!               u32 src_len | source
//!   [flags & 1] AFPC cache bytes (to end of file)
//!
//! Script bundles carry compiled bytecode/native code and execute without
//! parsing source. Module bundles carry the entry source plus a precompiled
//! graph (`FLAG_MODULE_CACHE`). `save_standalone` appends a bundle and a
//! length/magic trailer to a copy of the interpreter, producing an executable
//! that runs the bundle when invoked with no arguments.

use crate::afpc::{self, AfpcCache};
use rune_bytecode::opcode::BytecodeProgram;
use std::io;
use std::path::Path;

const MAGIC: &[u8; 8] = b"RUNEAPP2";
const FLAG_CACHE: u32 = 1;
const FLAG_MODULES: u32 = 2;
const FLAG_MODULE_CACHE: u32 = 4;
const EXE_TRAILER: &[u8; 8] = b"RUNEXE01";
const EXE_TRAILER_LEN: usize = 16;

/// Precompiled ESM graph: program order matches `specifiers`.
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ModuleProgramCache {
    pub programs: Vec<BytecodeProgram>,
    pub specifiers: Vec<String>,
}

pub struct Bundle {
    pub source: String,
    pub cache: Option<AfpcCache>,
    pub modules: Vec<(String, String)>,
    pub module_cache: Option<ModuleProgramCache>,
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

fn encode_bundle(
    source: &str,
    cache: Option<&AfpcCache>,
    modules: &[(String, String)],
    module_cache: Option<&ModuleProgramCache>,
) -> io::Result<Vec<u8>> {
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
    let module_cache_bytes = match module_cache {
        Some(cache) => Some(
            rkyv::to_bytes::<rkyv::rancor::Error>(cache)
                .map_err(|e| io::Error::other(format!("module cache serialize: {e:?}")))?
                .to_vec(),
        ),
        None => None,
    };
    if module_cache_bytes.is_some() {
        flags |= FLAG_MODULE_CACHE;
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
    if let Some(bytes) = module_cache_bytes {
        out.extend_from_slice(&bytes);
    }
    Ok(out)
}

fn write_bundle(
    path: &Path,
    source: &str,
    cache: Option<&AfpcCache>,
    modules: &[(String, String)],
    module_cache: Option<&ModuleProgramCache>,
) -> io::Result<usize> {
    let out = encode_bundle(source, cache, modules, module_cache)?;
    std::fs::write(path, &out)?;
    Ok(out.len())
}

/// Encode a compiled script bundle without writing it.
pub fn script_bundle_bytes(source: &str, cache: &AfpcCache) -> io::Result<Vec<u8>> {
    encode_bundle(source, Some(cache), &[], None)
}

/// Encode an AOT module bundle without writing it.
pub fn module_bytecode_bundle_bytes(
    source: &str,
    cache: &ModuleProgramCache,
) -> io::Result<Vec<u8>> {
    encode_bundle(source, None, &[], Some(cache))
}

/// Write a compiled script bundle (source + AFPC cache).
pub fn save_bundle<P: AsRef<Path>>(path: P, source: &str, cache: &AfpcCache) -> io::Result<usize> {
    write_bundle(path.as_ref(), source, Some(cache), &[], None)
}

/// Write a source module bundle (entry + specifier→source map).
pub fn save_module_bundle<P: AsRef<Path>>(
    path: P,
    source: &str,
    modules: &[(String, String)],
) -> io::Result<usize> {
    write_bundle(path.as_ref(), source, None, modules, None)
}

/// Write an AOT module bundle: entry source plus precompiled graph.
pub fn save_module_bytecode_bundle<P: AsRef<Path>>(
    path: P,
    source: &str,
    cache: &ModuleProgramCache,
) -> io::Result<usize> {
    write_bundle(path.as_ref(), source, None, &[], Some(cache))
}

/// Append a bundle payload and trailer to an interpreter binary image.
pub fn executable_with_bundle(exe: &[u8], bundle: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(exe.len() + bundle.len() + EXE_TRAILER_LEN);
    out.extend_from_slice(exe);
    out.extend_from_slice(bundle);
    out.extend_from_slice(&(bundle.len() as u64).to_le_bytes());
    out.extend_from_slice(EXE_TRAILER);
    out
}

/// Locate an appended bundle in an executable image.
pub fn find_embedded_bundle(data: &[u8]) -> Option<&[u8]> {
    if data.len() < EXE_TRAILER_LEN {
        return None;
    }
    let trailer = &data[data.len() - EXE_TRAILER_LEN..];
    if &trailer[8..] != EXE_TRAILER {
        return None;
    }
    let len = u64::from_le_bytes(trailer[..8].try_into().ok()?) as usize;
    let end = data.len() - EXE_TRAILER_LEN;
    if len > end {
        return None;
    }
    Some(&data[end - len..end])
}

/// Read and decode a bundle appended to the current executable.
pub fn load_embedded_bundle() -> Option<Bundle> {
    let exe = std::env::current_exe().ok()?;
    let data = std::fs::read(exe).ok()?;
    decode_bundle(find_embedded_bundle(&data)?)
}

/// Build a standalone executable by copying `exe` and appending a bundle.
pub fn save_standalone<P: AsRef<Path>>(path: P, exe: &[u8], bundle: &[u8]) -> io::Result<usize> {
    let out = executable_with_bundle(exe, bundle);
    std::fs::write(path.as_ref(), &out)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path.as_ref())?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path.as_ref(), perms)?;
    }
    Ok(out.len())
}

pub fn load_bundle<P: AsRef<Path>>(path: P) -> Option<Bundle> {
    decode_bundle(&std::fs::read(path).ok()?)
}

/// Decode a bundle from an in-memory image.
pub fn decode_bundle(data: &[u8]) -> Option<Bundle> {
    if data.len() < 16 || &data[..8] != MAGIC {
        return None;
    }
    let flags = u32::from_le_bytes(data[8..12].try_into().ok()?);
    let mut cursor = 12usize;
    let source = String::from_utf8(read_section(data, &mut cursor)?.to_vec()).ok()?;
    let mut modules = Vec::new();
    if flags & FLAG_MODULES != 0 {
        if cursor + 4 > data.len() {
            return None;
        }
        let count = u32::from_le_bytes(data[cursor..cursor + 4].try_into().ok()?) as usize;
        cursor += 4;
        for _ in 0..count {
            let spec = String::from_utf8(read_section(data, &mut cursor)?.to_vec()).ok()?;
            let src = String::from_utf8(read_section(data, &mut cursor)?.to_vec()).ok()?;
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
    let module_cache = if flags & FLAG_MODULE_CACHE != 0 {
        let body = &data[cursor..];
        let mut aligned = vec![0u8; body.len() + 16];
        let offset = (16 - (aligned.as_ptr() as usize & 15)) & 15;
        aligned[offset..offset + body.len()].copy_from_slice(body);
        Some(
            rkyv::from_bytes::<ModuleProgramCache, rkyv::rancor::Error>(
                &aligned[offset..offset + body.len()],
            )
            .ok()?,
        )
    } else {
        None
    };
    Some(Bundle {
        source,
        cache,
        modules,
        module_cache,
    })
}
