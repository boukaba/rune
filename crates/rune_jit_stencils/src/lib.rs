//! Copy-and-Patch stencil library for Rune's baseline JIT.
//!
//! This crate provides pre-compiled machine-code stencils (generated at build
//! time by `build.rs`) and a `StencilPatcher` that emits them into JIT code
//! buffers by memcpy + patching holes.

pub mod patcher;

include!(concat!(env!("OUT_DIR"), "/stencils.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stencil_sizes() {
        for s in ALL_STENCILS {
            assert!(!s.bytes.is_empty(), "stencil {} has empty bytes", s.name);
            assert!(s.bytes.len() % 4 == 0, "stencil {} bytes not aligned to 4: {}",
                    s.name, s.bytes.len());
        }
    }

    #[test]
    fn test_stencil_holes_in_bounds() {
        for s in ALL_STENCILS {
            for h in s.holes {
                assert!(
                    h.byte_offset < s.bytes.len(),
                    "stencil {} hole offset {} out of bounds (len {})",
                    s.name, h.byte_offset, s.bytes.len()
                );
                assert!(
                    h.bit_offset + h.bit_width <= 32,
                    "stencil {} hole at offset {} bits {}+{} exceeds 32",
                    s.name, h.byte_offset, h.bit_offset, h.bit_width
                );
            }
        }
    }

    #[test]
    fn test_patch_load_smi_16() {
        let expected_smi = (42u64 << 1) | 1;
        let mut buf = LOAD_SMI_16_BYTES.to_vec();
        patcher::patch_stencil(&mut buf, LOAD_SMI_16_HOLES, &[expected_smi]);

        // Real C stencil compiles to MOVZ W0 (32-bit, sf=0): 0x52800000 | (imm << 5)
        let expected_movz: u32 = 0x52800000 | (0x55_u32 << 5);
        let actual_movz = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        assert_eq!(actual_movz, expected_movz,
            "load_smi_16 patched with 42: expected {:#010x}, got {:#010x}",
            expected_movz, actual_movz);
    }

    #[test]
    fn test_patch_load_smi_32() {
        let value: u64 = 0xDEAD_BEEF;
        let smi_val = (value << 1) | 1;
        let mut buf = LOAD_SMI_32_BYTES.to_vec();
        let lower16 = smi_val & 0xFFFF;
        let upper16 = (smi_val >> 16) & 0xFFFF;
        patcher::patch_stencil(&mut buf, LOAD_SMI_32_HOLES, &[lower16, upper16]);

        // Real C stencil: MOVZ W0 (32-bit, sf=0): 0x52800000 | (imm << 5)
        let expected_movz_low: u32 = 0x52800000 | ((smi_val as u32 & 0xFFFF) << 5);
        let actual_instr0 = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        assert_eq!(actual_instr0, expected_movz_low,
            "load_smi_32 first MOVZ: expected {:#010x}, got {:#010x}",
            expected_movz_low, actual_instr0);

        // MOVK W0 (32-bit, sf=0, hw=1): 0x72A00000 | (imm << 5)
        let expected_movk_high: u32 = 0x72A00000 | ((smi_val as u32 >> 16) << 5);
        let actual_instr1 = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        assert_eq!(actual_instr1, expected_movk_high,
            "load_smi_32 MOVK upper: expected {:#010x}, got {:#010x}",
            expected_movk_high, actual_instr1);
    }

    #[test]
    fn test_link_holes_defined() {
        for s in ALL_STENCILS {
            for lh in s.link_holes {
                assert!(
                    lh.byte_offset < s.bytes.len(),
                    "stencil {} link hole offset {} out of bounds (len {})",
                    s.name, lh.byte_offset, s.bytes.len()
                );
                // Verify the referenced helper exists
                let helper_found = ALL_HELPERS.iter().any(|h| h.name == lh.helper_name);
                assert!(helper_found,
                    "stencil {} link hole references unknown helper '{}'",
                    s.name, lh.helper_name);
            }
        }
    }
}
