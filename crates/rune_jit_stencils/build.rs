/// build.rs — Stencil compiler for copy-and-patch JIT.
///
/// At build time, we generate stencil byte sequences using a minimal AArch64
/// instruction encoder. Each stencil is a pre-compiled instruction sequence
/// with "holes" (placeholder immediates/offsets) that get patched at runtime.
///
/// The generated Rust code embeds stencil bytes + hole descriptors as constants,
/// eliminating the C compiler dependency of the original copy-and-patch paper.
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let stencils = generate_stencils();
    let code = render_stencils(&stencils);
    fs::write(Path::new(&out_dir).join("stencils.rs"), code).unwrap();
    println!("cargo::rerun-if-changed=build.rs");
}

/// A stencil: a sequence of machine-code bytes with patchable holes.
#[allow(dead_code)]
struct Stencil {
    name: &'static str,
    bytes: Vec<u8>,
    holes: Vec<Hole>,
}

/// A hole in a stencil — a bit field to be patched at runtime.
struct Hole {
    byte_offset: usize,
    bit_offset: u8,
    bit_width: u8,
}

// ── AArch64 instruction encoder (minimal, for stencil generation only) ──────

#[allow(dead_code)]
mod aarch64_enc {
    /// MOVZ: Move wide with zero (64-bit).
    /// Encoding: 1 10 10 1 01 hw imm16 Rd
    pub fn movz(rd: u8, imm16: u16, shift: u8) -> u32 {
        assert!(rd < 32);
        let hw = shift / 16;
        assert!(hw <= 3);
        0xD2800000 | ((hw as u32) << 21) | ((imm16 as u32) << 5) | (rd as u32)
    }

    /// MOVK: Move wide with keep (64-bit).
    /// Same encoding as MOVZ but with opc=11.
    pub fn movk(rd: u8, imm16: u16, shift: u8) -> u32 {
        assert!(rd < 32);
        let hw = shift / 16;
        assert!(hw <= 3);
        0xF2800000 | ((hw as u32) << 21) | ((imm16 as u32) << 5) | (rd as u32)
    }

    /// STR xd, [xn, #uoffset] — unsigned offset, scaled by 8
    pub fn str_off(xd: u8, xn: u8, uoffset: u32) -> u32 {
        assert!(uoffset % 8 == 0, "str_off offset must be 8-aligned");
        0xF9000000 | ((uoffset >> 3) << 10) | ((xn as u32) << 5) | (xd as u32)
    }

    /// LDR xd, [xn, #uoffset] — unsigned offset, scaled by 8
    pub fn ldr_off(xd: u8, xn: u8, uoffset: u32) -> u32 {
        assert!(uoffset % 8 == 0, "ldr_off offset must be 8-aligned");
        0xF9400000 | ((uoffset >> 3) << 10) | ((xn as u32) << 5) | (xd as u32)
    }

    /// ADD xd, xn, #imm12
    pub fn add_imm(xd: u8, xn: u8, imm12: u16) -> u32 {
        assert!(imm12 < 4096);
        0x91000000 | ((imm12 as u32) << 10) | ((xn as u32) << 5) | (xd as u32)
    }

    /// SUB xd, xn, #imm12
    pub fn sub_imm(xd: u8, xn: u8, imm12: u16) -> u32 {
        assert!(imm12 < 4096);
        0xD1000000 | ((imm12 as u32) << 10) | ((xn as u32) << 5) | (xd as u32)
    }

    /// RET: Return from subroutine.
    pub fn ret() -> u32 {
        0xD65F03C0
    }

    /// Encode 4 bytes (a single instruction) and append to buffer.
    pub fn emit(buf: &mut Vec<u8>, instr: u32) {
        buf.extend_from_slice(&instr.to_le_bytes());
    }
}

// ── Stencil definitions ───────────────────────────────────────────────────

fn generate_stencils() -> Vec<Stencil> {
    let mut stencils = Vec::new();

    // ── PushRegister stencil ──────────────────────────────────────────────
    // Push x0 onto JIT stack.
    //   str x0, [x22]       ; store at current JIT stack top
    //   add x22, x22, #8    ; increment JIT stack pointer
    {
        let mut buf = Vec::new();
        aarch64_enc::emit(&mut buf, aarch64_enc::str_off(0, 22, 0));
        aarch64_enc::emit(&mut buf, aarch64_enc::add_imm(22, 22, 8));
        stencils.push(Stencil {
            name: "push_reg",
            bytes: buf,
            holes: vec![],
        });
    }

    // ── PopRegister stencil ───────────────────────────────────────────────
    // Pop from JIT stack into x0.
    //   sub x22, x22, #8    ; decrement JIT stack pointer
    //   ldr x0, [x22]       ; load from new top
    {
        let mut buf = Vec::new();
        aarch64_enc::emit(&mut buf, aarch64_enc::sub_imm(22, 22, 8));
        aarch64_enc::emit(&mut buf, aarch64_enc::ldr_off(0, 22, 0));
        stencils.push(Stencil {
            name: "pop_reg",
            bytes: buf,
            holes: vec![],
        });
    }

    // ── LoadSmi stencil (16-bit immediate) ────────────────────────────────
    // Load an Smi immediate and push it onto JIT stack.
    //   movz x0, #imm16     ; load Smi-encoded immediate
    //   str x0, [x22]       ; push
    //   add x22, x22, #8
    {
        let mut buf = Vec::new();
        aarch64_enc::emit(&mut buf, aarch64_enc::movz(0, 0, 0));
        aarch64_enc::emit(&mut buf, aarch64_enc::str_off(0, 22, 0));
        aarch64_enc::emit(&mut buf, aarch64_enc::add_imm(22, 22, 8));
        stencils.push(Stencil {
            name: "load_smi_16",
            bytes: buf,
            holes: vec![
                Hole { byte_offset: 0, bit_offset: 5, bit_width: 16 }, // imm16 in MOVZ
            ],
        });
    }

    // ── LoadSmi stencil (32-bit immediate, movz+movk) ─────────────────────
    // For i31-range Smis, the immediate fits in 31 bits. We encode as
    // movz + movk with the lower 16 bits in movz, upper 16 bits in movk.
    // (Bits 31..<shift go into the next movk.)
    {
        let mut buf = Vec::new();
        aarch64_enc::emit(&mut buf, aarch64_enc::movz(0, 0, 0));
        aarch64_enc::emit(&mut buf, aarch64_enc::movk(0, 1, 16));
        aarch64_enc::emit(&mut buf, aarch64_enc::str_off(0, 22, 0));
        aarch64_enc::emit(&mut buf, aarch64_enc::add_imm(22, 22, 8));
        stencils.push(Stencil {
            name: "load_smi_32",
            bytes: buf,
            holes: vec![
                Hole { byte_offset: 0, bit_offset: 5, bit_width: 16 }, // lower 16 bits in MOVZ
                Hole { byte_offset: 4, bit_offset: 5, bit_width: 16 }, // upper 16 bits in MOVK
            ],
        });
    }

    // ── Ret stencil ───────────────────────────────────────────────────────
    {
        let mut buf = Vec::new();
        aarch64_enc::emit(&mut buf, aarch64_enc::ret());
        stencils.push(Stencil {
            name: "ret",
            bytes: buf,
            holes: vec![],
        });
    }

    stencils
}

// ── Code generation ───────────────────────────────────────────────────────

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
