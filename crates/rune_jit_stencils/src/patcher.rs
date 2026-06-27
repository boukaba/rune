//! Runtime stencil patcher — memcpy + patch holes + resolve link holes.
//!
//! The patcher takes pre-compiled stencil byte sequences and writes them into
//! a JIT code buffer, patching the holes with runtime-determined values.
//! Link holes (B/BL relocations to runtime helpers) are resolved based on the
//! emitted position of each helper in the JIT buffer.

use std::collections::HashMap;
use crate::{HoleDef, StencilDef, HelperDef};

/// Write a value into a bit field of a 32-bit word at the given byte offset.
fn patch_bits(buf: &mut [u8], byte_offset: usize, bit_offset: u8, bit_width: u8, value: u64) {
    let range = &mut buf[byte_offset..byte_offset + 4];
    let word = <&mut [u8; 4]>::try_from(range)
        .expect("hole byte_offset out of bounds");
    let mut val = u32::from_le_bytes(*word);
    let mask = (1u32 << bit_width) - 1;
    val &= !(mask << bit_offset);
    val |= ((value as u32) & mask) << bit_offset;
    *word = val.to_le_bytes();
}

/// Patch a B/BL branch instruction at `byte_offset` with the displacement
/// (in bytes) from branch PC to target.
///
/// The 26-bit signed offset is in instruction units (divided by 4).
fn patch_link(buf: &mut [u8], byte_offset: usize, link_offset: i64) {
    let offset_in_instrs = (link_offset / 4) as i32;
    assert!((-1 << 25..(1 << 25)).contains(&offset_in_instrs),
        "branch offset {} out of 26-bit signed range at byte_offset {}", offset_in_instrs, byte_offset);

    let range = &mut buf[byte_offset..byte_offset + 4];
    let word = <&mut [u8; 4]>::try_from(range)
        .expect("link hole byte_offset out of bounds");
    let mut val = u32::from_le_bytes(*word);
    // Bits 25:0 = branch offset (26-bit signed, in instruction units)
    val &= !0x03FFFFFFu32;
    val |= (offset_in_instrs as u32) & 0x03FFFFFF;
    *word = val.to_le_bytes();
}

/// Patch a single stencil by copying `stencil_bytes` into `buf` at `offset`,
/// then applying all value holes.
pub fn patch_stencil_into(
    buf: &mut [u8],
    offset: usize,
    stencil_bytes: &[u8],
    holes: &[HoleDef],
    hole_values: &[u64],
) {
    assert_eq!(holes.len(), hole_values.len(),
        "stencil patching: {} holes but {} values", holes.len(), hole_values.len());

    let dest = &mut buf[offset..offset + stencil_bytes.len()];
    dest.copy_from_slice(stencil_bytes);

    for (hole, value) in holes.iter().zip(hole_values.iter()) {
        patch_bits(buf, offset + hole.byte_offset, hole.bit_offset, hole.bit_width, *value);
    }
}

/// Patch a stencil in-place (modifies the byte slice directly).
pub fn patch_stencil(buf: &mut [u8], holes: &[HoleDef], hole_values: &[u64]) {
    for (hole, value) in holes.iter().zip(hole_values.iter()) {
        patch_bits(buf, hole.byte_offset, hole.bit_offset, hole.bit_width, *value);
    }
}

/// A pre-loaded stencil ready for emission (value holes only, no link holes).
pub struct Stencil {
    pub bytes: &'static [u8],
    pub holes: &'static [HoleDef],
}

impl Stencil {
    pub const fn new(bytes: &'static [u8], holes: &'static [HoleDef]) -> Self {
        Self { bytes, holes }
    }

    pub fn emit(&self, buf: &mut [u8], offset: usize, hole_values: &[u64]) {
        patch_stencil_into(buf, offset, self.bytes, self.holes, hole_values);
    }
}

/// High-level patcher that owns a JIT code buffer and emits stencils.
///
/// Usage:
/// 1. Call `emit_helper()` for each runtime helper (records their positions).
/// 2. Call `emit_stencil()` for each stencil (resolves link holes automatically).
pub struct StencilPatcher<'a> {
    pub buf: &'a mut [u8],
    pub offset: usize,
    helper_offsets: HashMap<&'static str, usize>,
}

impl<'a> StencilPatcher<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, offset: 0, helper_offsets: HashMap::new() }
    }

    /// Emit a helper's body bytes and record its position for link resolution.
    pub fn emit_helper(&mut self, helper: &HelperDef) {
        self.helper_offsets.insert(helper.name, self.offset);
        self.write_raw(helper.bytes);
    }

    /// Emit a stencil: copy bytes, patch value holes, resolve link holes.
    pub fn emit_stencil(&mut self, stencil: &StencilDef, hole_values: &[u64]) {
        // Copy stencil bytes into the JIT buffer.
        let stencil_start = self.offset;
        self.write_raw(stencil.bytes);

        // Patch value holes (immediates in MOVZ/MOVK).
        patch_stencil(&mut self.buf[stencil_start..stencil_start + stencil.bytes.len()],
                      stencil.holes, hole_values);

        // Patch link holes (B/BL relocations to helpers).
        for link in stencil.link_holes {
            let helper_offset = *self.helper_offsets.get(link.helper_name)
                .unwrap_or_else(|| panic!("stencil '{}' references helper '{}' which was not emitted",
                         stencil.name, link.helper_name));
            let branch_pc = stencil_start + link.byte_offset;
            let displacement = helper_offset as i64 - branch_pc as i64;
            patch_link(self.buf, branch_pc, displacement);
        }
    }

    /// Write raw bytes (for non-stencil data like inline caches).
    pub fn write_raw(&mut self, data: &[u8]) {
        let dest = &mut self.buf[self.offset..self.offset + data.len()];
        dest.copy_from_slice(data);
        self.offset += data.len();
    }

    /// Emit a value-only stencil (no link holes), patching its holes.
    pub fn emit(&mut self, stencil: &Stencil, hole_values: &[u64]) {
        stencil.emit(self.buf, self.offset, hole_values);
        self.offset += stencil.bytes.len();
    }

    pub fn current_offset(&self) -> usize {
        self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_bits_lsb() {
        let mut buf = vec![0u8; 4];
        patch_bits(&mut buf, 0, 0, 4, 0xA);
        let val = u32::from_le_bytes(buf[..4].try_into().unwrap());
        assert_eq!(val, 0xA, "patching lower nibble with 0xA");
    }

    #[test]
    fn test_patch_bits_middle() {
        let mut buf = vec![0u8; 4];
        patch_bits(&mut buf, 0, 8, 4, 0x7);
        let val = u32::from_le_bytes(buf[..4].try_into().unwrap());
        assert_eq!(val, 0x700, "patching bits 8-11 with 0x7");
    }

    #[test]
    fn test_patch_bits_preserve_surrounding() {
        let mut buf = vec![0xFFu8; 4];
        patch_bits(&mut buf, 0, 4, 8, 0);
        let val = u32::from_le_bytes(buf[..4].try_into().unwrap());
        assert_eq!(val, 0xFFFFF00F, "clearing middle 8 bits preserves outer ones");
    }

    #[test]
    fn test_patch_link_basic() {
        // B instruction with all zeros: 0x14000000 = B #0
        let mut buf = vec![0u8; 4];
        buf[..4].copy_from_slice(&0x14000000u32.to_le_bytes());
        // Target is 8 instructions ahead (32 bytes) → offset = 8
        patch_link(&mut buf, 0, 32);
        let val = u32::from_le_bytes(buf[..4].try_into().unwrap());
        // 0x14000000 | 8 = 0x14000008
        assert_eq!(val, 0x14000008u32, "B #8 encoding");
    }

    #[test]
    fn test_patch_link_negative() {
        let mut buf = vec![0u8; 4];
        buf[..4].copy_from_slice(&0x94000000u32.to_le_bytes()); // BL #0
        // Target is 4 instructions back (-16 bytes) → offset = -4
        patch_link(&mut buf, 0, -16);
        let val = u32::from_le_bytes(buf[..4].try_into().unwrap());
        // 0x94000000 | ((-4 as u32) & 0x3FFFFFF) = 0x93FFFFFC
        let expected = 0x94000000u32 | ((-4i32 as u32) & 0x03FFFFFF);
        assert_eq!(val, expected, "BL #-4 encoding");
    }

    #[test]
    fn test_emit_load_smi_no_link() {
        // For stencils without link holes (e.g., push_reg, pop_reg), the old
        // Stencil::emit path should still work.
        use crate::PUSH_REG_BYTES;
        use crate::PUSH_REG_HOLES;

        let stencil = Stencil::new(PUSH_REG_BYTES, PUSH_REG_HOLES);
        let mut buf = vec![0u8; 16];
        let mut patcher = StencilPatcher::new(&mut buf);
        patcher.emit(&stencil, &[]);

        // PUSH_REG is naked-asm: STR x0,[x22]; ADD x22,x22,#8; RET
        let str_instr = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        assert_eq!(str_instr, 0xF90002C0u32, "STR x0,[x22]");
    }

    #[test]
    fn test_emit_real_c_stencil_with_link_hole() {
        use crate::{RUNE_PUSH_HELPER, LOAD_SMI_16_BYTES, LOAD_SMI_16_HOLES, LOAD_SMI_16_LINK_HOLES};

        let stencil = StencilDef {
            name: "load_smi_16",
            bytes: LOAD_SMI_16_BYTES,
            holes: LOAD_SMI_16_HOLES,
            link_holes: LOAD_SMI_16_LINK_HOLES,
        };

        let mut buf = vec![0u8; 64];
        let mut patcher = StencilPatcher::new(&mut buf);

        // Emit helper first
        let helper_offset = patcher.current_offset();
        patcher.emit_helper(&RUNE_PUSH_HELPER);
        let helper_used = patcher.current_offset() - helper_offset;
        assert!(helper_used > 0, "helper bytes emitted");

        // Emit stencil with link hole
        let stencil_offset = patcher.current_offset();
        let smi_val = (42u64 << 1) | 1;
        patcher.emit_stencil(&stencil, &[smi_val]);

        // Verify value hole: MOVZ W0, #42 (smi = 85 = 0x55)
        let expected_movz: u32 = 0x52800000 | (0x55u32 << 5);
        let actual_movz = u32::from_le_bytes(buf[stencil_offset..stencil_offset+4].try_into().unwrap());
        assert_eq!(actual_movz, expected_movz,
            "MOVZ encoding: expected {:#010x}, got {:#010x}", expected_movz, actual_movz);

        // Verify link hole: B instruction targets helper
        let branch_instr = u32::from_le_bytes(buf[stencil_offset+4..stencil_offset+8].try_into().unwrap());
        let actual_offset = (branch_instr & 0x03FFFFFF) as i32;
        let actual_offset_se = if actual_offset & (1 << 25) != 0 {
            actual_offset | !0x3FFFFFFi32
        } else {
            actual_offset
        };
        let expected_disp = (helper_offset as i64 - (stencil_offset + 4) as i64) / 4;
        assert_eq!(actual_offset_se as i64, expected_disp,
            "B offset: expected {}, got {}", expected_disp, actual_offset_se);
    }
}
