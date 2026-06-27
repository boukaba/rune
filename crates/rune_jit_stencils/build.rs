//! build.rs — Copy-and-patch stencil compiler.
//!
//! Compiles C stencil functions with Clang at build time, extracts their
//! machine-code bytes, and generates Rust constants for runtime use.
//!
//! Two kinds of stencils:
//!
//! 1. **Naked-asm stencils** — `__attribute__((naked))` functions with inline
//!    assembly. These have NO link-time holes (no helper calls) and are used
//!    for simple operations like push/pop/ret.
//!
//! 2. **Real C stencils** — regular C functions that call runtime helpers
//!    (e.g., `rune_push(0xDEAD)`). Clang generates MOV + BL/B with link-time
//!    relocations. These have value holes (immediates) AND link holes (B/BL
//!    offsets that need resolution relative to the emitted helper).

use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let stencil_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("stencils");

    for entry in fs::read_dir(&stencil_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.path().extension().is_some_and(|e| e == "c" || e == "h") {
            println!("cargo::rerun-if-changed={}", entry.path().display());
        }
    }

    let mut emitter = Emitter::new();

    // ── Runtime helpers (compiled first, emitted as byte refs) ──────
    let rune_push = compile_helper(&stencil_dir, "rune_push");
    emitter.add_helper(&rune_push);
    let rune_load_local = compile_helper(&stencil_dir, "rune_load_local");
    emitter.add_helper(&rune_load_local);
    let rune_store_local = compile_helper(&stencil_dir, "rune_store_local");
    emitter.add_helper(&rune_store_local);

    // ── Naked-asm stencils (no link holes) ──────────────────────────
    emit_naked_stencil(&mut emitter, &stencil_dir, "push_reg", &[
        (0, 0xF90002C0u32, "STR x0, [x22]"),
        (4, 0x910022D6u32, "ADD x22, x22, #8"),
        (8, 0xD65F03C0u32, "RET"),
    ], &[], 8);

    emit_naked_stencil(&mut emitter, &stencil_dir, "pop_reg", &[
        (0, 0xD10022D6u32, "SUB x22, x22, #8"),
        (4, 0xF94002C0u32, "LDR x0, [x22]"),
        (8, 0xD65F03C0u32, "RET"),
    ], &[], 8);

    emit_naked_stencil(&mut emitter, &stencil_dir, "ret", &[
        (0, 0xD65F03C0u32, "RET"),
    ], &[], 4);

    // ── Real C stencils (value holes + link holes) ─────────────────
    // load_const: void stencil(void) { rune_push(0xDEAD); }
    // Clang generates: MOV W0, #0xDEAD ; B _rune_push
    // Value at imm16 (byte 0, bits 20:5), link to rune_push via B (byte 4).
    // Used by LoadUndefined (0), LoadNull (2), LoadBoolean (4/6), LoadFloat64.
    emit_real_c_stencil(&mut emitter, &stencil_dir, "load_const",
        &[ValueCheck { offset: 0, mask: 0xFFE0001Fu32, expected: 0x52800000u32,
                        desc: "MOVZ W0, #?" }],
        &[HoleSpec { byte_offset: 0, bit_offset: 5, bit_width: 16 }],
        &[LinkHoleSpec { byte_offset: 4, helper_name: "rune_push" }],
    );

    // ── Real C stencils (value holes + link holes) ─────────────────
    // load_local: void stencil(void) { rune_load_local(0xDEAD); }
    // Clang generates: MOV W0, #0xDEAD ; B _rune_load_local
    emit_real_c_stencil(&mut emitter, &stencil_dir, "load_local",
        &[ValueCheck { offset: 0, mask: 0xFFE0001Fu32, expected: 0x52800000u32,
                        desc: "MOVZ W0, #?" }],
        &[HoleSpec { byte_offset: 0, bit_offset: 5, bit_width: 16 }],
        &[LinkHoleSpec { byte_offset: 4, helper_name: "rune_load_local" }],
    );

    // store_local: void stencil(void) { rune_store_local(0xDEAD); }
    // Clang generates: MOV W0, #0xDEAD ; B _rune_store_local
    emit_real_c_stencil(&mut emitter, &stencil_dir, "store_local",
        &[ValueCheck { offset: 0, mask: 0xFFE0001Fu32, expected: 0x52800000u32,
                        desc: "MOVZ W0, #?" }],
        &[HoleSpec { byte_offset: 0, bit_offset: 5, bit_width: 16 }],
        &[LinkHoleSpec { byte_offset: 4, helper_name: "rune_store_local" }],
    );

    // load_smi_16: void stencil(void) { rune_push(0xDEAD); }
    // Clang generates: MOV W0, #0xDEAD ; B _rune_push
    emit_real_c_stencil(&mut emitter, &stencil_dir, "load_smi_16",
        &[ValueCheck { offset: 0, mask: 0xFFE0001Fu32, expected: 0x52800000u32,
                        desc: "MOVZ W0, #?" }],
        &[HoleSpec { byte_offset: 0, bit_offset: 5, bit_width: 16 }],
        &[LinkHoleSpec { byte_offset: 4, helper_name: "rune_push" }],
    );

    // load_smi_32: same C stencil, different placeholder (needs MOVZ+MOVK)
    // Compiler generates MOVZ W0, #? + MOVK W0, #?, lsl #16 + B _rune_push
    emit_real_c_stencil(&mut emitter, &stencil_dir, "load_smi_32",
        &[
            ValueCheck { offset: 0, mask: 0xFFE0001Fu32, expected: 0x52800000u32,
                        desc: "MOVZ W0, #?" },
            ValueCheck { offset: 4, mask: 0xFFE0001Fu32, expected: 0x72A00000u32,
                        desc: "MOVK W0, #?, lsl #16" },
        ],
        &[
            HoleSpec { byte_offset: 0, bit_offset: 5, bit_width: 16 }, // lower 16
            HoleSpec { byte_offset: 4, bit_offset: 5, bit_width: 16 }, // upper 16
        ],
        &[LinkHoleSpec { byte_offset: 8, helper_name: "rune_push" }],
    );

    let code = emitter.render();
    fs::write(Path::new(&out_dir).join("stencils.rs"), code).unwrap();
}

// ── Data types ───────────────────────────────────────────────────────────

struct Stencil {
    name: String,
    bytes: Vec<u8>,
    holes: Vec<Hole>,
    link_holes: Vec<LinkHole>,
}

struct Helper {
    name: String,
    bytes: Vec<u8>,
}

struct Hole {
    byte_offset: usize,
    bit_offset: u8,
    bit_width: u8,
}

struct LinkHole {
    byte_offset: usize,
    helper_name: String,
}

struct ValueCheck {
    offset: usize,
    mask: u32,
    expected: u32,
    desc: &'static str,
}

struct HoleSpec {
    byte_offset: usize,
    bit_offset: u8,
    bit_width: u8,
}

struct LinkHoleSpec {
    byte_offset: usize,
    helper_name: &'static str,
}

// ── Emitter: collects helpers and stencils, generates Rust code ──────────

struct Emitter {
    helpers: Vec<Helper>,
    stencils: Vec<Stencil>,
}

impl Emitter {
    fn new() -> Self { Self { helpers: Vec::new(), stencils: Vec::new() } }

    fn add_helper(&mut self, h: &Helper) { self.helpers.push(Helper { name: h.name.clone(), bytes: h.bytes.clone() }); }

    fn add_stencil(&mut self, s: Stencil) { self.stencils.push(s); }

    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("// Auto-generated by build.rs — do not edit.\n");
        out.push_str("#[derive(Clone, Copy)]\n");
        out.push_str("#[allow(dead_code)]\n");
        out.push_str("pub struct StencilDef {\n");
        out.push_str("    pub name: &'static str,\n");
        out.push_str("    pub bytes: &'static [u8],\n");
        out.push_str("    pub holes: &'static [HoleDef],\n");
        out.push_str("    pub link_holes: &'static [LinkHoleDef],\n");
        out.push_str("}\n\n");
        out.push_str("#[derive(Clone, Copy)]\n");
        out.push_str("#[allow(dead_code)]\n");
        out.push_str("pub struct HelperDef {\n");
        out.push_str("    pub name: &'static str,\n");
        out.push_str("    pub bytes: &'static [u8],\n");
        out.push_str("}\n\n");
        out.push_str("#[derive(Clone, Copy)]\n");
        out.push_str("pub struct HoleDef {\n");
        out.push_str("    pub byte_offset: usize,\n");
        out.push_str("    pub bit_offset: u8,\n");
        out.push_str("    pub bit_width: u8,\n");
        out.push_str("}\n\n");
        out.push_str("#[derive(Clone, Copy)]\n");
        out.push_str("pub struct LinkHoleDef {\n");
        out.push_str("    pub byte_offset: usize,\n");
        out.push_str("    pub helper_name: &'static str,\n");
        out.push_str("}\n\n");

        // Helper constants
        for h in &self.helpers {
            out.push_str(&format!(
                "pub static {}_HELPER: HelperDef = HelperDef {{ name: \"{}\", bytes: &[{}] }};\n",
                h.name.to_uppercase(), h.name,
                h.bytes.iter().map(|b| format!("{b:#04x}")).collect::<Vec<_>>().join(", ")
            ));
        }
        out.push('\n');

        // Stencil constants
        for s in &self.stencils {
            out.push_str(&format!(
                "pub const {}_BYTES: &[u8] = &[{}];\n",
                s.name.to_uppercase(),
                s.bytes.iter().map(|b| format!("{b:#04x}")).collect::<Vec<_>>().join(", ")
            ));
            if s.holes.is_empty() {
                out.push_str(&format!("pub const {}_HOLES: &[HoleDef] = &[];\n", s.name.to_uppercase()));
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
            if s.link_holes.is_empty() {
                out.push_str(&format!("pub const {}_LINK_HOLES: &[LinkHoleDef] = &[];\n", s.name.to_uppercase()));
            } else {
                out.push_str(&format!(
                    "pub const {}_LINK_HOLES: &[LinkHoleDef] = &[{}];\n",
                    s.name.to_uppercase(),
                    s.link_holes.iter().map(|lh| {
                        format!("LinkHoleDef {{ byte_offset: {}, helper_name: \"{}\" }}",
                            lh.byte_offset, lh.helper_name)
                    }).collect::<Vec<_>>().join(", ")
                ));
            }
        }

        // All-stencils list
        out.push_str("\npub static ALL_STENCILS: &[StencilDef] = &[\n");
        for s in &self.stencils {
            out.push_str(&format!(
                "    StencilDef {{ name: \"{}\", bytes: {}_BYTES, holes: {}_HOLES, link_holes: {}_LINK_HOLES }},\n",
                s.name, s.name.to_uppercase(), s.name.to_uppercase(), s.name.to_uppercase()
            ));
        }
        out.push_str("];\n");

        // All-helpers list
        out.push_str("\npub static ALL_HELPERS: &[HelperDef] = &[\n");
        for h in &self.helpers {
            out.push_str(&format!(
                "    {}_HELPER,\n", h.name.to_uppercase()
            ));
        }
        out.push_str("];\n");

        out
    }
}

// ── Mach-O parser (minimal, macOS only) ─────────────────────────────────

const MH_MAGIC_64: u32 = 0xFEEDFACF;
const LC_SEGMENT_64: u32 = 0x19;

fn extract_text_section(o_bytes: &[u8]) -> Option<&[u8]> {
    if o_bytes.len() < 32 { return None; }
    if u32::from_le_bytes(o_bytes[0..4].try_into().unwrap()) != MH_MAGIC_64 { return None; }
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
                if &o_bytes[sec_off..sec_off+16] == b"__text\0\0\0\0\0\0\0\0\0\0" &&
                   &o_bytes[sec_off+16..sec_off+32] == b"__TEXT\0\0\0\0\0\0\0\0\0\0" {
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

/// Parse Mach-O relocations for a section, returning the offset + symbol index
/// for each ARM64_RELOC_BRANCH26 entry.
fn find_branch_relocs(o_bytes: &[u8]) -> Vec<(usize, u32)> {
    let mut result = Vec::new();
    if o_bytes.len() < 32 { return result; }
    if u32::from_le_bytes(o_bytes[0..4].try_into().unwrap()) != MH_MAGIC_64 { return result; }
    let ncmds = u32::from_le_bytes(o_bytes[16..20].try_into().unwrap());
    let mut offset: usize = 32;
    for _ in 0..ncmds {
        if offset + 8 > o_bytes.len() { return result; }
        let cmd = u32::from_le_bytes(o_bytes[offset..offset+4].try_into().unwrap());
        let cmdsize = u32::from_le_bytes(o_bytes[offset+4..offset+8].try_into().unwrap()) as usize;
        if cmd == LC_SEGMENT_64 {
            let nsects = u32::from_le_bytes(o_bytes[offset+64..offset+68].try_into().unwrap()) as usize;
            let sections_start = offset + 72;
            for i in 0..nsects {
                let sec_off = sections_start + i * 80;
                if sec_off + 80 > o_bytes.len() { return result; }
                // Check segname/sectname, or just process the first section
                let sectname = &o_bytes[sec_off..sec_off+16];
                if sectname != b"__text\0\0\0\0\0\0\0\0\0\0" { continue; }
                let reloff = u32::from_le_bytes(o_bytes[sec_off+56..sec_off+60].try_into().unwrap()) as usize;
                let nreloc = u32::from_le_bytes(o_bytes[sec_off+60..sec_off+64].try_into().unwrap()) as usize;
                for j in 0..nreloc {
                    let r_off = reloff + j * 8;
                    if r_off + 8 > o_bytes.len() { return result; }
                    let r_addr = i32::from_le_bytes(o_bytes[r_off..r_off+4].try_into().unwrap());
                    let r_info = u32::from_le_bytes(o_bytes[r_off+4..r_off+8].try_into().unwrap());
                    let r_type = (r_info >> 28) & 0xF; // bits 31:28
                    let r_extern = (r_info >> 27) & 1; // bit 27
                    let r_symbolnum = r_info & 0xFFFFFF; // bits 23:0
                    // ARM64_RELOC_BRANCH26 = 2
                    if r_type == 2 && r_extern == 1 {
                        result.push((r_addr as usize, r_symbolnum));
                    }
                }
            }
        }
        offset += cmdsize;
        if offset >= o_bytes.len() { break; }
    }
    result
}

// ── Compilation ──────────────────────────────────────────────────────────

fn compile_c(stencil_dir: &Path, src_stem: &str) -> Vec<u8> {
    let c_file = stencil_dir.join(format!("{src_stem}.c"));
    let obj_path = stencil_dir.join(format!("{src_stem}.o"));
    let output = Command::new("clang")
        .args(["-O2", "-c", "-ffreestanding", "-target", "arm64-apple-macos", "-o"])
        .arg(&obj_path)
        .arg(&c_file)
        .output()
        .expect("failed to execute clang");
    assert!(output.status.success(),
        "clang failed compiling {}:\n{}", c_file.display(), String::from_utf8_lossy(&output.stderr));
    fs::read(&obj_path).unwrap()
}

/// Compile a runtime helper function, strip its prologue/epilogue, and return the body bytes.
///
/// All helpers follow the same prologue/epilogue pattern (saving/restoring
/// callee-saved x22 and x21), so this function is generic. The body is the
/// span between prologue (STP) and epilogue (LDP + RET).
fn compile_helper(stencil_dir: &Path, name: &str) -> Helper {
    let obj = compile_c(stencil_dir, name);
    let section = extract_text_section(&obj).unwrap_or_else(|| panic!("helper {name}: no text section"));

    // Expected prologue: stp x22, x21, [sp, #-16]!
    // Expected epilogue: ldp x22, x21, [sp], #16 + ret
    let expected_prologue: u32 = 0xA9BF57F6;
    let expected_epilogue: u32 = 0xA8C157F6; // ldp x22, x21, [sp], #16
    let expected_ret: u32 = 0xD65F03C0;

    assert!(section.len() >= 16, "helper {name}: section too small: {} bytes", section.len());

    let prologue = u32::from_le_bytes(section[0..4].try_into().unwrap());
    assert_eq!(prologue, expected_prologue,
        "helper {name}: expected prologue {:#010x}, got {:#010x}", expected_prologue, prologue);

    let ret_offset = section.len() - 4;
    let ret_actual = u32::from_le_bytes(section[ret_offset..ret_offset + 4].try_into().unwrap());
    assert_eq!(ret_actual, expected_ret,
        "helper {name}: expected RET at end ({:#010x}), got {:#010x}", expected_ret, ret_actual);

    let epilogue_offset = ret_offset - 4;
    let epilogue = u32::from_le_bytes(section[epilogue_offset..epilogue_offset + 4].try_into().unwrap());
    assert_eq!(epilogue, expected_epilogue,
        "helper {name}: expected epilogue {:#010x} at offset {}, got {:#010x}",
        expected_epilogue, epilogue_offset, epilogue);

    let body = section[4..epilogue_offset].to_vec();
    assert!(!body.is_empty(), "helper {name}: empty body");
    assert_eq!(body.len() % 4, 0, "helper {name}: body length {} not multiple of 4", body.len());

    Helper { name: name.to_string(), bytes: body }
}

/// Compile a naked-asm stencil, verify instructions, strip RET.
fn emit_naked_stencil(emitter: &mut Emitter, stencil_dir: &Path, name: &str,
                       checks: &[(usize, u32, &str)], holes: &[HoleSpec], body_len: usize) {
    let obj = compile_c(stencil_dir, name);
    let section = extract_text_section(&obj).unwrap_or_else(|| panic!("{name}: no text section"));

    for (offset, expected, desc) in checks {
        let instr = u32::from_le_bytes(section[*offset..offset+4].try_into().unwrap());
        assert_eq!(instr, *expected, "{name}[{}]: expected {} ({:#010x}), got {:#010x}", offset, desc, expected, instr);
    }

    let body = section[..body_len].to_vec();
    emitter.add_stencil(Stencil {
        name: name.to_string(),
        bytes: body,
        holes: holes.iter().map(|h| Hole { byte_offset: h.byte_offset, bit_offset: h.bit_offset, bit_width: h.bit_width }).collect(),
        link_holes: vec![],
    });
}

/// Compile a real C stencil (calls a helper), verify instructions, extract
/// value holes and link holes.
fn emit_real_c_stencil(emitter: &mut Emitter, stencil_dir: &Path, name: &str,
                        value_checks: &[ValueCheck],
                        holes: &[HoleSpec], link_holes: &[LinkHoleSpec]) {
    let obj = compile_c(stencil_dir, name);
    let section = extract_text_section(&obj).unwrap_or_else(|| panic!("{name}: no text section"));

    // Verify expected instruction pattern
    for vc in value_checks {
        let instr = u32::from_le_bytes(section[vc.offset..vc.offset+4].try_into().unwrap());
        let masked = instr & vc.mask;
        assert_eq!(masked, vc.expected,
            "{name}[{}]: expected {} ({:#010x}), got masked={:#010x} full={:#010x}",
            vc.offset, vc.desc, vc.expected, masked, instr);
    }

    // Parse branch relocations for link holes
    let branch_relocs = find_branch_relocs(&obj);

    // Verify we found the expected link holes
    assert_eq!(branch_relocs.len(), link_holes.len(),
        "{name}: found {} branch relocs, expected {} link holes",
        branch_relocs.len(), link_holes.len());

    // The stencil body is from offset 0 to just before the epilogue.
    // For a tail-call stencil: MOV[Z] + B = 8 bytes (no RET).
    // We find the body length from the last expected instruction
    // (the branch is the tail-call, no epilogue after it).
    let last_value_offset = value_checks.iter().map(|vc| vc.offset + 4).max().unwrap_or(0);
    // The branch instruction is a link hole — include it in the body.
    let branch_ends_at = link_holes.iter().map(|lh| lh.byte_offset + 4).max().unwrap_or(0);
    let body_end = std::cmp::max(last_value_offset, branch_ends_at);
    let body = section[..body_end].to_vec();

    emitter.add_stencil(Stencil {
        name: name.to_string(),
        bytes: body,
        holes: holes.iter().map(|h| Hole { byte_offset: h.byte_offset, bit_offset: h.bit_offset, bit_width: h.bit_width }).collect(),
        link_holes: link_holes.iter().map(|lh| LinkHole {
            byte_offset: lh.byte_offset,
            helper_name: lh.helper_name.to_string(),
        }).collect(),
    });
}
