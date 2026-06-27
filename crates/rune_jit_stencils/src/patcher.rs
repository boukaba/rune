//! Runtime stencil patcher — memcpy + patch holes.
//!
//! The patcher takes pre-compiled stencil byte sequences and writes them into
//! a JIT code buffer, patching the holes with runtime-determined values.

use crate::HoleDef;

/// Write a value into a bit field of a 32-bit word at the given byte offset.
/// The word is read as little-endian u32, patched, and written back.
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

/// Patch a single stencil by copying `stencil_bytes` into `buf` at `offset`,
/// then applying all holes.
///
/// # Panics
///
/// Panics if `buf` is too small, if hole offsets are out of bounds, or if
/// a hole value doesn't fit in the specified bit width.
pub fn patch_stencil_into(
    buf: &mut [u8],
    offset: usize,
    stencil_bytes: &[u8],
    holes: &[HoleDef],
    hole_values: &[u64],
) {
    assert_eq!(holes.len(), hole_values.len(),
        "stencil patching: {} holes but {} values", holes.len(), hole_values.len());

    // Copy stencil bytes into the JIT buffer.
    let dest = &mut buf[offset..offset + stencil_bytes.len()];
    dest.copy_from_slice(stencil_bytes);

    // Patch each hole.
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

/// A pre-loaded stencil ready for emission.
pub struct Stencil {
    pub bytes: &'static [u8],
    pub holes: &'static [HoleDef],
}

impl Stencil {
    pub const fn new(bytes: &'static [u8], holes: &'static [HoleDef]) -> Self {
        Self { bytes, holes }
    }

    /// Emit this stencil into the JIT code buffer at the given offset,
    /// patching holes with the provided values.
    pub fn emit(&self, buf: &mut [u8], offset: usize, hole_values: &[u64]) {
        patch_stencil_into(buf, offset, self.bytes, self.holes, hole_values);
    }
}

/// High-level patcher that owns a JIT code buffer and emits stencils.
pub struct StencilPatcher<'a> {
    pub buf: &'a mut [u8],
    pub offset: usize,
}

impl<'a> StencilPatcher<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, offset: 0 }
    }

    /// Emit a stencil, patching its holes, and advance the offset.
    pub fn emit(&mut self, stencil: &Stencil, hole_values: &[u64]) {
        stencil.emit(self.buf, self.offset, hole_values);
        self.offset += stencil.bytes.len();
    }

    /// Write raw bytes (for non-stencil data like inline caches).
    pub fn write_raw(&mut self, data: &[u8]) {
        let dest = &mut self.buf[self.offset..self.offset + data.len()];
        dest.copy_from_slice(data);
        self.offset += data.len();
    }

    /// Current emit position.
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
        // Patch bits 0..4 with value 0xA
        patch_bits(&mut buf, 0, 0, 4, 0xA);
        let val = u32::from_le_bytes(buf[..4].try_into().unwrap());
        // bits 0-3 of word = 0xA, rest = 0 → word = 0xA
        assert_eq!(val, 0xA, "patching lower nibble with 0xA");
    }

    #[test]
    fn test_patch_bits_middle() {
        let mut buf = vec![0u8; 4];
        // Patch bits 8..12 with value 0x7
        patch_bits(&mut buf, 0, 8, 4, 0x7);
        let val = u32::from_le_bytes(buf[..4].try_into().unwrap());
        // bits 8-11 = 0x7, rest = 0 → 0x7 << 8 = 0x700
        assert_eq!(val, 0x700, "patching bits 8-11 with 0x7");
    }

    #[test]
    fn test_patch_bits_preserve_surrounding() {
        let mut buf = vec![0xFFu8; 4];
        // Patch bits 4..12 with 0 (clear middle nibble)
        patch_bits(&mut buf, 0, 4, 8, 0);
        let val = u32::from_le_bytes(buf[..4].try_into().unwrap());
        // bits 0-3 = 0xF, bits 4-11 = 0, bits 12-31 = 0xFFF
        assert_eq!(val, 0xFFFFF00F, "clearing middle 8 bits preserves outer ones");
    }

    #[test]
    fn test_emit_load_smi() {
        use crate::LOAD_SMI_16_BYTES;
        use crate::LOAD_SMI_16_HOLES;

        let stencil = Stencil::new(LOAD_SMI_16_BYTES, LOAD_SMI_16_HOLES);
        let mut buf = vec![0u8; 16];
        let mut patcher = StencilPatcher::new(&mut buf);
        // Emit load_smi with immediate 42
        let smi_val = (42u64 << 1) | 1;
        patcher.emit(&stencil, &[smi_val]);

        let expected_movz: u32 = 0xD2800000 | ((0x55 as u32) << 5);
        let actual_movz = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        assert_eq!(actual_movz, expected_movz, "MOVZ encoding");
    }
}
