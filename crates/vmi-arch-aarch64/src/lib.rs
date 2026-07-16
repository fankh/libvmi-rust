use vmi_arch_api::{AddressTranslator, Translation};
use vmi_driver_api::{read_scalar, MemoryAccess};
use vmi_types::{ByteOrder, Gpa, Gva, Result, TranslationRoot, VmiError};

const VALID: u64 = 1;
const TABLE_OR_PAGE: u64 = 1 << 1;
const PHYSICAL_ADDRESS_MASK: u64 = 0x0000_ffff_ffff_ffff;

fn is_canonical(address: u64, bits: u8) -> bool {
    let Some(low_mask) = low_mask(bits) else {
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

fn low_mask(bits: u8) -> Option<u64> {
    1u64.checked_shl(u32::from(bits))
        .and_then(|value| value.checked_sub(1))
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TranslationGranule {
    KiB4,
    KiB16,
    KiB64,
}

impl TranslationGranule {
    const fn page_shift(self) -> u8 {
        match self {
            Self::KiB4 => 12,
            Self::KiB16 => 14,
            Self::KiB64 => 16,
        }
    }

    const fn index_bits(self) -> u8 {
        match self {
            Self::KiB4 => 9,
            Self::KiB16 => 11,
            Self::KiB64 => 13,
        }
    }

    const fn maximum_levels(self) -> u8 {
        match self {
            Self::KiB4 | Self::KiB16 => 4,
            Self::KiB64 => 3,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Aarch64Translator {
    granule: TranslationGranule,
    address_bits: u8,
}

impl Default for Aarch64Translator {
    fn default() -> Self {
        Self {
            granule: TranslationGranule::KiB4,
            address_bits: 48,
        }
    }
}

impl Aarch64Translator {
    pub fn new(granule: TranslationGranule, address_bits: u8) -> Result<Self> {
        let page_shift = granule.page_shift();
        let maximum_bits = granule
            .index_bits()
            .checked_mul(granule.maximum_levels())
            .and_then(|bits| page_shift.checked_add(bits))
            .ok_or_else(|| VmiError::Backend("AArch64 maximum address size overflow".into()))?;
        if address_bits <= page_shift || address_bits > maximum_bits || address_bits > 48 {
            return Err(VmiError::Backend(format!(
                "invalid AArch64 address size {address_bits} for {granule:?} granule"
            )));
        }
        Ok(Self {
            granule,
            address_bits,
        })
    }

    pub const fn granule(self) -> TranslationGranule {
        self.granule
    }

    pub const fn address_bits(self) -> u8 {
        self.address_bits
    }
}

impl AddressTranslator for Aarch64Translator {
    fn cache_tag(&self) -> u64 {
        let granule = match self.granule {
            TranslationGranule::KiB4 => 4,
            TranslationGranule::KiB16 => 16,
            TranslationGranule::KiB64 => 64,
        };
        0x4136_3400_0000_0000 | (granule << 8) | u64::from(self.address_bits)
    }

    fn translate(
        &self,
        memory: &dyn MemoryAccess,
        root: TranslationRoot,
        address: Gva,
    ) -> Result<Translation> {
        let va = address.raw();
        if !is_canonical(va, self.address_bits) {
            return Err(VmiError::NonCanonicalAddress {
                address: va,
                bits: self.address_bits,
            });
        }

        let page_shift = self.granule.page_shift();
        let index_bits = self.granule.index_bits();
        let translated_bits = self
            .address_bits
            .checked_sub(page_shift)
            .ok_or_else(|| VmiError::Backend("AArch64 address size underflow".into()))?;
        let level_count = translated_bits.div_ceil(index_bits);
        let first_level = 4u8
            .checked_sub(level_count)
            .ok_or_else(|| VmiError::Backend("AArch64 translation level underflow".into()))?;
        let index_mask = low_mask(index_bits)
            .ok_or_else(|| VmiError::Backend("AArch64 index mask overflow".into()))?;
        let page_mask = low_mask(page_shift)
            .ok_or_else(|| VmiError::Backend("AArch64 page mask overflow".into()))?;
        let mut table = root.raw() & PHYSICAL_ADDRESS_MASK & !page_mask;

        for ordinal in 0..level_count {
            let level = first_level
                .checked_add(ordinal)
                .ok_or_else(|| VmiError::Backend("AArch64 translation level overflow".into()))?;
            let remaining_levels = level_count
                .checked_sub(ordinal)
                .and_then(|remaining| remaining.checked_sub(1))
                .ok_or_else(|| VmiError::Backend("AArch64 remaining level underflow".into()))?;
            let entry_shift = remaining_levels
                .checked_mul(index_bits)
                .and_then(|shift| page_shift.checked_add(shift))
                .ok_or_else(|| VmiError::Backend("AArch64 entry shift overflow".into()))?;
            let index = (va >> entry_shift) & index_mask;
            let entry_offset = index.checked_mul(8).ok_or_else(|| {
                VmiError::Backend("AArch64 page-table entry offset overflow".into())
            })?;
            let entry_address = table.checked_add(entry_offset).ok_or_else(|| {
                VmiError::Backend("AArch64 page-table entry address overflow".into())
            })?;
            let entry: u64 = read_scalar(memory, Gpa::new(entry_address), ByteOrder::LittleEndian)?;
            if entry & VALID == 0 {
                return Err(VmiError::PageNotPresent { address: va, level });
            }
            let table_or_page = entry & TABLE_OR_PAGE != 0;
            let last = ordinal.checked_add(1) == Some(level_count);
            if last {
                if !table_or_page {
                    return Err(VmiError::InvalidPageTableEntry { entry, level });
                }
                let page_size = 1u64
                    .checked_shl(u32::from(page_shift))
                    .ok_or_else(|| VmiError::Backend("AArch64 page size overflow".into()))?;
                let page_offset_mask = page_size
                    .checked_sub(1)
                    .ok_or_else(|| VmiError::Backend("AArch64 page mask underflow".into()))?;
                return Ok(Translation::new(
                    Gpa::new(
                        (entry & PHYSICAL_ADDRESS_MASK & !page_offset_mask)
                            | (va & page_offset_mask),
                    ),
                    page_size,
                ));
            }
            if table_or_page {
                table = entry & PHYSICAL_ADDRESS_MASK & !page_mask;
                continue;
            }
            if level == 0 {
                return Err(VmiError::InvalidPageTableEntry { entry, level });
            }
            let block_size = 1u64
                .checked_shl(u32::from(entry_shift))
                .ok_or_else(|| VmiError::Backend("AArch64 block size overflow".into()))?;
            let block_offset_mask = block_size
                .checked_sub(1)
                .ok_or_else(|| VmiError::Backend("AArch64 block mask underflow".into()))?;
            return Ok(Translation::new(
                Gpa::new(
                    (entry & PHYSICAL_ADDRESS_MASK & !block_offset_mask) | (va & block_offset_mask),
                ),
                block_size,
            ));
        }
        Err(VmiError::Backend(
            "AArch64 translation completed without a page or block".into(),
        ))
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
        fn read_into(&self, address: Gpa, output: &mut [u8]) -> Result<()> {
            let length = output.len();
            for (offset, byte) in output.iter_mut().enumerate() {
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

    fn page_fixture(translator: Aarch64Translator, va: u64, output: u64) -> Memory {
        let page_shift = translator.granule.page_shift();
        let index_bits = translator.granule.index_bits();
        let levels = (translator.address_bits - page_shift).div_ceil(index_bits);
        let index_mask = (1u64 << index_bits) - 1;
        let mut entries = Vec::new();
        for ordinal in 0..levels {
            let table = 0x1_0000 + u64::from(ordinal) * (1u64 << page_shift);
            let remaining = levels - ordinal - 1;
            let index = (va >> (page_shift + remaining * index_bits)) & index_mask;
            let value = if ordinal + 1 == levels {
                output | TABLE_OR_PAGE | VALID
            } else {
                (table + (1u64 << page_shift)) | TABLE_OR_PAGE | VALID
            };
            entries.push((table + index * 8, value));
        }
        Memory::new(&entries)
    }

    #[test]
    fn translates_all_supported_granules() {
        for (granule, bits, va, physical, page_size) in [
            (
                TranslationGranule::KiB4,
                48,
                0x1234_5678_9abc,
                0x9000_0000,
                1 << 12,
            ),
            (
                TranslationGranule::KiB16,
                47,
                0x1234_5678_1234,
                0xa000_0000,
                1 << 14,
            ),
            (
                TranslationGranule::KiB64,
                48,
                0x1234_5678_4321,
                0xb000_0000,
                1 << 16,
            ),
        ] {
            let translator = Aarch64Translator::new(granule, bits).unwrap();
            let memory = page_fixture(translator, va, physical);
            assert_eq!(
                translator
                    .translate(&memory, TranslationRoot::new(0x1_0000), Gva::new(va))
                    .unwrap(),
                Translation::new(Gpa::new(physical | (va & (page_size - 1))), page_size)
            );
        }
    }

    #[test]
    fn translates_blocks_for_each_granule() {
        for (granule, bits, va, block_base, expected_size) in [
            (
                TranslationGranule::KiB4,
                48,
                0x4000_1234,
                0x8000_0000,
                1 << 30,
            ),
            (
                TranslationGranule::KiB16,
                47,
                0x10_0123_4567,
                0x2000_0000,
                1 << 25,
            ),
            (
                TranslationGranule::KiB64,
                48,
                0x20_1234_5678,
                0x4_0000_0000,
                1 << 29,
            ),
        ] {
            let translator = Aarch64Translator::new(granule, bits).unwrap();
            let page_shift = granule.page_shift();
            let index_bits = granule.index_bits();
            let levels = (bits - page_shift).div_ceil(index_bits);
            let top_shift = page_shift + (levels - 1) * index_bits;
            let next_shift = top_shift - index_bits;
            let top_index = (va >> top_shift) & ((1 << index_bits) - 1);
            let block_index = (va >> next_shift) & ((1 << index_bits) - 1);
            let next_table = 0x1_0000 + (1u64 << page_shift);
            let memory = Memory::new(&[
                (0x1_0000 + top_index * 8, next_table | 3),
                (next_table + block_index * 8, block_base | 1),
            ]);
            assert_eq!(
                translator
                    .translate(&memory, TranslationRoot::new(0x1_0000), Gva::new(va))
                    .unwrap(),
                Translation::new(
                    Gpa::new(block_base | (va & (expected_size - 1))),
                    expected_size,
                )
            );
        }
    }

    #[test]
    fn validates_address_sizes_and_canonical_addresses() {
        assert!(!is_canonical(0, 0));
        assert!(!is_canonical(0, 64));
        assert!(!is_canonical(0, u8::MAX));
        assert!(Aarch64Translator::new(TranslationGranule::KiB4, 12).is_err());
        assert!(Aarch64Translator::new(TranslationGranule::KiB16, 49).is_err());
        let translator = Aarch64Translator::new(TranslationGranule::KiB4, 39).unwrap();
        assert!(matches!(
            translator.translate(
                &Memory::new(&[]),
                TranslationRoot::new(0),
                Gva::new(1 << 39)
            ),
            Err(VmiError::NonCanonicalAddress { bits: 39, .. })
        ));
    }

    #[test]
    fn rejects_level_zero_blocks() {
        let memory = Memory::new(&[(0x1000, 0x4000_0001)]);
        assert!(matches!(
            Aarch64Translator::default().translate(
                &memory,
                TranslationRoot::new(0x1000),
                Gva::new(0)
            ),
            Err(VmiError::InvalidPageTableEntry { level: 0, .. })
        ));
    }

    proptest! {
        #[test]
        fn arbitrary_supported_granule_pages_preserve_offsets(
            granule_index in 0usize..3,
            page_index in 0u64..(1u64 << 30),
            offset_seed in any::<u16>(),
            physical_seed in 0u64..(1u64 << 40),
        ) {
            let (granule, bits) = [
                (TranslationGranule::KiB4, 48),
                (TranslationGranule::KiB16, 47),
                (TranslationGranule::KiB64, 48),
            ][granule_index];
            let translator = Aarch64Translator::new(granule, bits).unwrap();
            let page_size = 1u64 << granule.page_shift();
            let offset = u64::from(offset_seed) & (page_size - 1);
            let va = (page_index << granule.page_shift()) | offset;
            let physical = physical_seed & PHYSICAL_ADDRESS_MASK & !(page_size - 1);
            let memory = page_fixture(translator, va, physical);
            let translated = translator
                .translate(&memory, TranslationRoot::new(0x1_0000), Gva::new(va))
                .unwrap();
            prop_assert_eq!(translated, Translation::new(Gpa::new(physical | offset), page_size));
        }

        #[test]
        fn arbitrary_addresses_above_configured_width_are_rejected(
            address_bits in 13u8..=48,
            low in any::<u64>(),
        ) {
            let translator = Aarch64Translator::new(TranslationGranule::KiB4, address_bits).unwrap();
            let low_mask = (1u64 << address_bits) - 1;
            let address = (low & low_mask) | (1u64 << address_bits);
            let rejected = matches!(
                translator.translate(
                    &Memory::new(&[]),
                    TranslationRoot::new(0),
                    Gva::new(address),
                ),
                Err(VmiError::NonCanonicalAddress { .. })
            );
            prop_assert!(rejected);
        }
    }
}
