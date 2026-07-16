use vmi_arch_api::{AddressTranslator, Translation};
use vmi_driver_api::{read_scalar, MemoryAccess};
use vmi_types::{ByteOrder, Gpa, Gva, Result, TranslationRoot, VmiError};

const PRESENT: u64 = 1;
const LARGE: u64 = 1 << 7;
const ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;

fn is_canonical(address: u64, bits: u8) -> bool {
    let Some(low_mask) = 1u64
        .checked_shl(u32::from(bits))
        .and_then(|value| value.checked_sub(1))
    else {
        return false;
    };
    let Some(sign_shift) = bits.checked_sub(1) else {
        return false;
    };
    let Some(sign_bit) = 1u64.checked_shl(u32::from(sign_shift)) else {
        return false;
    };
    let expected_upper = if address & sign_bit == 0 {
        0
    } else {
        !low_mask
    };
    address & !low_mask == expected_upper
}

#[derive(Copy, Clone, Debug, Default)]
pub struct Amd64Translator;

#[derive(Copy, Clone, Debug, Default)]
pub struct Amd64La57Translator;

fn translate_levels(
    memory: &dyn MemoryAccess,
    root: TranslationRoot,
    address: Gva,
    address_bits: u8,
    shifts: &[u8],
) -> Result<Translation> {
    let va = address.raw();
    if !is_canonical(va, address_bits) {
        return Err(VmiError::NonCanonicalAddress {
            address: va,
            bits: address_bits,
        });
    }
    let mut table = root.raw() & ADDRESS_MASK;
    for (position, shift) in shifts.iter().copied().enumerate() {
        let remaining = shifts
            .len()
            .checked_sub(position)
            .ok_or_else(|| VmiError::Backend("AMD64 translation level underflow".into()))?;
        let level = u8::try_from(remaining)
            .map_err(|_| VmiError::Backend("AMD64 translation level overflow".into()))?;
        let index = (va >> shift) & 0x1ff;
        let entry_offset = index
            .checked_mul(8)
            .ok_or_else(|| VmiError::Backend("AMD64 page-table entry offset overflow".into()))?;
        let entry_address = table
            .checked_add(entry_offset)
            .ok_or_else(|| VmiError::Backend("AMD64 page-table entry address overflow".into()))?;
        let entry: u64 = read_scalar(memory, Gpa::new(entry_address), ByteOrder::LittleEndian)?;
        if entry & PRESENT == 0 {
            return Err(VmiError::PageNotPresent { address: va, level });
        }
        if entry & LARGE != 0 && level > 3 {
            return Err(VmiError::InvalidPageTableEntry { entry, level });
        }
        if level == 3 && entry & LARGE != 0 {
            return Ok(Translation::new(
                Gpa::new((entry & 0x000f_ffff_c000_0000) | (va & 0x3fff_ffff)),
                1 << 30,
            ));
        }
        if level == 2 && entry & LARGE != 0 {
            return Ok(Translation::new(
                Gpa::new((entry & 0x000f_ffff_ffe0_0000) | (va & 0x1f_ffff)),
                1 << 21,
            ));
        }
        if level == 1 {
            if entry & LARGE != 0 {
                return Err(VmiError::InvalidPageTableEntry { entry, level });
            }
            return Ok(Translation::new(
                Gpa::new((entry & ADDRESS_MASK) | (va & 0xfff)),
                1 << 12,
            ));
        }
        table = entry & ADDRESS_MASK;
    }
    Err(VmiError::Backend(
        "AMD64 translation level list is empty".into(),
    ))
}

impl AddressTranslator for Amd64Translator {
    fn cache_tag(&self) -> u64 {
        0x414d_4436_345f_3438
    }

    fn translate(
        &self,
        memory: &dyn MemoryAccess,
        root: TranslationRoot,
        address: Gva,
    ) -> Result<Translation> {
        translate_levels(memory, root, address, 48, &[39, 30, 21, 12])
    }
}

impl AddressTranslator for Amd64La57Translator {
    fn cache_tag(&self) -> u64 {
        0x414d_4436_345f_3537
    }

    fn translate(
        &self,
        memory: &dyn MemoryAccess,
        root: TranslationRoot,
        address: Gva,
    ) -> Result<Translation> {
        translate_levels(memory, root, address, 57, &[48, 39, 30, 21, 12])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    struct Memory(BTreeMap<u64, u8>);
    impl Memory {
        fn new(entries: &[(u64, u64)]) -> Self {
            let mut bytes = BTreeMap::new();
            for (address, value) in entries {
                for (offset, byte) in value.to_le_bytes().into_iter().enumerate() {
                    bytes.insert(address + offset as u64, byte);
                }
            }
            Self(bytes)
        }
    }
    impl MemoryAccess for Memory {
        fn read_into(&self, address: Gpa, buffer: &mut [u8]) -> Result<()> {
            let length = buffer.len();
            for (offset, byte) in buffer.iter_mut().enumerate() {
                *byte =
                    *self
                        .0
                        .get(&(address.raw() + offset as u64))
                        .ok_or(VmiError::ReadFailed {
                            address: address.raw(),
                            length,
                        })?;
            }
            Ok(())
        }
    }

    #[test]
    fn empty_translation_level_list_fails_closed() {
        assert!(translate_levels(
            &Memory::new(&[]),
            TranslationRoot::new(0),
            Gva::new(0),
            48,
            &[]
        )
        .is_err());
    }

    #[test]
    fn canonicality_helper_rejects_invalid_widths() {
        assert!(!is_canonical(0, 0));
        assert!(!is_canonical(0, 64));
        assert!(!is_canonical(0, u8::MAX));
    }

    #[test]
    fn translates_4k_page() {
        let va = 0x0000_7f12_3456_7788u64;
        let i = [
            (va >> 39) & 0x1ff,
            (va >> 30) & 0x1ff,
            (va >> 21) & 0x1ff,
            (va >> 12) & 0x1ff,
        ];
        let memory = Memory::new(&[
            (0x1000 + i[0] * 8, 0x2001),
            (0x2000 + i[1] * 8, 0x3001),
            (0x3000 + i[2] * 8, 0x4001),
            (0x4000 + i[3] * 8, 0x1234_5001),
        ]);
        assert_eq!(
            Amd64Translator
                .translate(&memory, TranslationRoot::new(0x1000), Gva::new(va))
                .unwrap(),
            Translation::new(Gpa::new(0x1234_5788), 4096)
        );
    }

    #[test]
    fn translates_1g_page_and_rejects_noncanonical() {
        let memory = Memory::new(&[(0x1000, 0x2001), (0x2008, 0x8000_0081)]);
        assert_eq!(
            Amd64Translator
                .translate(&memory, TranslationRoot::new(0x1000), Gva::new(0x4000_1234))
                .unwrap(),
            Translation::new(Gpa::new(0x8000_1234), 1 << 30)
        );
        assert!(matches!(
            Amd64Translator.translate(
                &memory,
                TranslationRoot::new(0x1000),
                Gva::new(0x0001_0000_0000_0000)
            ),
            Err(VmiError::NonCanonicalAddress { .. })
        ));
    }

    #[test]
    fn translates_2m_page() {
        let va = 0x0000_0000_0065_4321u64;
        let memory = Memory::new(&[
            (0x1000, 0x2001),
            (0x2000, 0x3001),
            (0x3000 + (((va >> 21) & 0x1ff) * 8), 0x0200_0081),
        ]);
        assert_eq!(
            Amd64Translator
                .translate(&memory, TranslationRoot::new(0x1000), Gva::new(va))
                .unwrap(),
            Translation::new(Gpa::new(0x0205_4321), 1 << 21)
        );
    }

    #[test]
    fn translates_la57_page_and_enforces_57_bit_canonicality() {
        let va = 0x0001_1234_5678_9abcu64;
        let shifts = [48, 39, 30, 21, 12];
        let indices: Vec<_> = shifts
            .into_iter()
            .map(|shift| (va >> shift) & 0x1ff)
            .collect();
        let memory = Memory::new(&[
            (0x1000 + indices[0] * 8, 0x2001),
            (0x2000 + indices[1] * 8, 0x3001),
            (0x3000 + indices[2] * 8, 0x4001),
            (0x4000 + indices[3] * 8, 0x5001),
            (0x5000 + indices[4] * 8, 0x1234_5001),
        ]);
        assert_eq!(
            Amd64La57Translator
                .translate(&memory, TranslationRoot::new(0x1000), Gva::new(va))
                .unwrap(),
            Translation::new(Gpa::new(0x1234_5abc), 4096)
        );
        assert!(matches!(
            Amd64La57Translator.translate(
                &memory,
                TranslationRoot::new(0x1000),
                Gva::new(0x0200_0000_0000_0000)
            ),
            Err(VmiError::NonCanonicalAddress { bits: 57, .. })
        ));
    }

    proptest! {
        #[test]
        fn translates_arbitrary_canonical_4k_offsets(
            page_index in 0u64..(1u64 << 35),
            offset in 0u64..4096,
            physical_page in 0u64..(1u64 << 40),
        ) {
            let va = (page_index << 12) | offset;
            let physical_page = physical_page & ADDRESS_MASK;
            let indices = [
                (va >> 39) & 0x1ff,
                (va >> 30) & 0x1ff,
                (va >> 21) & 0x1ff,
                (va >> 12) & 0x1ff,
            ];
            let memory = Memory::new(&[
                (0x1000 + indices[0] * 8, 0x2001),
                (0x2000 + indices[1] * 8, 0x3001),
                (0x3000 + indices[2] * 8, 0x4001),
                (0x4000 + indices[3] * 8, physical_page | PRESENT),
            ]);

            let translation = Amd64Translator
                .translate(&memory, TranslationRoot::new(0x1000), Gva::new(va))
                .unwrap();
            prop_assert_eq!(translation.physical_address, Gpa::new(physical_page | offset));
            prop_assert_eq!(translation.page_size, 4096);
        }

        #[test]
        fn rejects_every_noncanonical_48_bit_address(
            low in 0u64..(1u64 << 47),
            high in 1u64..(1u64 << 16),
        ) {
            let address = (high << 48) | low;
            let rejected = matches!(
                Amd64Translator.translate(
                    &Memory::new(&[]),
                    TranslationRoot::new(0),
                    Gva::new(address),
                ),
                Err(VmiError::NonCanonicalAddress { bits: 48, .. })
            );
            prop_assert!(rejected);
        }
    }
}
