use std::collections::HashSet;

use vmi_arch_api::AddressTranslator;
use vmi_core::VmiSession;
use vmi_profile::SymbolTable;
use vmi_types::{Gva, Result, TranslationRoot, VmiError};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct WindowsProcessOffsets {
    pub active_process_links: u64,
    pub unique_process_id: u64,
    pub image_file_name: u64,
    pub image_file_name_length: usize,
    pub directory_table_base: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsProcess {
    pub eprocess: Gva,
    pub pid: u64,
    pub image: String,
    pub directory_table_base: u64,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct WindowsModuleOffsets {
    pub in_load_order_links: u64,
    pub dll_base: u64,
    pub size_of_image: u64,
    pub base_dll_name: u64,
    pub maximum_name_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsModule {
    pub entry: Gva,
    pub name: String,
    pub base: Gva,
    pub size: u32,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct WindowsFileOffsets {
    pub device_object: u64,
    pub file_name: u64,
    pub flags: u64,
    pub read_access: u64,
    pub write_access: u64,
    pub delete_access: u64,
    pub maximum_name_bytes: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsFile {
    pub file_object: Gva,
    pub device_object: Gva,
    pub name: String,
    pub flags: u32,
    pub read_access: bool,
    pub write_access: bool,
    pub delete_access: bool,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct WindowsHandleTableOffsets {
    pub eprocess_object_table: u64,
    pub table_code: u64,
    pub next_handle_needing_pool: u64,
    pub entry_size: u64,
    pub entry_object: u64,
    pub entry_granted_access: u64,
    pub object_pointer_mask: u64,
    pub object_pointer_shift: u8,
    pub object_pointer_sign_bit: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsHandle {
    pub handle: u64,
    pub object: Gva,
    pub granted_access: u32,
}

pub struct WindowsIntrospector<'a> {
    session: &'a VmiSession,
    translator: &'a dyn AddressTranslator,
    kernel_root: TranslationRoot,
    profile: &'a SymbolTable,
    offsets: WindowsProcessOffsets,
}

impl<'a> WindowsIntrospector<'a> {
    pub fn new(
        session: &'a VmiSession,
        translator: &'a dyn AddressTranslator,
        kernel_root: TranslationRoot,
        profile: &'a SymbolTable,
        offsets: WindowsProcessOffsets,
    ) -> Self {
        Self {
            session,
            translator,
            kernel_root,
            profile,
            offsets,
        }
    }

    pub fn processes(&self, limit: usize) -> Result<Vec<WindowsProcess>> {
        if limit == 0
            || self.offsets.image_file_name_length == 0
            || self.offsets.image_file_name_length > 4096
        {
            return Err(VmiError::Backend(
                "invalid Windows traversal limit or image length".into(),
            ));
        }
        let head = self
            .profile
            .symbol("PsActiveProcessHead")
            .ok_or_else(|| {
                VmiError::Backend("profile does not contain PsActiveProcessHead".into())
            })?
            .address;
        let mut node = self.read_u64(head)?;
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        while node != head {
            reserve_seen(&mut seen, "Windows process cycle detector")?;
            if !seen.insert(node) {
                return Err(VmiError::Backend(format!(
                    "Windows process list looped at unexpected node {node:#x}"
                )));
            }
            let eprocess = node
                .checked_sub(self.offsets.active_process_links)
                .ok_or_else(|| {
                    VmiError::Backend(format!("invalid ActiveProcessLinks pointer {node:#x}"))
                })?;
            reserve_one(&mut output, "Windows process list")?;
            output.push(self.read_process(eprocess)?);
            node = self.read_u64(node)?;
            if node != head && output.len() >= limit {
                return Err(VmiError::Backend(format!(
                    "Windows process list exceeded limit {limit}"
                )));
            }
        }
        Ok(output)
    }

    pub fn modules(
        &self,
        offsets: WindowsModuleOffsets,
        limit: usize,
    ) -> Result<Vec<WindowsModule>> {
        if limit == 0 || offsets.maximum_name_bytes == 0 || offsets.maximum_name_bytes > 65_536 {
            return Err(VmiError::Backend(
                "invalid Windows module traversal limit or name length".into(),
            ));
        }
        let head = self
            .profile
            .symbol("PsLoadedModuleList")
            .ok_or_else(|| VmiError::Backend("profile does not contain PsLoadedModuleList".into()))?
            .address;
        let mut node = self.read_u64(head)?;
        let mut seen = HashSet::new();
        let mut output = Vec::new();
        while node != head {
            reserve_seen(&mut seen, "Windows module cycle detector")?;
            if !seen.insert(node) {
                return Err(VmiError::Backend(format!(
                    "Windows module list looped at unexpected node {node:#x}"
                )));
            }
            let entry = node
                .checked_sub(offsets.in_load_order_links)
                .ok_or_else(|| {
                    VmiError::Backend(format!("invalid module list pointer {node:#x}"))
                })?;
            reserve_one(&mut output, "Windows module list")?;
            output.push(self.read_module(entry, offsets)?);
            node = self.read_u64(node)?;
            if node != head && output.len() >= limit {
                return Err(VmiError::Backend(format!(
                    "Windows module list exceeded limit {limit}"
                )));
            }
        }
        Ok(output)
    }

    pub fn file_object(&self, address: Gva, offsets: WindowsFileOffsets) -> Result<WindowsFile> {
        if address.raw() == 0
            || offsets.maximum_name_bytes == 0
            || offsets.maximum_name_bytes > 65_536
        {
            return Err(VmiError::Backend(
                "invalid Windows FILE_OBJECT arguments".into(),
            ));
        }
        let device_object = self.read_u64(add(
            address.raw(),
            offsets.device_object,
            "file device object",
        )?)?;
        let name = self.read_unicode_string(
            add(address.raw(), offsets.file_name, "file name")?,
            offsets.maximum_name_bytes,
        )?;
        let flags = self.read_u32(add(address.raw(), offsets.flags, "file flags")?)?;
        let read_access =
            self.read_bool(add(address.raw(), offsets.read_access, "file read access")?)?;
        let write_access = self.read_bool(add(
            address.raw(),
            offsets.write_access,
            "file write access",
        )?)?;
        let delete_access = self.read_bool(add(
            address.raw(),
            offsets.delete_access,
            "file delete access",
        )?)?;
        Ok(WindowsFile {
            file_object: address,
            device_object: Gva::new(device_object),
            name,
            flags,
            read_access,
            write_access,
            delete_access,
        })
    }

    pub fn handles(
        &self,
        eprocess: Gva,
        offsets: WindowsHandleTableOffsets,
        handle_limit: usize,
    ) -> Result<Vec<WindowsHandle>> {
        if eprocess.raw() == 0
            || handle_limit == 0
            || offsets.entry_size == 0
            || offsets.entry_size > 4096
            || 4096u64.checked_rem(offsets.entry_size) != Some(0)
            || offsets.object_pointer_shift >= 64
            || offsets.object_pointer_sign_bit >= 64
        {
            return Err(VmiError::Backend(
                "invalid Windows handle-table arguments".into(),
            ));
        }
        let table = self.read_u64(add(
            eprocess.raw(),
            offsets.eprocess_object_table,
            "process object table",
        )?)?;
        if table == 0 {
            return Ok(Vec::new());
        }
        let table_code = self.read_u64(add(table, offsets.table_code, "handle table code")?)?;
        let level = table_code & 3;
        if level > 2 {
            return Err(VmiError::Backend(format!(
                "invalid Windows handle table level {level}"
            )));
        }
        let entries = table_code & !3;
        if entries == 0 {
            return Err(VmiError::Backend(
                "Windows handle table has null entry array".into(),
            ));
        }
        let next_handle =
            self.read_u64(add(table, offsets.next_handle_needing_pool, "next handle")?)?;
        if next_handle & 3 != 0 {
            return Err(VmiError::Backend(format!(
                "unaligned Windows next handle {next_handle:#x}"
            )));
        }
        let count = usize::try_from(next_handle / 4)
            .map_err(|_| VmiError::Backend("Windows handle count is too large".into()))?;
        if count > handle_limit {
            return Err(VmiError::Backend(format!(
                "Windows handle count {count} exceeds limit {handle_limit}"
            )));
        }
        let mut output = Vec::new();
        for index in 0..count {
            let index_u64 = u64::try_from(index)
                .map_err(|_| VmiError::Backend("Windows handle index is too large".into()))?;
            let entry = self.handle_entry_address(entries, level, index_u64, offsets.entry_size)?;
            let encoded = self.read_u64(add(entry, offsets.entry_object, "handle object")?)?;
            let pointer = (encoded & offsets.object_pointer_mask)
                .checked_shl(u32::from(offsets.object_pointer_shift))
                .ok_or_else(|| VmiError::Backend("Windows object pointer shift overflow".into()))?;
            if pointer == 0 {
                continue;
            }
            let object = sign_extend(pointer, offsets.object_pointer_sign_bit)?;
            let granted_access = self.read_u32(add(
                entry,
                offsets.entry_granted_access,
                "handle granted access",
            )?)?;
            reserve_one(&mut output, "Windows handle list")?;
            let handle = index_u64
                .checked_mul(4)
                .ok_or_else(|| VmiError::Backend("Windows handle value overflow".into()))?;
            output.push(WindowsHandle {
                handle,
                object: Gva::new(object),
                granted_access,
            });
        }
        Ok(output)
    }

    fn handle_entry_address(
        &self,
        table_base: u64,
        level: u64,
        index: u64,
        entry_size: u64,
    ) -> Result<u64> {
        const PAGE_SIZE: u64 = 4096;
        const POINTERS_PER_PAGE: u64 = PAGE_SIZE / 8;
        let entries_per_page = PAGE_SIZE
            .checked_div(entry_size)
            .filter(|entries| *entries != 0)
            .ok_or_else(|| VmiError::Backend("invalid Windows handle entry size".into()))?;
        let leaf_index = index
            .checked_div(entries_per_page)
            .ok_or_else(|| VmiError::Backend("invalid Windows handle page divisor".into()))?;
        let entry_index = index
            .checked_rem(entries_per_page)
            .ok_or_else(|| VmiError::Backend("invalid Windows handle page divisor".into()))?;
        let leaf = match level {
            0 => table_base,
            1 => self.read_table_pointer(table_base, leaf_index)?,
            2 => {
                let directory_index = leaf_index / POINTERS_PER_PAGE;
                let pointer_index = leaf_index % POINTERS_PER_PAGE;
                let directory = self.read_table_pointer(table_base, directory_index)?;
                self.read_table_pointer(directory, pointer_index)?
            }
            _ => {
                return Err(VmiError::Backend(format!(
                    "invalid Windows handle table level {level}"
                )))
            }
        };
        leaf.checked_add(
            entry_index
                .checked_mul(entry_size)
                .ok_or_else(|| VmiError::Backend("Windows handle entry offset overflow".into()))?,
        )
        .ok_or_else(|| VmiError::Backend("Windows handle leaf overflow".into()))
    }

    fn read_table_pointer(&self, table: u64, index: u64) -> Result<u64> {
        let address = table
            .checked_add(index.checked_mul(8).ok_or_else(|| {
                VmiError::Backend("Windows handle pointer offset overflow".into())
            })?)
            .ok_or_else(|| VmiError::Backend("Windows handle pointer table overflow".into()))?;
        let pointer = self.read_u64(address)? & !0xf;
        if pointer == 0 || pointer & 0xfff != 0 {
            return Err(VmiError::Backend(format!(
                "invalid Windows handle table page pointer {pointer:#x}"
            )));
        }
        Ok(pointer)
    }

    fn read_process(&self, eprocess: u64) -> Result<WindowsProcess> {
        let pid = self.read_u64(add(eprocess, self.offsets.unique_process_id, "PID")?)?;
        let directory_table_base =
            self.read_u64(add(eprocess, self.offsets.directory_table_base, "DTB")?)?;
        let image_address = add(eprocess, self.offsets.image_file_name, "image")?;
        let bytes = self.session.read_virtual(
            self.translator,
            self.kernel_root,
            Gva::new(image_address),
            self.offsets.image_file_name_length,
        )?;
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        let mut bytes = bytes;
        bytes.truncate(end);
        let image = decode_guest_bytes(bytes, "Windows process image")?;
        Ok(WindowsProcess {
            eprocess: Gva::new(eprocess),
            pid,
            image,
            directory_table_base,
        })
    }

    fn read_module(&self, entry: u64, offsets: WindowsModuleOffsets) -> Result<WindowsModule> {
        let base = self.read_u64(add(entry, offsets.dll_base, "module base")?)?;
        let size = self.read_u32(add(entry, offsets.size_of_image, "module size")?)?;
        let name = self.read_unicode_string(
            add(entry, offsets.base_dll_name, "module name")?,
            offsets.maximum_name_bytes,
        )?;
        Ok(WindowsModule {
            entry: Gva::new(entry),
            name,
            base: Gva::new(base),
            size,
        })
    }

    fn read_unicode_string(&self, address: u64, maximum_bytes: usize) -> Result<String> {
        let length = usize::from(self.read_u16(address)?);
        let declared_maximum = usize::from(self.read_u16(add(address, 2, "Unicode maximum")?)?);
        if length > declared_maximum || length > maximum_bytes || length % 2 != 0 {
            return Err(VmiError::Backend(format!(
                "invalid Windows UNICODE_STRING length {length}/{declared_maximum}"
            )));
        }
        if length == 0 {
            return Ok(String::new());
        }
        let buffer = self.read_u64(add(address, 8, "Unicode buffer")?)?;
        if buffer == 0 {
            return Err(VmiError::Backend(
                "non-empty Windows UNICODE_STRING has null buffer".into(),
            ));
        }
        let bytes = self.session.read_virtual(
            self.translator,
            self.kernel_root,
            Gva::new(buffer),
            length,
        )?;
        let mut words = Vec::new();
        words.try_reserve_exact(bytes.len() / 2).map_err(|error| {
            VmiError::Backend(format!(
                "failed to allocate Windows Unicode buffer: {error}"
            ))
        })?;
        words.extend(bytes.chunks_exact(2).filter_map(|pair| {
            let &[first, second] = pair else {
                return None;
            };
            Some(u16::from_le_bytes([first, second]))
        }));
        decode_utf16_words(&words, "Windows module name")
    }

    fn read_u16(&self, address: u64) -> Result<u16> {
        let bytes =
            self.session
                .read_virtual(self.translator, self.kernel_root, Gva::new(address), 2)?;
        Ok(u16::from_le_bytes(bytes.try_into().map_err(|_| {
            VmiError::Backend("short Windows u16 read".into())
        })?))
    }

    fn read_bool(&self, address: u64) -> Result<bool> {
        let bytes =
            self.session
                .read_virtual(self.translator, self.kernel_root, Gva::new(address), 1)?;
        match bytes
            .first()
            .copied()
            .ok_or_else(|| VmiError::Backend("short Windows BOOLEAN read".into()))?
        {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(VmiError::Backend(format!(
                "invalid Windows BOOLEAN value {value}"
            ))),
        }
    }

    fn read_u32(&self, address: u64) -> Result<u32> {
        let bytes =
            self.session
                .read_virtual(self.translator, self.kernel_root, Gva::new(address), 4)?;
        Ok(u32::from_le_bytes(bytes.try_into().map_err(|_| {
            VmiError::Backend("short Windows u32 read".into())
        })?))
    }

    fn read_u64(&self, address: u64) -> Result<u64> {
        let bytes =
            self.session
                .read_virtual(self.translator, self.kernel_root, Gva::new(address), 8)?;
        Ok(u64::from_le_bytes(bytes.try_into().map_err(|_| {
            VmiError::Backend("short Windows pointer read".into())
        })?))
    }
}

fn decode_guest_bytes(bytes: Vec<u8>, description: &str) -> Result<String> {
    let bytes = match String::from_utf8(bytes) {
        Ok(text) => return Ok(text),
        Err(error) => error.into_bytes(),
    };
    let capacity = bytes
        .len()
        .checked_mul(3)
        .ok_or_else(|| VmiError::Backend(format!("{description} decoded length overflow")))?;
    let mut output = String::new();
    output.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate decoded {description}: {error}"))
    })?;
    let mut offset = 0usize;
    while offset < bytes.len() {
        let remaining = bytes.get(offset..).ok_or_else(|| {
            VmiError::Backend(format!("{description} decoder offset is out of bounds"))
        })?;
        match std::str::from_utf8(remaining) {
            Ok(valid) => {
                output.push_str(valid);
                break;
            }
            Err(error) => {
                let valid_length = error.valid_up_to();
                let valid = remaining.get(..valid_length).ok_or_else(|| {
                    VmiError::Backend(format!("{description} valid prefix is out of bounds"))
                })?;
                let valid = std::str::from_utf8(valid).map_err(|error| {
                    VmiError::Backend(format!("{description} valid prefix failed: {error}"))
                })?;
                output.push_str(valid);
                output.push(char::REPLACEMENT_CHARACTER);
                let Some(error_length) = error.error_len() else {
                    break;
                };
                offset = offset
                    .checked_add(valid_length)
                    .and_then(|value| value.checked_add(error_length))
                    .ok_or_else(|| {
                        VmiError::Backend(format!("{description} decoder progress overflow"))
                    })?;
            }
        }
    }
    Ok(output)
}

fn decode_utf16_words(words: &[u16], description: &str) -> Result<String> {
    let capacity = words
        .len()
        .checked_mul(3)
        .ok_or_else(|| VmiError::Backend(format!("{description} decoded length overflow")))?;
    let mut output = String::new();
    output.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate decoded {description}: {error}"))
    })?;
    for decoded in char::decode_utf16(words.iter().copied()) {
        let character = decoded
            .map_err(|error| VmiError::Backend(format!("invalid {description}: {error}")))?;
        output.push(character);
    }
    Ok(output)
}

fn reserve_one<T>(values: &mut Vec<T>, description: &str) -> Result<()> {
    values
        .try_reserve(1)
        .map_err(|error| VmiError::Backend(format!("failed to grow {description}: {error}")))
}

fn reserve_seen<T: Eq + std::hash::Hash>(values: &mut HashSet<T>, description: &str) -> Result<()> {
    values
        .try_reserve(1)
        .map_err(|error| VmiError::Backend(format!("failed to grow {description}: {error}")))
}

fn add(base: u64, offset: u64, field: &str) -> Result<u64> {
    base.checked_add(offset)
        .ok_or_else(|| VmiError::Backend(format!("Windows {field} address overflow")))
}

fn sign_extend(value: u64, sign_bit: u8) -> Result<u64> {
    let width = sign_bit
        .checked_add(1)
        .ok_or_else(|| VmiError::Backend("Windows sign-extension width overflow".into()))?;
    let low_mask = if width == 64 {
        u64::MAX
    } else {
        1u64.checked_shl(u32::from(width))
            .and_then(|mask| mask.checked_sub(1))
            .ok_or_else(|| VmiError::Backend("Windows sign-extension mask overflow".into()))?
    };
    let sign_mask = 1u64
        .checked_shl(u32::from(sign_bit))
        .ok_or_else(|| VmiError::Backend("Windows sign bit overflow".into()))?;
    let truncated = value & low_mask;
    if truncated & sign_mask == 0 {
        Ok(truncated)
    } else {
        Ok(truncated | !low_mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vmi_arch_api::Translation;
    use vmi_driver_api::MemoryAccess;
    use vmi_testkit::FakeConnector;
    use vmi_types::{AttachRequest, Gpa};

    #[test]
    fn guest_byte_decoder_matches_lossy_utf8_semantics() {
        for bytes in [
            b"System".to_vec(),
            "image ☃".as_bytes().to_vec(),
            vec![0xf0, 0x28, 0x8c, 0x28],
            vec![0xe2, 0x82],
        ] {
            let expected = String::from_utf8_lossy(&bytes).into_owned();
            assert_eq!(decode_guest_bytes(bytes, "test").unwrap(), expected);
        }
    }

    #[test]
    fn utf16_decoder_handles_bmp_pairs_and_invalid_surrogates() {
        let words: Vec<u16> = "kernel-\u{1f980}.sys".encode_utf16().collect();
        assert_eq!(
            decode_utf16_words(&words, "test").unwrap(),
            "kernel-\u{1f980}.sys"
        );
        assert!(decode_utf16_words(&[0xd800], "test").is_err());
    }

    struct Identity;
    impl AddressTranslator for Identity {
        fn cache_tag(&self) -> u64 {
            0x5749_4e44_4f57_5349
        }

        fn translate(
            &self,
            _memory: &dyn MemoryAccess,
            _root: TranslationRoot,
            address: Gva,
        ) -> Result<Translation> {
            Ok(Translation::new(Gpa::new(address.raw() & !0xfff), 4096))
        }
    }

    fn process(next: u64, pid: u64, dtb: u64, image: &str) -> Vec<u8> {
        let mut data = vec![0u8; 0x50];
        data[0x10..0x18].copy_from_slice(&next.to_le_bytes());
        data[0x20..0x28].copy_from_slice(&pid.to_le_bytes());
        data[0x28..0x30].copy_from_slice(&dtb.to_le_bytes());
        data[0x30..0x30 + image.len()].copy_from_slice(image.as_bytes());
        data
    }

    fn module(next: u64, base: u64, size: u32, name_buffer: u64, name: &str) -> Vec<u8> {
        let mut data = vec![0u8; 0x60];
        data[0x10..0x18].copy_from_slice(&next.to_le_bytes());
        data[0x20..0x28].copy_from_slice(&base.to_le_bytes());
        data[0x28..0x2c].copy_from_slice(&size.to_le_bytes());
        let name_bytes = name.encode_utf16().count() as u16 * 2;
        data[0x30..0x32].copy_from_slice(&name_bytes.to_le_bytes());
        data[0x32..0x34].copy_from_slice(&name_bytes.to_le_bytes());
        data[0x38..0x40].copy_from_slice(&name_buffer.to_le_bytes());
        data
    }

    fn module_offsets() -> WindowsModuleOffsets {
        WindowsModuleOffsets {
            in_load_order_links: 0x10,
            dll_base: 0x20,
            size_of_image: 0x28,
            base_dll_name: 0x30,
            maximum_name_bytes: 256,
        }
    }

    fn file_offsets() -> WindowsFileOffsets {
        WindowsFileOffsets {
            device_object: 0,
            file_name: 0x10,
            flags: 0x20,
            read_access: 0x24,
            write_access: 0x25,
            delete_access: 0x26,
            maximum_name_bytes: 1024,
        }
    }

    fn file_object(name_address: u64, name: &str) -> Vec<u8> {
        let mut data = vec![0u8; 0x30];
        data[..8].copy_from_slice(&0xffff_8000_1234_0000u64.to_le_bytes());
        let length = (name.encode_utf16().count() * 2) as u16;
        data[0x10..0x12].copy_from_slice(&length.to_le_bytes());
        data[0x12..0x14].copy_from_slice(&length.to_le_bytes());
        data[0x18..0x20].copy_from_slice(&name_address.to_le_bytes());
        data[0x20..0x24].copy_from_slice(&0x120u32.to_le_bytes());
        data[0x24] = 1;
        data[0x25] = 0;
        data[0x26] = 1;
        data
    }

    fn handle_offsets() -> WindowsHandleTableOffsets {
        WindowsHandleTableOffsets {
            eprocess_object_table: 0,
            table_code: 0,
            next_handle_needing_pool: 8,
            entry_size: 16,
            entry_object: 0,
            entry_granted_access: 8,
            object_pointer_mask: !0xf,
            object_pointer_shift: 0,
            object_pointer_sign_bit: 47,
        }
    }

    fn offsets() -> WindowsProcessOffsets {
        WindowsProcessOffsets {
            active_process_links: 0x10,
            unique_process_id: 0x20,
            directory_table_base: 0x28,
            image_file_name: 0x30,
            image_file_name_length: 16,
        }
    }

    #[test]
    fn sign_extension_handles_boundary_bits_and_rejects_invalid_widths() {
        assert_eq!(sign_extend(1 << 47, 47).unwrap(), 0xffff_8000_0000_0000);
        assert_eq!(sign_extend(u64::MAX, 63).unwrap(), u64::MAX);
        assert!(sign_extend(0, 64).is_err());
        assert!(sign_extend(0, u8::MAX).is_err());
    }

    #[test]
    fn walks_windows_process_list() {
        let connector = FakeConnector::default()
            .with_segment(0x800_u64, 0x1010u64.to_le_bytes().to_vec())
            .with_segment(0x1000_u64, process(0x2010, 4, 0x111000, "System"))
            .with_segment(0x2000_u64, process(0x800, 100, 0x222000, "worker.exe"));
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("800 D PsActiveProcessHead\n").unwrap();
        let processes = WindowsIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets(),
        )
        .processes(8)
        .unwrap();
        assert_eq!(
            processes[0],
            WindowsProcess {
                eprocess: Gva::new(0x1000),
                pid: 4,
                image: "System".into(),
                directory_table_base: 0x111000
            }
        );
        assert_eq!(processes[1].image, "worker.exe");
    }

    #[test]
    fn rejects_corrupt_lists_and_invalid_limits() {
        let connector = FakeConnector::default()
            .with_segment(0x800_u64, 0x1008u64.to_le_bytes().to_vec())
            .with_segment(0x1000_u64, process(0x1008, 4, 0, "bad"));
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("800 D PsActiveProcessHead\n").unwrap();
        let inspector = WindowsIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets(),
        );
        assert!(inspector.processes(4).is_err());
        assert!(inspector.processes(0).is_err());
    }

    #[test]
    fn walks_windows_loaded_module_list_and_decodes_utf16() {
        let first_name: Vec<u8> = "ntoskrnl.exe"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let second_name: Vec<u8> = "드라이버.sys"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let connector = FakeConnector::default()
            .with_segment(0x800_u64, 0x1010u64.to_le_bytes().to_vec())
            .with_segment(
                0x1000_u64,
                module(
                    0x2010,
                    0xffff_f800_0000_0000,
                    0x200000,
                    0x3000,
                    "ntoskrnl.exe",
                ),
            )
            .with_segment(
                0x2000_u64,
                module(
                    0x800,
                    0xffff_f800_0100_0000,
                    0x12000,
                    0x4000,
                    "드라이버.sys",
                ),
            )
            .with_segment(0x3000_u64, first_name)
            .with_segment(0x4000_u64, second_name);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile =
            SymbolTable::from_system_map("800 D PsLoadedModuleList\n900 D PsActiveProcessHead\n")
                .unwrap();
        let modules = WindowsIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets(),
        )
        .modules(module_offsets(), 8)
        .unwrap();
        assert_eq!(modules[0].name, "ntoskrnl.exe");
        assert_eq!(modules[0].size, 0x200000);
        assert_eq!(modules[1].name, "드라이버.sys");
        assert_eq!(modules[1].base, Gva::new(0xffff_f800_0100_0000));
    }

    #[test]
    fn rejects_corrupt_module_lists_and_unicode_lengths() {
        let mut bad = module(0x1010, 0, 0, 0, "bad");
        bad[0x30..0x32].copy_from_slice(&3u16.to_le_bytes());
        let connector = FakeConnector::default()
            .with_segment(0x800_u64, 0x1010u64.to_le_bytes().to_vec())
            .with_segment(0x1000_u64, bad);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile =
            SymbolTable::from_system_map("800 D PsLoadedModuleList\n900 D PsActiveProcessHead\n")
                .unwrap();
        let inspector = WindowsIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets(),
        );
        assert!(inspector.modules(module_offsets(), 4).is_err());
        assert!(inspector.modules(module_offsets(), 0).is_err());
    }

    #[test]
    fn decodes_windows_file_object() {
        let name = r"\Device\HarddiskVolume3\Windows\System32\ntdll.dll";
        let name_bytes: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let connector = FakeConnector::default()
            .with_segment(0x1000_u64, file_object(0x2000, name))
            .with_segment(0x2000_u64, name_bytes);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("800 D PsActiveProcessHead\n").unwrap();
        let file = WindowsIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets(),
        )
        .file_object(Gva::new(0x1000), file_offsets())
        .unwrap();
        assert_eq!(file.name, name);
        assert_eq!(file.device_object, Gva::new(0xffff_8000_1234_0000));
        assert_eq!(file.flags, 0x120);
        assert!(file.read_access);
        assert!(!file.write_access);
        assert!(file.delete_access);
    }

    #[test]
    fn rejects_malformed_windows_file_objects() {
        let mut invalid_bool = file_object(0x2000, "x");
        invalid_bool[0x24] = 2;
        let connector = FakeConnector::default()
            .with_segment(0x1000_u64, invalid_bool)
            .with_segment(0x2000_u64, vec![b'x', 0]);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("800 D PsActiveProcessHead\n").unwrap();
        let inspector = WindowsIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets(),
        );
        assert!(inspector
            .file_object(Gva::new(0x1000), file_offsets())
            .is_err());
        assert!(inspector.file_object(Gva::new(0), file_offsets()).is_err());
    }

    #[test]
    fn enumerates_level_zero_windows_handle_tables() {
        let mut table = vec![0u8; 16];
        table[..8].copy_from_slice(&0x4000u64.to_le_bytes());
        table[8..16].copy_from_slice(&12u64.to_le_bytes());
        let mut entries = vec![0u8; 48];
        entries[..8].copy_from_slice(&0xffff_8000_1111_0000u64.to_le_bytes());
        entries[8..12].copy_from_slice(&0x12019fu32.to_le_bytes());
        entries[32..40].copy_from_slice(&0xffff_8000_2222_0000u64.to_le_bytes());
        entries[40..44].copy_from_slice(&0x1f0003u32.to_le_bytes());
        let connector = FakeConnector::default()
            .with_segment(0x2000_u64, 0x3000u64.to_le_bytes().to_vec())
            .with_segment(0x3000_u64, table)
            .with_segment(0x4000_u64, entries);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("800 D PsActiveProcessHead\n").unwrap();
        let handles = WindowsIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets(),
        )
        .handles(Gva::new(0x2000), handle_offsets(), 16)
        .unwrap();
        assert_eq!(handles.len(), 2);
        assert_eq!(handles[0].handle, 0);
        assert_eq!(handles[0].object, Gva::new(0xffff_8000_1111_0000));
        assert_eq!(handles[0].granted_access, 0x12019f);
        assert_eq!(handles[1].handle, 8);
    }

    #[test]
    fn enumerates_level_one_and_two_windows_handle_tables() {
        for (level, pointer_pages, leaf) in [
            (1u64, vec![(0x4000u64, 0x5000u64)], 0x5000u64),
            (
                2u64,
                vec![(0x4000u64, 0x5000u64), (0x5000u64, 0x6000u64)],
                0x6000u64,
            ),
        ] {
            let mut table = vec![0u8; 16];
            table[..8].copy_from_slice(&(0x4000 | level).to_le_bytes());
            table[8..16].copy_from_slice(&4u64.to_le_bytes());
            let mut connector = FakeConnector::default()
                .with_segment(0x2000_u64, 0x3000u64.to_le_bytes().to_vec())
                .with_segment(0x3000_u64, table);
            for (page, next) in pointer_pages {
                connector = connector.with_segment(page, next.to_le_bytes().to_vec());
            }
            let mut entry = vec![0u8; 16];
            entry[..8].copy_from_slice(&0xffff_8000_3333_0000u64.to_le_bytes());
            entry[8..12].copy_from_slice(&0x1234u32.to_le_bytes());
            connector = connector.with_segment(leaf, entry);
            let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
            let profile = SymbolTable::from_system_map("800 D PsActiveProcessHead\n").unwrap();
            let handles = WindowsIntrospector::new(
                &session,
                &Identity,
                TranslationRoot::new(0),
                &profile,
                offsets(),
            )
            .handles(Gva::new(0x2000), handle_offsets(), 16)
            .unwrap();
            assert_eq!(handles.len(), 1);
            assert_eq!(handles[0].object, Gva::new(0xffff_8000_3333_0000));
            assert_eq!(handles[0].granted_access, 0x1234);
        }
    }

    #[test]
    fn rejects_invalid_windows_handle_table_pages() {
        let mut table = vec![0u8; 16];
        table[..8].copy_from_slice(&0x4001u64.to_le_bytes());
        table[8..16].copy_from_slice(&4u64.to_le_bytes());
        let connector = FakeConnector::default()
            .with_segment(0x2000_u64, 0x3000u64.to_le_bytes().to_vec())
            .with_segment(0x3000_u64, table);
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("800 D PsActiveProcessHead\n").unwrap();
        let inspector = WindowsIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets(),
        );
        assert!(inspector
            .handles(Gva::new(0x2000), handle_offsets(), 16)
            .is_err());
        assert!(inspector
            .handles(Gva::new(0x2000), handle_offsets(), 0)
            .is_err());
    }

    #[test]
    fn handle_entry_address_rejects_invalid_levels() {
        let connector = FakeConnector::default();
        let session = VmiSession::attach(&connector, AttachRequest::default()).unwrap();
        let profile = SymbolTable::from_system_map("800 D PsActiveProcessHead\n").unwrap();
        let inspector = WindowsIntrospector::new(
            &session,
            &Identity,
            TranslationRoot::new(0),
            &profile,
            offsets(),
        );

        let error = inspector
            .handle_entry_address(0x1000, 3, 0, 16)
            .unwrap_err();
        assert!(matches!(error, VmiError::Backend(message) if message.contains("level 3")));
    }
}
