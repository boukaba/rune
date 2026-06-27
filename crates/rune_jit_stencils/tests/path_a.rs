//! Real Path A prototype: C stencil calling runtime helper, Clang compiles everything.
//!
//! This validates the core copy-and-patch value proposition:
//! 1. C stencil with hardcoded placeholder → Clang generates MOV + B
//! 2. Runtime helper with inline asm → Clang saves/restores JIT regs
//! 3. Build-time: extract bytes, strip prologue/epilogue, identify holes
//! 4. JIT runtime: patch value hole + link-time hole, emit

use std::process::Command;
use std::io::Write;
use std::path::Path;

// ── Mach-O parser (same as prototype test) ─────────────────────────────────

const MH_MAGIC_64: u32 = 0xFEEDFACF;
const LC_SEGMENT_64: u32 = 0x19;

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

// ── Relocation parser (simplified Mach-O) ─────────────────────────────────

/// Parse Mach-O relocations for the __TEXT,__text section.
/// Returns the relocation info for the first branch relocation found.
fn find_branch_reloc(o_bytes: &[u8]) -> Option<(usize, u32)> {
    // Find the __TEXT,__text section header to get reloff and nreloc
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
                if &o_bytes[sec_off..sec_off+16] == b"__text\0\0\0\0\0\0\0\0\0\0" {
                    // reloff at section offset 56, nreloc at 60
                    let reloff = u32::from_le_bytes(o_bytes[sec_off+56..sec_off+60].try_into().unwrap()) as usize;
                    let nreloc = u32::from_le_bytes(o_bytes[sec_off+60..sec_off+64].try_into().unwrap()) as usize;
                    // Parse each relocation entry (8 bytes each)
                    for j in 0..nreloc {
                        let r_off = reloff + j * 8;
                        if r_off + 8 > o_bytes.len() { return None; }
                        let r_addr = i32::from_le_bytes(o_bytes[r_off..r_off+4].try_into().unwrap());
                        let r_info = u32::from_le_bytes(o_bytes[r_off+4..r_off+8].try_into().unwrap());
                        // Mach-O relocation_info on Apple ARM64:
                        // r_symbolnum:24 (bits 23:0), r_pcrel:1 (bit 24),
                        // r_length:2 (bits 26:25), r_extern:1 (bit 27),
                        // r_type:4 (bits 31:28)
                        let r_type = (r_info >> 28) & 0xF;
                        let r_extern = (r_info >> 27) & 1;
                        if r_type == 2 && r_extern == 1 {
                            // ARM64_RELOC_BRANCH26: the address is the instruction offset
                            return Some((r_addr as usize, r_info & 0xFFFFFF));
                        }
                    }
                }
            }
        }
        offset += cmdsize;
        if offset >= o_bytes.len() { break; }
    }
    None
}

// ── Compile C files ────────────────────────────────────────────────────────

fn compile_c(src: &str, out_path: &Path) -> Vec<u8> {
    let dir = out_path.parent().unwrap();
    let _ = std::fs::create_dir_all(dir);

    let src_path = dir.join("input.c");
    let mut f = std::fs::File::create(&src_path).unwrap();
    f.write_all(src.as_bytes()).unwrap();
    drop(f);

    let output = Command::new("clang")
        .args(["-O2", "-c", "-ffreestanding", "-target", "arm64-apple-macos", "-o"])
        .arg(out_path)
        .arg(&src_path)
        .output()
        .expect("failed to execute clang");

    assert!(output.status.success(),
        "clang failed:\n{}", String::from_utf8_lossy(&output.stderr));
    std::fs::read(out_path).unwrap()
}

// ── Tests ─────────────────────────────────────────────────────────────────

 #[test]
 fn test_real_c_stencil_pipeline() {
     let tmp = std::env::temp_dir().join("rune_path_a_proto");
     let _ = std::fs::remove_dir_all(&tmp);
     let _ = std::fs::create_dir_all(&tmp);

     // ── Helper: rune_push ────────────────────────────────────────────
     let helper_c = r#"
#include <stdint.h>
void rune_push(int64_t val) {
    __asm__(
        "str %[val], [x22]\n\t"
        "add x22, x22, #8"
        : : [val] "r" (val) : "x22", "memory"
    );
}
"#;
     let helper_o = compile_c(helper_c, &tmp.join("helper.o"));
     let helper_raw = extract_text_section(&helper_o)
         .expect("helper: no __TEXT,__text");

     // Helper compiled with prologue/epilogue (saves/restores callee-saved x22).
     // Expected: stp x22,x21,[sp,#-16]! ; str x0,[x22] ; add x22,x22,#8 ; ldp x22,x21,[sp],#16 ; ret
     // We strip prologue(4 bytes) and epilogue(8 bytes), keep body(8 bytes).
     eprintln!("helper raw section ({} bytes): {:02x?}", helper_raw.len(), helper_raw);

     // Verify helper body
     assert!(helper_raw.len() >= 16, "helper too small: {}", helper_raw.len());
     let helper_body = &helper_raw[4..12]; // skip prologue, take str+add, skip epilogue+ret
     let str_instr = u32::from_le_bytes(helper_body[0..4].try_into().unwrap());
     let add_instr = u32::from_le_bytes(helper_body[4..8].try_into().unwrap());
     assert_eq!(str_instr, 0xF90002C0u32, "helper: expected STR x0,[x22], got {:#010x}", str_instr);
     assert_eq!(add_instr, 0x910022D6u32, "helper: expected ADD x22,x22,#8, got {:#010x}", add_instr);
     eprintln!("helper body (stripped): {:02x?}", helper_body);

     // ── Stencil: load_smi ────────────────────────────────────────────
     let stencil_c = r#"
#include <stdint.h>
void rune_push(int64_t val);
void stencil_load_smi(void) {
    rune_push(0xDEAD);  // placeholder — see MOV W0 in compiled output
}
"#;
     let stencil_o = compile_c(stencil_c, &tmp.join("stencil.o"));
     let stencil_raw = extract_text_section(&stencil_o)
         .expect("stencil: no __TEXT,__text");
     eprintln!("stencil raw section ({} bytes): {:02x?}", stencil_raw.len(), stencil_raw);

     // Expected: mov w0, #0xDEAD (4 bytes) ; b /placehoder/ (4 bytes) = 8 bytes
     assert!(stencil_raw.len() >= 8, "stencil too small: {}", stencil_raw.len());
     let mov_instr = u32::from_le_bytes(stencil_raw[0..4].try_into().unwrap());
     let branch_instr = u32::from_le_bytes(stencil_raw[4..8].try_into().unwrap());

    // Verify MOV pattern: MOVZ W0, #? (32-bit form, sf=0)
    // 32-bit MOVZ: 0x52800000 | (imm16 << 5) | Rd
    // Mask excludes imm16 (bits 20:5) and checks everything else
    const MOV_W0_MASK: u32 = 0xFFE0001Fu32;
    let mov_base = mov_instr & MOV_W0_MASK;
    assert_eq!(mov_base, 0x52800000u32,
         "stencil[0]: expected MOVZ W0 pattern {:#010x}, got {:#010x}, full: {:#010x}",
         0x52800000u32, mov_base, mov_instr);

     // Extract the placeholder
     let placeholder = (mov_instr >> 5) & 0xFFFF;
     assert_eq!(placeholder, 0xDEAD,
         "expected placeholder 0xDEAD, got {:#06x}", placeholder);
     eprintln!("MOV W0 placeholder imm16: {:#06x}", placeholder);

     // Verify branch instruction: B (unconditional, bit 31 = 0) or BL (bit 31 = 1)
     // Both use 26-bit offset at bits 25:0
     let is_b = (branch_instr >> 31) == 0;
     let is_bl = (branch_instr >> 31) == 1;
     assert!(is_b || is_bl, "stencil[1]: expected B or BL, got {:#010x}", branch_instr);
     eprintln!("branch type: {}", if is_b { "B (tail call)" } else { "BL (call)" });

     // Find the branch relocation
     let (reloc_addr, reloc_sym) = find_branch_reloc(&stencil_o)
         .expect("stencil: no branch relocation found");
     assert_eq!(reloc_addr, 4, "expected branch reloc at offset 4, got {reloc_addr}");
     eprintln!("branch relocation: offset 4, symbol {reloc_sym}");

     // ── Simulate JIT runtime patching ────────────────────────────────
     // We have:
     //   stencil body (8 bytes): mov w0, #0xDEAD ; b /placeholder/
     //   helper body (8 bytes): str x0,[x22] ; add x22,x22,#8
     //
     // At JIT compile time:
     // 1. Patch MOV W0: replace 0xDEAD with actual Smi value
     // 2. Patch B: compute offset from stencil to helper in JIT buffer
     // 3. Emit stencil body + helper body

     let smi_val: u64 = (42 << 1) | 1; // Smi(42) = 85 = 0x55

     let mut jit_buf = [0u8; 64];
     let stencil_offset = 8;   // stencil starts at offset 8 (after some header)
     let helper_offset = 24;   // helper at offset 24

     // Copy stencil body
     jit_buf[stencil_offset..stencil_offset+8].copy_from_slice(&stencil_raw[..8]);
     // Copy helper body
     jit_buf[helper_offset..helper_offset+8].copy_from_slice(&helper_body[..8]);

     // Patch MOV W0: replace 0xDEAD with Smi(42) = 0x55
     let mov_word = u32::from_le_bytes(jit_buf[stencil_offset..stencil_offset+4].try_into().unwrap());
     let patched_mov = (mov_word & !0x001FFFE0u32) | ((smi_val as u32) << 5);
     jit_buf[stencil_offset..stencil_offset+4].copy_from_slice(&patched_mov.to_le_bytes());

     // Verify MOV W0 now encodes Smi(42) = 0x55
     let expected_mov: u32 = 0x52800000 | (0x55u32 << 5); // MOVZ W0, #0x55
     let actual_mov = u32::from_le_bytes(jit_buf[stencil_offset..stencil_offset+4].try_into().unwrap());
     assert_eq!(actual_mov, expected_mov,
         "patched MOV: expected {:#010x}, got {:#010x}", expected_mov, actual_mov);

     // Patch B: compute offset
     // BL/B offset is in instructions (28-bit signed, divided by 4)
     // target = helper_offset, source = stencil_offset + 4 (PC at B instruction)
     let branch_pc = stencil_offset + 4;
     let target_pc = helper_offset;
     let offset_in_instrs = ((target_pc as i64 - branch_pc as i64) / 4) as i32;
     // 26-bit signed check
     assert!((-1 << 25..(1 << 25)).contains(&offset_in_instrs),
         "B offset {} out of range", offset_in_instrs);

     let branch_word = u32::from_le_bytes(jit_buf[stencil_offset+4..stencil_offset+8].try_into().unwrap());
     let patched_branch = (branch_word & !0x03FFFFFFu32) | ((offset_in_instrs as u32) & 0x3FFFFFF);
     jit_buf[stencil_offset+4..stencil_offset+8].copy_from_slice(&patched_branch.to_le_bytes());

     // Verify B instruction now targets helper
     let actual_branch = u32::from_le_bytes(jit_buf[stencil_offset+4..stencil_offset+8].try_into().unwrap());
     let actual_offset = (actual_branch & 0x03FFFFFF) as i32;
     // Sign extend 26-bit to 32-bit
     let actual_offset_se = if actual_offset & (1 << 25) != 0 {
         actual_offset | !0x3FFFFFF
     } else {
         actual_offset
     };
     assert_eq!(actual_offset_se, offset_in_instrs,
         "B offset: expected {}, got {}", offset_in_instrs, actual_offset_se);

     // Verify the helper bytes are unchanged
     let actual_str = u32::from_le_bytes(jit_buf[helper_offset..helper_offset+4].try_into().unwrap());
     assert_eq!(actual_str, 0xF90002C0u32, "helper STR changed");
     let actual_add = u32::from_le_bytes(jit_buf[helper_offset+4..helper_offset+8].try_into().unwrap());
     assert_eq!(actual_add, 0x910022D6u32, "helper ADD changed");

     eprintln!("\n=== Path A validation: PASSED ===");
     eprintln!("JIT buffer layout:");
     eprintln!("  [{stencil_offset:2}..{:2}] stencil: MOV W0, #{smi_val} ; B #{offset_in_instrs}", stencil_offset+7);
     eprintln!("  [{helper_offset:2}..{:2}] helper:  STR x0,[x22] ; ADD x22,x22,#8", helper_offset+7);
     eprintln!("Full JIT buffer: {:02x?}", &jit_buf[..32]);

     // Clean up
     let _ = std::fs::remove_dir_all(&tmp);
 }

 #[test]
 fn test_path_a_load_smi_verify_encoding() {
     // Verify that the 32-bit MOVZ W0 encoding is correct for the placeholder.
     // 0x52800000 = MOVZ W0, #0, LSL #0
     // 0x52800000 | (0xDEAD << 5) = 0x529BD5A0
     let expected: u32 = 0x52800000 | (0xDEADu32 << 5);
     assert_eq!(expected, 0x529BD5A0u32,
         "MOVZ W0, #0xDEAD encoding: expected {:#010x}, got {:#010x}",
         0x529BD5A0u32, expected);
 }

 #[test]
 fn test_path_a_stencil_with_different_placeholder() {
     // Verify that a different placeholder value works and Clang still
     // generates MOVZ W0 (not literal pool).
     let tmp = std::env::temp_dir().join("rune_path_a_proto2");
     let _ = std::fs::remove_dir_all(&tmp);
     let _ = std::fs::create_dir_all(&tmp);

     // Use a 12-bit placeholder (fits in MOVZ single immediate)
     let stencil_c = r#"
#include <stdint.h>
void rune_push(int64_t val);
void stencil_load_smi(void) {
    rune_push(0xFFF);  // small placeholder
}
"#.to_string();
     let stencil_o = compile_c(&stencil_c, &tmp.join("stencil.o"));
     let stencil_raw = extract_text_section(&stencil_o)
         .expect("stencil: no __TEXT,__text");

     let mov_instr = u32::from_le_bytes(stencil_raw[0..4].try_into().unwrap());
     let placeholder = (mov_instr >> 5) & 0xFFFF;
     assert_eq!(placeholder, 0xFFF,
         "expected placeholder 0xFFF, got {:#06x}", placeholder);

     let _ = std::fs::remove_dir_all(&tmp);
 }

#[test]
fn test_path_a_load_const_encoding() {
    // Verify that the real load_const.c compiles to MOVZ W0 + B _rune_push.
    // This is the Path A validation for the load_const stencil.
    // Uses the actual C file in the stencils directory (not an inline string)
    // to ensure build.rs and this test stay in sync.
    let stencil_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("stencils");
    let c_file = stencil_dir.join("load_const.c");
    assert!(c_file.exists(), "load_const.c not found at {}", c_file.display());

    let tmp = std::env::temp_dir().join("rune_path_a_load_const");
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::create_dir_all(&tmp);

    let obj_path = tmp.join("load_const.o");
    let output = Command::new("clang")
        .args(["-O2", "-c", "-ffreestanding", "-target", "arm64-apple-macos", "-o"])
        .arg(&obj_path)
        .arg(&c_file)
        .output()
        .expect("failed to execute clang");
    assert!(output.status.success(),
        "clang failed:\n{}", String::from_utf8_lossy(&output.stderr));

    let obj = std::fs::read(&obj_path).unwrap();
    let section = extract_text_section(&obj)
        .expect("load_const: no __TEXT,__text section");
    eprintln!("load_const raw section ({} bytes): {:02x?}", section.len(), section);

    // Expected: MOVZ W0, #0xDEAD (4 bytes) + B _rune_push (4 bytes) = 8 bytes
    assert!(section.len() >= 8, "load_const too small: {}", section.len());

    // Verify MOVZ W0 with 0xDEAD placeholder
    let mov_instr = u32::from_le_bytes(section[0..4].try_into().unwrap());
    const MOV_W0_MASK: u32 = 0xFFE0001Fu32;
    let mov_base = mov_instr & MOV_W0_MASK;
    assert_eq!(mov_base, 0x52800000u32,
        "load_const[0]: expected MOVZ W0 pattern {:#010x}, got {:#010x}, full: {:#010x}",
        0x52800000u32, mov_base, mov_instr);
    let placeholder = (mov_instr >> 5) & 0xFFFF;
    assert_eq!(placeholder, 0xDEAD,
        "load_const: expected placeholder 0xDEAD, got {:#06x}", placeholder);

    // Verify B instruction at offset 4
    let branch_instr = u32::from_le_bytes(section[4..8].try_into().unwrap());
    let is_b = (branch_instr >> 31) == 0;
    assert!(is_b, "load_const[4]: expected B (tail call), got {:#010x}", branch_instr);

    // Verify branch relocation at offset 4
    let (reloc_addr, _) = find_branch_reloc(&obj)
        .expect("load_const: no branch relocation found");
    assert_eq!(reloc_addr, 4, "load_const: expected branch reloc at offset 4, got {reloc_addr}");

    eprintln!("load_const: MOVZ W0, #0xDEAD ; B _rune_push — encoding verified ✓");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_path_a_load_local_encoding() {
    let stencil_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("stencils");
    let c_file = stencil_dir.join("load_local.c");
    assert!(c_file.exists(), "load_local.c not found");

    let tmp = std::env::temp_dir().join("rune_path_a_load_local");
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::create_dir_all(&tmp);

    let obj_path = tmp.join("load_local.o");
    let output = Command::new("clang")
        .args(["-O2", "-c", "-ffreestanding", "-target", "arm64-apple-macos", "-o"])
        .arg(&obj_path).arg(&c_file)
        .output().expect("clang failed");
    assert!(output.status.success());
    let obj = std::fs::read(&obj_path).unwrap();
    let section = extract_text_section(&obj).expect("no __TEXT,__text");
    assert!(section.len() >= 8, "load_local too small: {}", section.len());

    let mov_instr = u32::from_le_bytes(section[0..4].try_into().unwrap());
    assert_eq!(mov_instr & 0xFFE0001Fu32, 0x52800000u32,
        "load_local[0]: expected MOVZ W0, got {:#010x}", mov_instr);
    assert_eq!((mov_instr >> 5) & 0xFFFF, 0xDEAD,
        "load_local: expected placeholder 0xDEAD");

    let branch_instr = u32::from_le_bytes(section[4..8].try_into().unwrap());
    assert!((branch_instr >> 31) == 0, "load_local[4]: expected B, got {:#010x}", branch_instr);

    let (reloc_addr, _) = find_branch_reloc(&obj).expect("load_local: no branch reloc");
    assert_eq!(reloc_addr, 4, "load_local: expected reloc at offset 4");

    eprintln!("load_local: MOVZ W0, #0xDEAD ; B _rune_load_local ✓");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_path_a_store_local_encoding() {
    let stencil_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("stencils");
    let c_file = stencil_dir.join("store_local.c");
    assert!(c_file.exists(), "store_local.c not found");

    let tmp = std::env::temp_dir().join("rune_path_a_store_local");
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::create_dir_all(&tmp);

    let obj_path = tmp.join("store_local.o");
    let output = Command::new("clang")
        .args(["-O2", "-c", "-ffreestanding", "-target", "arm64-apple-macos", "-o"])
        .arg(&obj_path).arg(&c_file)
        .output().expect("clang failed");
    assert!(output.status.success());
    let obj = std::fs::read(&obj_path).unwrap();
    let section = extract_text_section(&obj).expect("no __TEXT,__text");
    assert!(section.len() >= 8, "store_local too small: {}", section.len());

    let mov_instr = u32::from_le_bytes(section[0..4].try_into().unwrap());
    assert_eq!(mov_instr & 0xFFE0001Fu32, 0x52800000u32,
        "store_local[0]: expected MOVZ W0, got {:#010x}", mov_instr);
    assert_eq!((mov_instr >> 5) & 0xFFFF, 0xDEAD,
        "store_local: expected placeholder 0xDEAD");

    let branch_instr = u32::from_le_bytes(section[4..8].try_into().unwrap());
    assert!((branch_instr >> 31) == 0, "store_local[4]: expected B, got {:#010x}", branch_instr);

    let (reloc_addr, _) = find_branch_reloc(&obj).expect("store_local: no branch reloc");
    assert_eq!(reloc_addr, 4, "store_local: expected reloc at offset 4");

    eprintln!("store_local: MOVZ W0, #0xDEAD ; B _rune_store_local ✓");

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_path_a_helper_determinism() {
     // Verify the helper compiles deterministically.
     let tmp = std::env::temp_dir().join("rune_path_a_proto3");
     let _ = std::fs::remove_dir_all(&tmp);
     let _ = std::fs::create_dir_all(&tmp);

     let helper_c = r#"
#include <stdint.h>
void rune_push(int64_t val) {
    __asm__(
        "str %[val], [x22]\n\t"
        "add x22, x22, #8"
        : : [val] "r" (val) : "x22", "memory"
    );
}
"#;
     let o1 = compile_c(helper_c, &tmp.join("h1.o"));
     let o2 = compile_c(helper_c, &tmp.join("h2.o"));
     let b1 = extract_text_section(&o1).unwrap();
     let b2 = extract_text_section(&o2).unwrap();
     assert_eq!(b1, b2, "helper: non-deterministic output");

     let _ = std::fs::remove_dir_all(&tmp);
 }
