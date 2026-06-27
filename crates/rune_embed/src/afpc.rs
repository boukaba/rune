//! AFPC (AOT-First Persistent Compilation) cache.
//!
//! This module persists a snapshot of a Rune execution unit so that subsequent
//! runs can skip parse + emit and, on supported platforms, skip the
//! interpreter entirely. The cache contains:
//!
//! - The `BytecodeProgram`
//! - A snapshot of the global shape table (required for cached native code)
//! - A snapshot of the inline-cache (SIDT) table
//! - Optional native code blobs for functions and hot-loop traces
//!
//! Shape IDs are content-addressed, so they are stable across process restarts
//! and cached native code remains valid after load.

use rune_bytecode::opcode::BytecodeProgram;
use rune_core::shape::{Shape, snapshot_shapes};
use rune_interpreter::ic::InlineCache;
use rune_jit_baseline::assembler::ExecutableMemory;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Magic bytes identifying an AFPC cache file.
const AFPC_MAGIC: &[u8; 4] = b"AFPC";
/// Cache format version. Bump when the serialized schema changes.
const AFPC_VERSION: u32 = 2;

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
        // Safe because CacheHeader is repr(C) with only primitive fields.
        unsafe { std::mem::transmute::<&Self, &[u8; std::mem::size_of::<Self>()]>(self) }
    }

    fn from_bytes(bytes: &[u8; std::mem::size_of::<Self>()]) -> Self {
        unsafe { std::mem::transmute::<[u8; std::mem::size_of::<Self>()], Self>(*bytes) }
    }
}

/// A serializable snapshot of a single shape.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ShapeEntry {
    /// Content-addressed shape id (stable across runs).
    pub shape_id: u64,
    /// Property key names in slot order.
    pub key_names: Vec<String>,
    /// Slot offset for each key name (usually 0..n, stored explicitly for fidelity).
    pub offsets: Vec<u64>,
    /// Dense-array sentinel shape.
    pub is_dense_array: bool,
}

impl ShapeEntry {
    /// Build a `ShapeEntry` from a runtime `Shape`.
    pub fn from_shape(shape: &Shape) -> Self {
        let mut key_names = Vec::with_capacity(shape.entries.len());
        let mut offsets = Vec::with_capacity(shape.entries.len());
        for ((_key, offset), name) in shape.entries.iter().zip(shape.key_names.iter()) {
            // Reconstruct the key name from the property key hash if possible;
            // otherwise fall back to the recorded key name.
            key_names.push(name.clone());
            offsets.push(*offset as u64);
        }
        Self {
            shape_id: shape.id,
            key_names,
            offsets,
            is_dense_array: shape.is_dense_array,
        }
    }

    /// Re-intern this shape into the global shape table.
    /// Because shape ids are content-addressed, the returned `Shape` will have
    /// the same id as when it was saved.
    pub fn restore(&self) -> &'static Shape {
        use rune_core::shape::PropertyKey;
        assert_eq!(self.key_names.len(), self.offsets.len());
        let entries: Vec<(PropertyKey, usize)> = self
            .key_names
            .iter()
            .zip(self.offsets.iter())
            .map(|(name, offset)| (PropertyKey::from_string(name), *offset as usize))
            .collect();
        Shape::intern(entries, self.key_names.clone())
    }
}

/// A native code blob for a compiled function.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CompiledFunc {
    /// Index into `BytecodeProgram.functions`.
    pub func_idx: usize,
    /// Raw executable bytes.
    pub code: Vec<u8>,
}

/// A native code blob for a compiled hot-loop trace.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct CompiledTrace {
    /// Program counter of the loop header.
    pub target_pc: usize,
    /// Raw executable bytes.
    pub code: Vec<u8>,
}

/// Full AFPC cache contents.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(
    serialize_bounds(__S: rkyv::ser::Allocator + rkyv::ser::Writer + rkyv::ser::Sharing),
    deserialize_bounds(__D: rkyv::rancor::Fallible<Error: rkyv::rancor::Source>),
    bytecheck(bounds(__C: rkyv::validation::ArchiveContext + rkyv::rancor::Fallible<Error: rkyv::rancor::Source>))
)]
pub struct AfpcCache {
    pub bytecode: BytecodeProgram,
    pub shape_table: Vec<ShapeEntry>,
    pub ic_table: Vec<InlineCache>,
    pub compiled_funcs: Vec<CompiledFunc>,
    pub compiled_traces: Vec<CompiledTrace>,
}

impl AfpcCache {
    /// Build an AFPC cache from a compiled program and a live VM state.
    /// `ics` should be taken from `vm.ics` after a warm-up execution.
    pub fn from_runtime(bytecode: BytecodeProgram, ics: Vec<InlineCache>) -> Self {
        let shape_table = snapshot_shapes()
            .into_iter()
            .map(ShapeEntry::from_shape)
            .collect();
        let compiled_funcs = aot_compile_functions(&bytecode);
        Self {
            bytecode,
            shape_table,
            ic_table: ics,
            compiled_funcs,
            compiled_traces: Vec::new(),
        }
    }

    /// Restore all shapes from the snapshot. Must be called before executing
    /// cached bytecode so that shape IDs match the compiled code and ICs.
    pub fn restore_shapes(&self) {
        for entry in &self.shape_table {
            entry.restore();
        }
    }
}

/// Serialize a full AFPC cache and write it to `path`.
pub fn save_afpc_cache<P: AsRef<Path>>(path: P, cache: &AfpcCache) -> std::io::Result<usize> {
    let mut file = fs::File::create(path)?;
    let header = CacheHeader::new();
    file.write_all(header.as_bytes())?;

    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(cache)
        .map_err(|e| std::io::Error::other(format!("rkyv serialize: {e:?}")))?;
    file.write_all(&bytes)?;

    Ok(std::mem::size_of::<CacheHeader>() + bytes.len())
}

/// Load a full AFPC cache from `path` if the cache is valid.
///
/// Returns `None` if the file does not exist, has bad magic, or an unsupported
/// version. Returns `None` on malformed rkyv data so callers can fall back to
/// source parsing.
pub fn load_afpc_cache<P: AsRef<Path>>(path: P) -> Option<AfpcCache> {
    let data = fs::read(path).ok()?;
    if data.len() < std::mem::size_of::<CacheHeader>() {
        return None;
    }

    let (header_bytes, body) = data.split_at(std::mem::size_of::<CacheHeader>());
    let header = CacheHeader::from_bytes(header_bytes.try_into().ok()?);
    if &header.magic != AFPC_MAGIC || header.version != AFPC_VERSION {
        return None;
    }

    rkyv::from_bytes::<AfpcCache, rkyv::rancor::Error>(body)
        .map_err(|e| eprintln!("AFPC cache load failed: {e:?}"))
        .ok()
}

/// Installs cached native code blobs into executable memory and returns a map
/// from function index to entry point address.
///
/// The returned `InstalledNativeCode` must be kept alive as long as the entry
/// points may be called; dropping it unmaps the memory.
pub struct InstalledNativeCode {
    _mem: ExecutableMemory,
    entries: HashMap<usize, *const u8>,
}

impl InstalledNativeCode {
    /// Install all compiled function blobs from `cache`.
    pub fn from_cache(cache: &AfpcCache) -> Self {
        let total_size: usize = cache.compiled_funcs.iter().map(|f| f.code.len()).sum();
        let mem = ExecutableMemory::allocate(total_size.max(1));
        let mut entries = HashMap::new();
        let mut offset = 0usize;
        for func in &cache.compiled_funcs {
            let len = func.code.len();
            if len == 0 {
                continue;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(func.code.as_ptr(), mem.ptr.add(offset), len);
            }
            entries.insert(func.func_idx, unsafe { mem.ptr.add(offset) as *const u8 });
            offset += len;
            // Pad to 4-byte alignment for the next blob (ARM/Thumb friendly).
            while !offset.is_multiple_of(4) && offset < mem.size {
                unsafe {
                    std::ptr::write(mem.ptr.add(offset), 0);
                }
                offset += 1;
            }
        }
        mem.make_executable();
        Self { _mem: mem, entries }
    }

    /// Take the entry map, leaving an empty map in `self`. The executable
    /// memory remains held by `self`.
    pub fn take_entries(&mut self) -> HashMap<usize, *const u8> {
        std::mem::take(&mut self.entries)
    }
}

/// AOT-compile all JIT-compatible functions in `program`.
///
/// On x86-64 this uses the existing baseline JIT. On other architectures the
/// baseline JIT is not available, so this returns an empty list; trace-based
/// AOT is planned for AArch64.
pub fn aot_compile_functions(program: &BytecodeProgram) -> Vec<CompiledFunc> {
    #[cfg(target_arch = "x86_64")]
    {
        use rune_jit_baseline::{CodeGen, is_jit_compatible};
        let mut out = Vec::new();
        for (idx, func_prog) in program.functions.iter().enumerate() {
            if is_jit_compatible(func_prog) {
                let codegen = CodeGen::new(func_prog.instructions.len());
                let compiled = codegen.compile(func_prog);
                let code = unsafe {
                    std::slice::from_raw_parts(compiled.mem.code_ptr(), compiled.mem.offset)
                        .to_vec()
                };
                // Keep the bailout table alive so table entries remain valid.
                std::mem::forget(compiled.bailout_table);
                out.push(CompiledFunc {
                    func_idx: idx,
                    code,
                });
            }
        }
        out
    }
    #[cfg(target_arch = "aarch64")]
    {
        use rune_jit_baseline::{Aarch64CodeGen, is_jit_compatible};
        let mut out = Vec::new();
        for (idx, func_prog) in program.functions.iter().enumerate() {
            if is_jit_compatible(func_prog) {
                let codegen = Aarch64CodeGen::new(func_prog.instructions.len());
                let compiled = codegen.compile(func_prog);
                let code = unsafe {
                    std::slice::from_raw_parts(compiled.mem.code_ptr(), compiled.mem.offset)
                        .to_vec()
                };
                // Keep the bailout table alive so table entries remain valid.
                std::mem::forget(compiled.bailout_table);
                out.push(CompiledFunc {
                    func_idx: idx,
                    code,
                });
            }
        }
        out
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = program;
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Backwards-compatible bytecode-only helpers (used by Sprint 16 tests and the
// simpler `--cache` path when no VM state is available).
// ---------------------------------------------------------------------------

/// Serialize only the bytecode portion of a program.
pub fn save_bytecode_cache<P: AsRef<Path>>(
    path: P,
    program: &BytecodeProgram,
) -> std::io::Result<usize> {
    let cache = AfpcCache {
        bytecode: program.clone(),
        shape_table: Vec::new(),
        ic_table: Vec::new(),
        compiled_funcs: Vec::new(),
        compiled_traces: Vec::new(),
    };
    save_afpc_cache(path, &cache)
}

/// Load just the bytecode from an AFPC cache file.
pub fn load_bytecode_cache<P: AsRef<Path>>(path: P) -> Option<BytecodeProgram> {
    load_afpc_cache(path).map(|c| c.bytecode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune_bytecode::opcode::{Instruction, Opcode};

    fn roundtrip(cache: &AfpcCache) -> AfpcCache {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("rune_afpc_test_{}.cache", id));
        let _ = fs::remove_file(&tmp);
        save_afpc_cache(&tmp, cache).unwrap();
        let loaded = load_afpc_cache(&tmp).expect("cache load failed");
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
        let cache = AfpcCache::from_runtime(program, Vec::new());
        let loaded = roundtrip(&cache);
        assert_eq!(loaded.bytecode.instructions.len(), 2);
        assert_eq!(loaded.bytecode.instructions[0].opcode, Opcode::LoadSmi);
        assert_eq!(loaded.bytecode.instructions[0].operands, vec![42]);
        assert_eq!(loaded.bytecode.string_pool, vec!["hello"]);
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
        let cache = AfpcCache::from_runtime(program, Vec::new());
        let loaded = roundtrip(&cache);
        assert_eq!(loaded.bytecode.functions.len(), 1);
        assert_eq!(loaded.bytecode.functions[0].instructions.len(), 2);
        assert_eq!(
            loaded.bytecode.functions[0].instructions[0].opcode,
            Opcode::LoadLocal
        );
    }

    #[test]
    fn test_shape_table_roundtrip() {
        use rune_core::shape::{PropertyKey, Shape};
        // Create a shape so the snapshot is non-empty.
        let _shape = Shape::intern(
            vec![(PropertyKey::from_string("x"), 0)],
            vec!["x".to_string()],
        );
        let program = BytecodeProgram::new(
            vec![Instruction::new(Opcode::Return, vec![])],
            vec![],
            vec![],
        );
        let cache = AfpcCache::from_runtime(program, Vec::new());
        assert!(!cache.shape_table.is_empty());

        let loaded = roundtrip(&cache);
        loaded.restore_shapes();

        // After restoring, interning the same shape should yield the same id.
        let restored = Shape::intern(
            vec![(PropertyKey::from_string("x"), 0)],
            vec!["x".to_string()],
        );
        assert!(loaded.shape_table.iter().any(|e| e.shape_id == restored.id));
    }

    #[test]
    fn test_ic_table_roundtrip() {
        use rune_interpreter::ic::IcEntry;
        let mut ic = InlineCache::new();
        ic.insert(
            9,
            123,
            IcEntry {
                offset: 0,
                is_own: true,
                proto_depth: 0,
            },
        );
        let program = BytecodeProgram::new(
            vec![Instruction::new(Opcode::Return, vec![])],
            vec![],
            vec![],
        );
        let cache = AfpcCache::from_runtime(program, vec![ic.clone()]);
        let loaded = roundtrip(&cache);
        assert_eq!(loaded.ic_table.len(), 1);
        assert_eq!(loaded.ic_table[0].entries.len(), 1);
        let (key, entry) = &loaded.ic_table[0].entries[0];
        assert_eq!(key.shape_id, 9);
        assert_eq!(key.key_hash, 123);
        assert_eq!(entry.offset, 0);
        assert!(entry.is_own);
    }
}
