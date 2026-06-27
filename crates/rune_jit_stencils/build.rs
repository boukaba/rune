/// build.rs — Copy-and-patch stencil compiler.
///
/// Compiles C stencil functions with Clang at build time, extracts their
/// machine-code bytes, and generates Rust constants for runtime use.
///
/// Each stencil is a naked C function in stencils/*.c. The function body
/// uses inline assembly to produce a fixed instruction sequence with
/// placeholder immediates for patchable holes.
///
/// At JIT compile time, the stencil bytes are memcpy'd into the code
/// buffer and the holes are patched with runtime values.
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let stencil_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("stencils");

    // Rerun if any C stencil file changes
    for entry in fs::read_dir(&stencil_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().is_some_and(|e| e == "c" || e == "h") {
            println!("cargo::rerun-if-changed={}", entry.path().display());
        }
    }

    let stencils = compile_stencils(&stencil_dir);
    let code = render_stencils(&stencils);
    fs::write(Path::new(&out_dir).join("stencils.rs"), code).unwrap();
}

/// A compiled stencil.
struct Stencil {
    /// Function name (from C source, without leading underscore).
    name: String,
    /// Machine-code bytes (body only — no epilogue).
    bytes: Vec<u8>,
    /// Patchable holes in the byte sequence.
    holes: Vec<Hole>,
}

/// A hole — a bit field within a 32-bit instruction word.
struct Hole {
    byte_offset: usize,
    bit_offset: u8,
    bit_width: u8,
}

// ── Mach-O parser (minimal, macOS only) ─────────────────────────────────

const MH_MAGIC_64: u32 = 0xFEEDFACF;
const LC_SEGMENT_64: u32 = 0x19;

/// Extract the __TEXT,__text section bytes from a Mach-O object file.
fn extract_text_section(o_bytes: &[u8]) -> Option<&[u8]> {
    if o_bytes.len() < 32 { return None; }
    let magic = u32::from_le_bytes(o_bytes[0..4].try_into().unwrap());
    if magic != MH_MAGIC_64 { return None; }

    let ncmds = u32::from_le_bytes(o_bytes[16..20].try_into().unwrap());
    let mut offset: usize = 32;

    for _ in 0..ncmds {
        if offset + 8 > o_bytes.len() { return None; }
        let cmd = u32::from_le_bytes(o_bytes[offset..offset+4].try_into().unwrap());
        let cmdsize = u32::from_le_bytes(o_bytes[offset+4..offset+8].try_into().unwrap()) as usize;
        if cmd == LC_SEGMENT_64 {
            let nsects = u32::from_le_bytes(o_bytes[offset+64..offset+68].try_into().unwrap()) as usize;
            let sections_start = offset + 72;
            for i in 0..nsects {
                let sec_off = sections_start + i * 80;
                if sec_off + 80 > o_bytes.len() { return None; }
                let sectname = &o_bytes[sec_off..sec_off+16];
                let segname = &o_bytes[sec_off+16..sec_off+32];
                if sectname == b"__text\0\0\0\0\0\0\0\0\0\0" &&
                   segname == b"__TEXT\0\0\0\0\0\0\0\0\0\0" {
                    let foff = u32::from_le_bytes(o_bytes[sec_off+48..sec_off+52].try_into().unwrap()) as usize;
                    let sz = u64::from_le_bytes(o_bytes[sec_off+40..sec_off+48].try_into().unwrap()) as usize;
                    if foff + sz <= o_bytes.len() {
                        return Some(&o_bytes[foff..foff+sz]);
                    }
                }
            }
        }
        offset += cmdsize;
        if offset >= o_bytes.len() { break; }
    }
    None
}

// ── Instruction analysis ────────────────────────────────────────────────

/// Decode an AArch64 instruction at the given offset.
#[inline]
fn decode_instr(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset+4].try_into().unwrap())
}

/// Expected instruction patterns (encoded as (mask, value) pairs).
/// Used to verify that Clang produced the expected code.
const RET: u32 = 0xD65F03C0;
const STR_X0_X22: u32 = 0xF90002C0;
const ADD_X22_X22_8: u32 = 0x910022D6;
const SUB_X22_X22_8: u32 = 0xD10022D6;
const LDR_X0_X22: u32 = 0xF94002C0;

/// Known stencil specifications: how to verify and extract holes from
/// each compiled function.
enum StencilSpec {
    /// STENCIL: MOVZ #? (imm16), STR x0,[x22], ADD x22,+8, RET
    LoadSmi16,
    /// STENCIL: MOVZ #?, MOVK #? (upper 16), STR, ADD, RET
    LoadSmi32,
    /// STENCIL: STR x0,[x22], ADD x22,+8, RET
    PushReg,
    /// STENCIL: SUB x22,-8, LDR x0,[x22], RET
    PopReg,
    /// STENCIL: RET
    Ret,
}

/// Compile a single C stencil file and extract its function body.
fn compile_one(stencil_dir: &Path, c_file: &Path, spec: StencilSpec) -> Stencil {
    // Derive stencil name from file stem
    let name = c_file.file_stem().unwrap().to_str().unwrap().to_string();

    // Compile with Clang
    let obj_path = stencil_dir.join(format!("{}.o", name));
    let output = Command::new("clang")
        .args([
            "-O2", "-c", "-ffreestanding",
            "-target", "arm64-apple-macos",
            "-o",
        ])
        .arg(&obj_path)
        .arg(c_file)
        .output()
        .expect("failed to execute clang");

    if !output.status.success() {
        panic!(
            "clang failed compiling {}:\nstdout: {}\nstderr: {}",
            c_file.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let obj_bytes = fs::read(&obj_path).unwrap();
    let section = extract_text_section(&obj_bytes)
        .unwrap_or_else(|| panic!("no __TEXT,__text section in {}", c_file.display()));

    // Parse according to spec
    let (bytes, holes) = parse_stencil(section, &name, spec);

    Stencil { name, bytes, holes }
}

/// Parse a compiled stencil body, verify the instructions, and extract holes.
fn parse_stencil(section: &[u8], _name: &str, spec: StencilSpec) -> (Vec<u8>, Vec<Hole>) {
    match spec {
        StencilSpec::LoadSmi16 => {
            // Expected: MOVZ x0, #? (4), STR x0,[x22] (4), ADD x22,x22,#8 (4), RET (4)
            assert!(section.len() >= 16, "load_smi_16 section too small: {} bytes", section.len());
            let movz = decode_instr(section, 0);
            let str_ = decode_instr(section, 4);
            let add_ = decode_instr(section, 8);
            let ret = decode_instr(section, 12);
            assert_eq!(str_, STR_X0_X22, "load_smi_16[1]: expected STR {:#010x}, got {:#010x}", STR_X0_X22, str_);
            assert_eq!(add_, ADD_X22_X22_8, "load_smi_16[2]: expected ADD {:#010x}, got {:#010x}", ADD_X22_X22_8, add_);
            assert_eq!(ret, RET, "load_smi_16[3]: expected RET {:#010x}, got {:#010x}", RET, ret);
            // Verify MOVZ pattern (opcode + Rd=0, imm16 varies)
            let movz_base = movz & 0xFF80001Fu32;
            assert_eq!(movz_base, 0xD2800000u32,
                "load_smi_16[0]: expected MOVZ pattern {:#010x}, got {:#010x} (full: {:#010x})",
                0xD2800000u32, movz_base, movz);
            // Stencil body = MOVZ + STR + ADD (12 bytes). RET is epilogue, stripped.
            let body = section[..12].to_vec();
            let holes = vec![
                Hole { byte_offset: 0, bit_offset: 5, bit_width: 16 }, // imm16 in MOVZ
            ];
            (body, holes)
        }
        StencilSpec::LoadSmi32 => {
            // Expected: MOVZ x0, #? (4), MOVK x0, #?, lsl #16 (4), STR (4), ADD (4), RET (4)
            assert!(section.len() >= 20, "load_smi_32 section too small: {} bytes", section.len());
            let movz = decode_instr(section, 0);
            let movk = decode_instr(section, 4);
            let str_ = decode_instr(section, 8);
            let add_ = decode_instr(section, 12);
            let ret = decode_instr(section, 16);
            assert_eq!(str_, STR_X0_X22, "load_smi_32[2]: expected STR {:#010x}, got {:#010x}", STR_X0_X22, str_);
            assert_eq!(add_, ADD_X22_X22_8, "load_smi_32[3]: expected ADD {:#010x}, got {:#010x}", ADD_X22_X22_8, add_);
            assert_eq!(ret, RET, "load_smi_32[4]: expected RET {:#010x}, got {:#010x}", RET, ret);
            // MOVZ base
            let movz_base = movz & 0xFF80001Fu32;
            assert_eq!(movz_base, 0xD2800000u32,
                "load_smi_32[0]: expected MOVZ {:#010x}, got {:#010x}",
                0xD2800000u32, movz_base);
            // MOVK base (same encoding but 0xF2800000 base)
            let movk_base = movk & 0xFF80001Fu32;
            assert_eq!(movk_base, 0xF2800000u32,
                "load_smi_32[1]: expected MOVK {:#010x}, got {:#010x}",
                0xF2800000u32, movk_base);
            // Verify hw=1 (shift 16) for MOVK
            let movk_hw = (movk >> 21) & 3;
            assert_eq!(movk_hw, 1, "load_smi_32 MOVK expected hw=1, got {movk_hw}");
            // Stencil body = MOVZ + MOVK + STR + ADD (16 bytes)
            let body = section[..16].to_vec();
            let holes = vec![
                Hole { byte_offset: 0, bit_offset: 5, bit_width: 16 }, // lower 16 bits
                Hole { byte_offset: 4, bit_offset: 5, bit_width: 16 }, // upper 16 bits
            ];
            (body, holes)
        }
        StencilSpec::PushReg => {
            assert!(section.len() >= 12, "push_reg section too small: {} bytes", section.len());
            let str_ = decode_instr(section, 0);
            let add_ = decode_instr(section, 4);
            let ret = decode_instr(section, 8);
            assert_eq!(str_, STR_X0_X22, "push_reg[0]: expected STR {:#010x}, got {:#010x}", STR_X0_X22, str_);
            assert_eq!(add_, ADD_X22_X22_8, "push_reg[1]: expected ADD {:#010x}, got {:#010x}", ADD_X22_X22_8, add_);
            assert_eq!(ret, RET, "push_reg[2]: expected RET {:#010x}, got {:#010x}", RET, ret);
            let body = section[..8].to_vec();
            (body, vec![])
        }
        StencilSpec::PopReg => {
            assert!(section.len() >= 12, "pop_reg section too small: {} bytes", section.len());
            let sub_ = decode_instr(section, 0);
            let ldr_ = decode_instr(section, 4);
            let ret = decode_instr(section, 8);
            assert_eq!(sub_, SUB_X22_X22_8, "pop_reg[0]: expected SUB {:#010x}, got {:#010x}", SUB_X22_X22_8, sub_);
            assert_eq!(ldr_, LDR_X0_X22, "pop_reg[1]: expected LDR {:#010x}, got {:#010x}", LDR_X0_X22, ldr_);
            assert_eq!(ret, RET, "pop_reg[2]: expected RET {:#010x}, got {:#010x}", RET, ret);
            let body = section[..8].to_vec();
            (body, vec![])
        }
        StencilSpec::Ret => {
            assert!(section.len() >= 4, "ret section too small: {} bytes", section.len());
            let ret = decode_instr(section, 0);
            assert_eq!(ret, RET, "ret[0]: expected RET {:#010x}, got {:#010x}", RET, ret);
            let body = section[..4].to_vec();
            (body, vec![])
        }
    }
}

/// Compile all stencils.
fn compile_stencils(stencil_dir: &Path) -> Vec<Stencil> {
    let mut stencils = Vec::new();

    let specs: Vec<(&str, StencilSpec)> = vec![
        ("load_smi_16", StencilSpec::LoadSmi16),
        ("load_smi_32", StencilSpec::LoadSmi32),
        ("push_reg",    StencilSpec::PushReg),
        ("pop_reg",     StencilSpec::PopReg),
        ("ret",         StencilSpec::Ret),
    ];

    for (file_stem, spec) in specs {
        let c_file = stencil_dir.join(format!("{file_stem}.c"));
        let stencil = compile_one(stencil_dir, &c_file, spec);
        stencils.push(stencil);
    }

    stencils
}

// ── Code generation ─────────────────────────────────────────────────────

fn render_stencils(stencils: &[Stencil]) -> String {
    let mut out = String::new();
    out.push_str("// Auto-generated by build.rs — do not edit.\n");
    out.push_str("#[allow(dead_code)]\n");
    out.push_str("pub struct StencilDef {\n");
    out.push_str("    pub name: &'static str,\n");
    out.push_str("    pub bytes: &'static [u8],\n");
    out.push_str("    pub holes: &'static [HoleDef],\n");
    out.push_str("}\n\n");
    out.push_str("#[allow(dead_code)]\n");
    out.push_str("#[derive(Clone, Copy)]\n");
    out.push_str("pub struct HoleDef {\n");
    out.push_str("    pub byte_offset: usize,\n");
    out.push_str("    pub bit_offset: u8,\n");
    out.push_str("    pub bit_width: u8,\n");
    out.push_str("}\n\n");

    for s in stencils {
        out.push_str(&format!(
            "pub const {}_BYTES: &[u8] = &[{}];\n",
            s.name.to_uppercase(),
            s.bytes.iter().map(|b| format!("{b:#04x}")).collect::<Vec<_>>().join(", ")
        ));
        if s.holes.is_empty() {
            out.push_str(&format!(
                "pub const {}_HOLES: &[HoleDef] = &[];\n",
                s.name.to_uppercase()
            ));
        } else {
            out.push_str(&format!(
                "pub const {}_HOLES: &[HoleDef] = &[{}];\n",
                s.name.to_uppercase(),
                s.holes.iter().map(|h| {
                    format!("HoleDef {{ byte_offset: {}, bit_offset: {}, bit_width: {} }}",
                        h.byte_offset, h.bit_offset, h.bit_width)
                }).collect::<Vec<_>>().join(", ")
            ));
        }
    }

    out.push_str("\npub static ALL_STENCILS: &[StencilDef] = &[\n");
    for s in stencils {
        out.push_str(&format!(
            "    StencilDef {{ name: \"{}\", bytes: {}_BYTES, holes: {}_HOLES }},\n",
            s.name, s.name.to_uppercase(), s.name.to_uppercase()
        ));
    }
    out.push_str("];\n");

    out
}
