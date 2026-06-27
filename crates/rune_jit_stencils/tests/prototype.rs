//! Prototype: Clang-compiled stencil end-to-end validation.
//!
//! This test validates Path A (the copy-and-patch paper's approach):
//! 1. Write a C stencil function using inline assembly
//! 2. Compile with Clang
//! 3. Parse the .o file to extract function bytes
//! 4. Identify the patchable hole (immediate in MOVZ)
//! 5. Patch with a runtime value and verify correctness
//!
//! If this test passes, Path A is viable. If Clang output changes across
//! versions and this test breaks, Path A has a maintenance cost.

use std::process::Command;
use std::io::Write;

/// Mach-O constants for minimal parser.
const MH_MAGIC_64: u32 = 0xFEEDFACF;
const LC_SEGMENT_64: u32 = 0x19;
const SECT_NAME: &[u8; 16] = b"__text\0\0\0\0\0\0\0\0\0\0";
const SEG_NAME: &[u8; 16] = b"__TEXT\0\0\0\0\0\0\0\0\0\0";

/// Parse a Mach-O object file and extract the __TEXT,__text section bytes.
fn extract_text_section(o_bytes: &[u8]) -> Option<&[u8]> {
    if o_bytes.len() < 32 { return None; }
    let magic = u32::from_le_bytes(o_bytes[0..4].try_into().unwrap());
    if magic != MH_MAGIC_64 { return None; }

    let ncmds = u32::from_le_bytes(o_bytes[16..20].try_into().unwrap());
    let mut offset: usize = 32; // past mach_header_64

    for _ in 0..ncmds {
        if offset + 8 > o_bytes.len() { return None; }
        let cmd = u32::from_le_bytes(o_bytes[offset..offset+4].try_into().unwrap());
        let cmdsize = u32::from_le_bytes(o_bytes[offset+4..offset+8].try_into().unwrap()) as usize;
        if cmd == LC_SEGMENT_64 {
            let nsects_offset = offset + 64;
            if nsects_offset + 4 > o_bytes.len() { return None; }
            let nsects = u32::from_le_bytes(o_bytes[nsects_offset..nsects_offset+4].try_into().unwrap()) as usize;

            let sections_offset = offset + 72;
            for i in 0..nsects {
                let sec_off = sections_offset + i * 80;
                if sec_off + 80 > o_bytes.len() { return None; }
                let sectname = &o_bytes[sec_off..sec_off+16];
                let segname = &o_bytes[sec_off+16..sec_off+32];
                if sectname == SECT_NAME && segname == SEG_NAME {
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

/// Find a stencil function symbol in the Mach-O to get exact byte bounds.
fn find_function_bytes<'a>(o_bytes: &'a [u8], _name: &str) -> Option<&'a [u8]> {
    // Get the text section first
    let text_section = extract_text_section(o_bytes)?;

    // In a simple .o with one function, the text section is the function body.
    // The function starts at the beginning of the text section.
    // (Symbol table parsing would give exact bounds, but for the prototype
    //  we know the section contains only our function.)
    Some(text_section)
}

/// Verify that the function bytes match the expected instruction template:
///   MOVZ x0, #imm16  (imm16 varies — the placeholder/hole)
///   STR  x0, [x22]    (fixed encoding)
///   ADD  x22, x22, #8 (fixed encoding)
///
/// Returns the three instructions if they match.
fn verify_template(bytes: &[u8]) -> Result<[u32; 3], String> {
    if bytes.len() < 12 {
        return Err(format!("stencil too short: {} bytes", bytes.len()));
    }

    let mut instrs = [0u32; 3];
    for (i, instr) in instrs.iter_mut().enumerate() {
        let start = i * 4;
        *instr = u32::from_le_bytes(bytes[start..start+4].try_into().unwrap());
    }

    // Check MOVZ: var field is imm16 (bits 20:5), all other bits are fixed.
    const MOVZ_MASK: u32 = 0xFFE0001Fu32; // checks bits 31:21 + bits 4:0 (Rd)
    if (instrs[0] & MOVZ_MASK) != 0xD2800000u32 {
        return Err(format!(
            "instr 0: expected MOVZ pattern, got {:#010x} (full: {:#010x})",
            instrs[0] & MOVZ_MASK, instrs[0]
        ));
    }

    // STR x0, [x22] — fixed: opcode + Rn=22 + Rt=0
    if instrs[1] != 0xF90002C0u32 {
        return Err(format!(
            "instr 1: expected STR x0, [x22] ({:#010x}), got {:#010x}",
            0xF90002C0u32, instrs[1]
        ));
    }

    // ADD x22, x22, #8 — fixed: opcode + Rn=22 + Rd=22 + imm12=8
    if instrs[2] != 0x910022D6u32 {
        return Err(format!(
            "instr 2: expected ADD x22, x22, #8 ({:#010x}), got {:#010x}",
            0x910022D6u32, instrs[2]
        ));
    }

    Ok(instrs)
}

/// Write C source to a temp file, compile with Clang, return .o bytes.
fn compile_stencil_c(c_source: &str) -> Vec<u8> {
    let dir = std::env::temp_dir().join(format!("stencil_proto_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);

    let src_path = dir.join("stencil.c");
    let obj_path = dir.join("stencil.o");
    let mut f = std::fs::File::create(&src_path).unwrap();
    f.write_all(c_source.as_bytes()).unwrap();
    drop(f);

    let output = Command::new("clang")
        .args([
            "-O2",
            "-c",
            "-ffreestanding",
            "-target", "arm64-apple-macos",
            "-o",
        ])
        .arg(&obj_path)
        .arg(&src_path)
        .output()
        .expect("failed to execute clang");

    assert!(output.status.success(),
        "clang failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr));

    std::fs::read(&obj_path).unwrap()
}

/// Extract the immediate field from a MOVZ instruction.
fn extract_movz_imm(instr: u32) -> u32 {
    (instr >> 5) & 0xFFFF
}

/// Patch a MOVZ immediate into an instruction.
fn patch_movz_imm(instr: u32, imm16: u16) -> u32 {
    (instr & !0x001FFFE0) | ((imm16 as u32) << 5)
}

#[test]
fn test_prototype_clang_pipeline() {
    let c_source = r#"
#include <stdint.h>

__attribute__((naked))
void load_smi_stencil(void) {
    __asm__(
        "mov x0, #0xDEAD\n\t"
        "str x0, [x22]\n\t"
        "add x22, x22, #8\n\t"
        "ret"
        :
        :
        : "x0", "x22", "memory"
    );
}
"#;

    let obj_bytes = compile_stencil_c(c_source);
    assert!(obj_bytes.len() > 64, "object file too small: {} bytes", obj_bytes.len());

    // Extract function bytes from the object file
    let func_bytes = find_function_bytes(&obj_bytes, "load_smi_stencil")
        .expect("failed to find __TEXT,__text section in object file");

    eprintln!("function bytes ({} total): {:02x?}", func_bytes.len(), func_bytes);

    // Verify instruction template
    let instrs = verify_template(func_bytes).expect("instruction template mismatch");

    // Extract the placeholder immediate from MOVZ
    let placeholder = extract_movz_imm(instrs[0]);
    assert_eq!(placeholder, 0xDEAD,
        "expected placeholder 0xDEAD, got {:#06x}", placeholder);

    eprintln!("placeholder MOVZ imm16: {:#06x}", placeholder);
    eprintln!("STR instruction: {:#010x}", instrs[1]);
    eprintln!("ADD instruction: {:#010x}", instrs[2]);
}

#[test]
fn test_prototype_patch_stencil() {
    // Same C source with placeholder 0xDEAD
    let c_source = r#"
#include <stdint.h>

__attribute__((naked))
void load_smi_stencil(void) {
    __asm__(
        "mov x0, #0xDEAD\n\t"
        "str x0, [x22]\n\t"
        "add x22, x22, #8\n\t"
        "ret"
        :
        :
        : "x0", "x22", "memory"
    );
}
"#;

    let obj_bytes = compile_stencil_c(c_source);
    let func_bytes = find_function_bytes(&obj_bytes, "load_smi_stencil")
        .expect("failed to find __TEXT,__text section");

    // Patch the stencil: replace placeholder 0xDEAD with 42
    // The hole is at byte_offset=0, bit_offset=5, bit_width=16
    let stencil_bytes = &func_bytes[..12]; // strip the RET (we add it separately at link time)
    let mut patched = stencil_bytes.to_vec();

    let smi_val = (42u64 << 1) | 1; // Smi(42)
    let word = u32::from_le_bytes(patched[0..4].try_into().unwrap());
    let patched_word = patch_movz_imm(word, smi_val as u16);
    patched[0..4].copy_from_slice(&patched_word.to_le_bytes());

    // Verify: MOVZ should now encode Smi(42) = 85 = 0x55
    // Expected: 0xD2800000 | (0x55 << 5) = 0xD2800AA0
    let expected_movz: u32 = 0xD2800000 | (0x55_u32 << 5);
    let actual_movz = u32::from_le_bytes(patched[0..4].try_into().unwrap());
    assert_eq!(actual_movz, expected_movz,
        "patched MOVZ: expected {:#010x}, got {:#010x}",
        expected_movz, actual_movz);

    // Verify the rest is unchanged
    let actual_str = u32::from_le_bytes(patched[4..8].try_into().unwrap());
    assert_eq!(actual_str, 0xF90002C0u32, "STR changed after patch");
    let actual_add = u32::from_le_bytes(patched[8..12].try_into().unwrap());
    assert_eq!(actual_add, 0x910022D6u32, "ADD changed after patch");

    eprintln!("Patching prototype: MOVZ 0xDEAD → Smi(42) = 0x55");
    eprintln!("  Patched bytes: {:02x?}", patched);
}

#[test]
fn test_prototype_clang_determinism() {
    // Verify that the same C source produces the same bytes across compilations.
    let c_source = r#"
#include <stdint.h>

__attribute__((naked))
void load_smi_stencil(void) {
    __asm__(
        "mov x0, #0xDEAD\n\t"
        "str x0, [x22]\n\t"
        "add x22, x22, #8\n\t"
        "ret"
        :
        :
        : "x0", "x22", "memory"
    );
}
"#;

    let bytes1 = compile_stencil_c(c_source);
    let bytes2 = compile_stencil_c(c_source);

    let func1 = find_function_bytes(&bytes1, "load_smi_stencil").unwrap();
    let func2 = find_function_bytes(&bytes2, "load_smi_stencil").unwrap();

    assert_eq!(func1, func2,
        "Clang produced different bytes on second compilation\nfirst:  {:02x?}\nsecond: {:02x?}",
        func1, func2);

    eprintln!("Clang determinism check: identical bytes ({} bytes)", func1.len());
}
