//! AFPC (AOT-First Persistent Compilation) bytecode cache.
//!
//! This module serializes a `BytecodeProgram` to disk with rkyv so that
//! subsequent runs can skip parse + emit. It is the first step toward the
//! full AFPC vision: eventually the cache will also hold shape tables,
//! compiled native code, and IC entries.

use rune_bytecode::opcode::BytecodeProgram;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Magic bytes identifying an AFPC bytecode cache file.
const AFPC_MAGIC: &[u8; 4] = b"AFPC";
/// Cache format version. Bump when the serialized schema changes.
const AFPC_VERSION: u32 = 1;

/// Header written at the start of every cache file.
#[derive(Copy, Clone, Debug)]
#[repr(C)]
struct CacheHeader {
    magic: [u8; 4],
    version: u32,
    /// Reserved for future use (e.g. flags, checksum offset).
    _reserved: u64,
}

impl CacheHeader {
    fn new() -> Self {
        Self {
            magic: *AFPC_MAGIC,
            version: AFPC_VERSION,
            _reserved: 0,
        }
    }

    fn as_bytes(&self) -> &[u8; std::mem::size_of::<Self>()] {
        // Safe because CacheHeader is repr(C) with no padding issues for these fields.
        unsafe { std::mem::transmute::<&Self, &[u8; std::mem::size_of::<Self>()]>(self) }
    }

    fn from_bytes(bytes: &[u8; std::mem::size_of::<Self>()]) -> Self {
        unsafe { std::mem::transmute::<[u8; std::mem::size_of::<Self>()], Self>(*bytes) }
    }
}

/// Serialize `program` and write it to `path`.
///
/// Returns the number of bytes written, or an IO error.
pub fn save_bytecode_cache<P: AsRef<Path>>(
    path: P,
    program: &BytecodeProgram,
) -> std::io::Result<usize> {
    let mut file = fs::File::create(path)?;
    let header = CacheHeader::new();
    file.write_all(header.as_bytes())?;

    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(program)
        .map_err(|e| std::io::Error::other(format!("rkyv serialize: {e:?}")))?;
    file.write_all(&bytes)?;

    Ok(std::mem::size_of::<CacheHeader>() + bytes.len())
}

/// Load a `BytecodeProgram` from `path` if the cache is valid.
///
/// Returns `None` if the file does not exist, has bad magic, or an unsupported
/// version. Returns `None` on malformed rkyv data so callers can fall back to
/// source parsing.
pub fn load_bytecode_cache<P: AsRef<Path>>(path: P) -> Option<BytecodeProgram> {
    let data = fs::read(path).ok()?;
    if data.len() < std::mem::size_of::<CacheHeader>() {
        return None;
    }

    let (header_bytes, body) = data.split_at(std::mem::size_of::<CacheHeader>());
    let header = CacheHeader::from_bytes(header_bytes.try_into().ok()?);
    if &header.magic != AFPC_MAGIC || header.version != AFPC_VERSION {
        return None;
    }

    // `from_bytes` validates the rkyv buffer and then deserializes back to a
    // native `BytecodeProgram`.
    rkyv::from_bytes::<BytecodeProgram, rkyv::rancor::Error>(body)
        .map_err(|e| eprintln!("AFPC cache load failed: {e:?}"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_bytecode::opcode::{Instruction, Opcode};

    fn roundtrip(program: &BytecodeProgram) -> BytecodeProgram {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("rune_afpc_test_{}.cache", id));
        let _ = fs::remove_file(&tmp);
        save_bytecode_cache(&tmp, program).unwrap();
        let loaded = load_bytecode_cache(&tmp).expect("cache load failed");
        let _ = fs::remove_file(&tmp);
        loaded
    }

    #[test]
    fn test_cache_header_roundtrip() {
        let h = CacheHeader::new();
        let bytes = h.as_bytes();
        let h2 = CacheHeader::from_bytes(bytes);
        assert_eq!(h2.magic, *AFPC_MAGIC);
        assert_eq!(h2.version, AFPC_VERSION);
    }

    #[test]
    fn test_bytecode_roundtrip_simple() {
        let program = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![42]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec!["hello".to_string()],
            vec![],
        );
        let loaded = roundtrip(&program);
        assert_eq!(loaded.instructions.len(), 2);
        assert_eq!(loaded.instructions[0].opcode, Opcode::LoadSmi);
        assert_eq!(loaded.instructions[0].operands, vec![42]);
        assert_eq!(loaded.string_pool, vec!["hello"]);
    }

    #[test]
    fn test_bytecode_roundtrip_nested_function() {
        let inner = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadLocal, vec![0]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![],
        );
        let program = BytecodeProgram::new(
            vec![
                Instruction::new(Opcode::LoadSmi, vec![1]),
                Instruction::new(Opcode::MakeFunction, vec![0]),
                Instruction::new(Opcode::Return, vec![]),
            ],
            vec![],
            vec![inner],
        );
        let loaded = roundtrip(&program);
        assert_eq!(loaded.functions.len(), 1);
        assert_eq!(loaded.functions[0].instructions.len(), 2);
        assert_eq!(loaded.functions[0].instructions[0].opcode, Opcode::LoadLocal);
    }
}
