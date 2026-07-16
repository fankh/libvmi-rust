use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

use bzip2::read::BzDecoder;
use flate2::read::{GzDecoder, ZlibDecoder};
use xz2::read::XzDecoder;

use serde_json::Value;

use vmi_types::{Gpa, MemoryRange, Result, VmiError};

pub const DEFAULT_MAX_DECODED_KDMP_SIZE: u64 = 64 * 1024 * 1024 * 1024;
pub const DEFAULT_MAX_ARTIFACT_SIZE: u64 = 64 * 1024 * 1024 * 1024;
pub const DEFAULT_MAX_MANIFEST_SEGMENTS: usize = 65_536;
pub const DEFAULT_MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const DEFAULT_MAX_MANIFEST_SIZE: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactProvenance {
    pub format: String,
    pub source: String,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct KdmpMetadata {
    pub major_version: u32,
    pub minor_version: u32,
    pub directory_table_base: u64,
    pub pfn_database: u64,
    pub loaded_module_list: u64,
    pub active_process_head: u64,
    pub machine_type: u32,
    pub processor_count: u32,
    pub bugcheck_code: u32,
    pub bugcheck_parameters: [u64; 4],
    pub debugger_data_block: u64,
}

impl KdmpMetadata {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let data = read_file_prefix(path, 0x2000)?;
        Self::parse(&data)
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 0x2000 || !data.starts_with(b"PAGE") || data.get(4..8) != Some(b"DU64") {
            return Err(VmiError::Backend(
                "artifact is not a legacy AMD64 PAGE/DU64 crash dump".into(),
            ));
        }
        let machine_type = read_u32(data, 0x30)?;
        if machine_type != 0x8664 {
            return Err(VmiError::Backend(format!(
                "unsupported KDMP machine type {machine_type:#x}"
            )));
        }
        Ok(Self {
            major_version: read_u32(data, 0x08)?,
            minor_version: read_u32(data, 0x0c)?,
            directory_table_base: read_u64(data, 0x10)?,
            pfn_database: read_u64(data, 0x18)?,
            loaded_module_list: read_u64(data, 0x20)?,
            active_process_head: read_u64(data, 0x28)?,
            machine_type,
            processor_count: read_u32(data, 0x34)?,
            bugcheck_code: read_u32(data, 0x38)?,
            bugcheck_parameters: [
                read_u64(data, 0x40)?,
                read_u64(data, 0x48)?,
                read_u64(data, 0x50)?,
                read_u64(data, 0x58)?,
            ],
            debugger_data_block: read_u64(data, 0x80)?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SnapshotSegment {
    pub range: MemoryRange,
    data: Arc<[u8]>,
}

impl SnapshotSegment {
    pub fn new(start: Gpa, data: impl Into<Arc<[u8]>>) -> Self {
        let data = data.into();
        Self {
            range: MemoryRange::new(start, u64::try_from(data.len()).unwrap_or(u64::MAX)),
            data,
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }
}

#[derive(Clone, Debug)]
pub struct SnapshotBundle {
    pub provenance: ArtifactProvenance,
    segments: Vec<SnapshotSegment>,
}

impl SnapshotBundle {
    pub fn converted_core_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let prefix = read_file_prefix(path, 8)?;
        if prefix.starts_with(b"\x7fELF") {
            Self::elf_vmcore_file(path)
        } else if prefix.starts_with(b"PAGEDU64")
            || prefix.starts_with(&[0x1f, 0x8b])
            || prefix.starts_with(b"BZh")
            || prefix.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0])
            || prefix.starts_with(&[0x28, 0xb5, 0x2f, 0xfd])
            || prefix.first() == Some(&0x78)
        {
            Self::kdmp_file(path)
        } else {
            Err(VmiError::Backend(
                "converted core is neither ELF nor Windows KDMP".into(),
            ))
        }
    }

    pub fn raw_file(path: impl AsRef<Path>, base: Gpa) -> Result<Self> {
        Self::raw_file_with_limit(path, base, DEFAULT_MAX_ARTIFACT_SIZE)
    }

    pub fn raw_file_with_limit(
        path: impl AsRef<Path>,
        base: Gpa,
        max_file_size: u64,
    ) -> Result<Self> {
        let path = path.as_ref();
        let data = read_file_limited(path, max_file_size, "raw artifact")?;
        Self::try_from_raw(path.display().to_string(), base, data.into())
    }

    pub fn from_raw(source: impl Into<String>, base: Gpa, data: Arc<[u8]>) -> Self {
        Self {
            provenance: ArtifactProvenance {
                format: "raw".into(),
                source: source.into(),
            },
            segments: vec![SnapshotSegment::new(base, data)],
        }
    }

    pub fn try_from_raw(source: impl Into<String>, base: Gpa, data: Arc<[u8]>) -> Result<Self> {
        let length = u64::try_from(data.len()).map_err(|_| {
            VmiError::Backend("raw artifact length does not fit the physical address model".into())
        })?;
        let end = u128::from(base.raw()).saturating_add(u128::from(length));
        if end > u128::from(u64::MAX).saturating_add(1) {
            return Err(VmiError::Backend(format!(
                "raw artifact at {base} extends beyond the physical address space"
            )));
        }
        Ok(Self::from_raw(source, base, data))
    }

    pub fn elf_vmcore_file(path: impl AsRef<Path>) -> Result<Self> {
        Self::elf_vmcore_file_with_limits(
            path,
            DEFAULT_MAX_ARTIFACT_SIZE,
            DEFAULT_MAX_ARTIFACT_SIZE,
        )
    }

    pub fn elf_vmcore_file_with_limits(
        path: impl AsRef<Path>,
        max_file_size: u64,
        max_memory_size: u64,
    ) -> Result<Self> {
        let path = path.as_ref();
        let data = read_file_limited(path, max_file_size, "ELF artifact")?;
        Self::from_elf_with_limits(
            path.display().to_string(),
            &data,
            "elf-vmcore",
            max_memory_size,
        )
    }

    pub fn from_elf_vmcore(source: impl Into<String>, data: &[u8]) -> Result<Self> {
        Self::from_elf_with_limits(source, data, "elf-vmcore", DEFAULT_MAX_ARTIFACT_SIZE)
    }

    pub fn xen_core_file(path: impl AsRef<Path>) -> Result<Self> {
        Self::xen_core_file_with_limits(path, DEFAULT_MAX_ARTIFACT_SIZE, DEFAULT_MAX_ARTIFACT_SIZE)
    }

    pub fn xen_core_file_with_limits(
        path: impl AsRef<Path>,
        max_file_size: u64,
        max_memory_size: u64,
    ) -> Result<Self> {
        let path = path.as_ref();
        let data = read_file_limited(path, max_file_size, "Xen core artifact")?;
        Self::from_elf_with_limits(
            path.display().to_string(),
            &data,
            "xen-core",
            max_memory_size,
        )
    }

    pub fn from_xen_core(source: impl Into<String>, data: &[u8]) -> Result<Self> {
        Self::from_elf_with_limits(source, data, "xen-core", DEFAULT_MAX_ARTIFACT_SIZE)
    }

    fn from_elf_with_limits(
        source: impl Into<String>,
        data: &[u8],
        format: &str,
        max_memory_size: u64,
    ) -> Result<Self> {
        if max_memory_size == 0 {
            return Err(VmiError::Backend(
                "maximum ELF decoded memory size must be non-zero".into(),
            ));
        }
        if data.len() < 64
            || !data.starts_with(b"\x7fELF")
            || data.get(4) != Some(&2)
            || data.get(5) != Some(&1)
        {
            return Err(VmiError::Backend(
                "artifact is not little-endian ELF64".into(),
            ));
        }
        let program_offset = read_u64(data, 32)?;
        let entry_size = u64::from(read_u16(data, 54)?);
        let entry_count = u64::from(read_u16(data, 56)?);
        if entry_size < 56 {
            return Err(VmiError::Backend(format!(
                "ELF program header size {entry_size} is too small"
            )));
        }
        let mut segments = Vec::new();
        let mut total_memory_size = 0u64;
        for index in 0..entry_count {
            let offset = program_offset
                .checked_add(
                    index
                        .checked_mul(entry_size)
                        .ok_or_else(|| VmiError::Backend("ELF program table overflow".into()))?,
                )
                .ok_or_else(|| VmiError::Backend("ELF program table overflow".into()))?;
            let offset = usize::try_from(offset)
                .map_err(|_| VmiError::Backend("ELF program header offset is too large".into()))?;
            if read_u32(data, offset)? != 1 {
                continue;
            }
            let file_offset = read_u64(data, add_offset(offset, 8)?)?;
            let physical = read_u64(data, add_offset(offset, 24)?)?;
            let file_size = read_u64(data, add_offset(offset, 32)?)?;
            let memory_size = read_u64(data, add_offset(offset, 40)?)?;
            if file_size > memory_size {
                return Err(VmiError::Backend(
                    "ELF segment file size exceeds memory size".into(),
                ));
            }
            total_memory_size = total_memory_size
                .checked_add(memory_size)
                .ok_or_else(|| VmiError::Backend("ELF decoded memory size overflow".into()))?;
            if total_memory_size > max_memory_size {
                return Err(VmiError::Backend(format!(
                    "ELF decoded memory exceeds {max_memory_size} bytes"
                )));
            }
            let file_start = usize::try_from(file_offset)
                .map_err(|_| VmiError::Backend("ELF segment offset is too large".into()))?;
            let file_len = usize::try_from(file_size)
                .map_err(|_| VmiError::Backend("ELF segment file size is too large".into()))?;
            let memory_len = usize::try_from(memory_size)
                .map_err(|_| VmiError::Backend("ELF segment memory size is too large".into()))?;
            let file_end = file_start
                .checked_add(file_len)
                .ok_or_else(|| VmiError::Backend("ELF segment range overflow".into()))?;
            let source_bytes = data
                .get(file_start..file_end)
                .ok_or_else(|| VmiError::Backend("ELF segment extends beyond artifact".into()))?;
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(memory_len).map_err(|error| {
                VmiError::Backend(format!("failed to allocate ELF segment: {error}"))
            })?;
            bytes.resize(memory_len, 0);
            bytes
                .get_mut(..file_len)
                .ok_or_else(|| VmiError::Backend("ELF segment buffer is too small".into()))?
                .copy_from_slice(source_bytes);
            reserve_segment(&mut segments, format)?;
            segments.push(SnapshotSegment::new(
                Gpa::new(physical),
                Arc::<[u8]>::from(bytes),
            ));
        }
        if segments.is_empty() {
            return Err(VmiError::Backend(
                "ELF artifact contains no PT_LOAD segments".into(),
            ));
        }
        segments.sort_by_key(|segment| segment.range.start);
        for pair in segments.windows(2) {
            let [left, right] = pair else { continue };
            if ranges_overlap(left.range, right.range) {
                return Err(VmiError::Backend(
                    "ELF PT_LOAD physical ranges overlap".into(),
                ));
            }
        }
        Ok(Self {
            provenance: ArtifactProvenance {
                format: format.into(),
                source: source.into(),
            },
            segments,
        })
    }

    pub fn lime_file(path: impl AsRef<Path>) -> Result<Self> {
        Self::lime_file_with_limit(path, DEFAULT_MAX_ARTIFACT_SIZE)
    }

    pub fn lime_file_with_limit(path: impl AsRef<Path>, max_file_size: u64) -> Result<Self> {
        let path = path.as_ref();
        let data = read_file_limited(path, max_file_size, "LiME artifact")?;
        Self::from_lime(path.display().to_string(), &data)
    }

    pub fn kdmp_file(path: impl AsRef<Path>) -> Result<Self> {
        Self::kdmp_file_with_limit(path, DEFAULT_MAX_DECODED_KDMP_SIZE)
    }

    pub fn kdmp_file_with_limit(path: impl AsRef<Path>, max_decoded_size: u64) -> Result<Self> {
        let path = path.as_ref();
        if max_decoded_size == 0 {
            return Err(VmiError::Backend(
                "maximum decoded KDMP size must be non-zero".into(),
            ));
        }
        let data = read_file_limited(path, max_decoded_size, "encoded KDMP artifact")?;
        let decoded = if data.starts_with(&[0x1f, 0x8b]) {
            decode_compressed_kdmp(GzDecoder::new(data.as_slice()), "gzip", max_decoded_size)?
        } else if data.starts_with(b"BZh") {
            decode_compressed_kdmp(BzDecoder::new(data.as_slice()), "bzip2", max_decoded_size)?
        } else if data.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0x00]) {
            decode_compressed_kdmp(XzDecoder::new(data.as_slice()), "xz", max_decoded_size)?
        } else if data.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
            let decoder = zstd::stream::read::Decoder::new(data.as_slice()).map_err(|error| {
                VmiError::Backend(format!("failed to initialize zstd KDMP decoder: {error}"))
            })?;
            decode_compressed_kdmp(decoder, "zstd", max_decoded_size)?
        } else if data.first() == Some(&0x78) {
            decode_compressed_kdmp(ZlibDecoder::new(data.as_slice()), "zlib", max_decoded_size)?
        } else {
            data
        };
        Self::from_kdmp(path.display().to_string(), &decoded)
    }

    pub fn from_kdmp(source: impl Into<String>, data: &[u8]) -> Result<Self> {
        const HEADER_SIZE: usize = 0x2000;
        const DESCRIPTOR_OFFSET: usize = 0x88;
        const RUNS_OFFSET: usize = 0x98;
        const RUN_SIZE: usize = 16;
        const PAGE_SIZE: u64 = 4096;
        if data.len() < HEADER_SIZE || !data.starts_with(b"PAGE") || data.get(4..8) != Some(b"DU64")
        {
            return Err(VmiError::Backend(
                "artifact is not a legacy AMD64 PAGE/DU64 crash dump".into(),
            ));
        }
        if data.get(HEADER_SIZE..HEADER_SIZE + 4) == Some(b"SDMP")
            || data.get(HEADER_SIZE..HEADER_SIZE + 4) == Some(b"FDMP")
        {
            return Self::from_bitmap_kdmp(source, data);
        }
        if read_u32(data, HEADER_SIZE)? == 0x40
            && data.get(HEADER_SIZE + 4..HEADER_SIZE + 8) == Some(b"RDMP")
        {
            return Self::from_rdmp_kdmp(source, data);
        }
        let machine = read_u32(data, 0x30)?;
        if machine != 0x8664 {
            return Err(VmiError::Backend(format!(
                "unsupported KDMP machine type {machine:#x}"
            )));
        }
        let run_count = usize::try_from(read_u32(data, DESCRIPTOR_OFFSET)?)
            .map_err(|_| VmiError::Backend("KDMP run count does not fit this host".into()))?;
        let declared_pages = read_u64(data, DESCRIPTOR_OFFSET + 8)?;
        let maximum_runs = (HEADER_SIZE - RUNS_OFFSET) / RUN_SIZE;
        if run_count == 0 || run_count > maximum_runs {
            return Err(VmiError::Backend(format!(
                "invalid KDMP physical run count {run_count}"
            )));
        }
        let mut payload_offset = HEADER_SIZE;
        let mut actual_pages = 0u64;
        let mut segments = Vec::new();
        segments.try_reserve_exact(run_count).map_err(|error| {
            VmiError::Backend(format!("failed to allocate KDMP run table: {error}"))
        })?;
        for index in 0..run_count {
            let run_offset = index
                .checked_mul(RUN_SIZE)
                .and_then(|offset| RUNS_OFFSET.checked_add(offset))
                .ok_or_else(|| VmiError::Backend("KDMP run-table offset overflow".into()))?;
            let base_page = read_u64(data, run_offset)?;
            let page_count = read_u64(data, add_offset(run_offset, 8)?)?;
            if page_count == 0 {
                return Err(VmiError::Backend(format!(
                    "KDMP physical run {index} is empty"
                )));
            }
            actual_pages = actual_pages
                .checked_add(page_count)
                .ok_or_else(|| VmiError::Backend("KDMP page count overflow".into()))?;
            let gpa = base_page
                .checked_mul(PAGE_SIZE)
                .ok_or_else(|| VmiError::Backend("KDMP physical address overflow".into()))?;
            let byte_length = page_count
                .checked_mul(PAGE_SIZE)
                .ok_or_else(|| VmiError::Backend("KDMP run length overflow".into()))?;
            let length = usize::try_from(byte_length)
                .map_err(|_| VmiError::Backend("KDMP run is too large".into()))?;
            let payload_end = payload_offset
                .checked_add(length)
                .ok_or_else(|| VmiError::Backend("KDMP payload offset overflow".into()))?;
            let bytes = data.get(payload_offset..payload_end).ok_or_else(|| {
                VmiError::Backend(format!("KDMP physical run {index} exceeds artifact"))
            })?;
            reserve_segment(&mut segments, "legacy KDMP")?;
            segments.push(SnapshotSegment::new(
                Gpa::new(gpa),
                Arc::<[u8]>::from(bytes),
            ));
            payload_offset = payload_end;
        }
        if actual_pages != declared_pages {
            return Err(VmiError::Backend(format!(
                "KDMP declares {declared_pages} pages but runs contain {actual_pages}"
            )));
        }
        segments.sort_by_key(|segment| segment.range.start);
        for pair in segments.windows(2) {
            let [left, right] = pair else { continue };
            if ranges_overlap(left.range, right.range) {
                return Err(VmiError::Backend(
                    "KDMP physical memory runs overlap".into(),
                ));
            }
        }
        Ok(Self {
            provenance: ArtifactProvenance {
                format: "windows-kdmp".into(),
                source: source.into(),
            },
            segments,
        })
    }

    fn from_bitmap_kdmp(source: impl Into<String>, data: &[u8]) -> Result<Self> {
        const HEADER: usize = 0x2000;
        const PAGE_SIZE: usize = 4096;
        if data.get(HEADER + 4..HEADER + 8) != Some(b"DUMP") {
            return Err(VmiError::Backend("invalid bitmap KDMP marker".into()));
        }
        let first_page = usize::try_from(read_u64(data, HEADER + 0x20)?)
            .map_err(|_| VmiError::Backend("bitmap KDMP first-page offset is too large".into()))?;
        let present = read_u64(data, HEADER + 0x28)?;
        let pages = read_u64(data, HEADER + 0x30)?;
        if pages == 0 || present > pages {
            return Err(VmiError::Backend("invalid bitmap KDMP page counts".into()));
        }
        let bitmap_len = usize::try_from(pages.div_ceil(8))
            .map_err(|_| VmiError::Backend("bitmap KDMP bitmap is too large".into()))?;
        let bitmap_start = HEADER
            .checked_add(0x38)
            .ok_or_else(|| VmiError::Backend("bitmap KDMP table offset overflow".into()))?;
        let bitmap_end = bitmap_start
            .checked_add(bitmap_len)
            .ok_or_else(|| VmiError::Backend("bitmap KDMP table range overflow".into()))?;
        let bitmap = data
            .get(bitmap_start..bitmap_end)
            .ok_or_else(|| VmiError::Backend("truncated bitmap KDMP bitmap".into()))?;
        if first_page < bitmap_end {
            return Err(VmiError::Backend(
                "bitmap KDMP payload overlaps its bitmap".into(),
            ));
        }

        let mut payload = first_page;
        let mut found = 0u64;
        let mut segments = Vec::new();
        for pfn in 0..pages {
            let bitmap_index = usize::try_from(pfn / 8)
                .map_err(|_| VmiError::Backend("bitmap KDMP PFN is too large".into()))?;
            let bitmap_byte = bitmap
                .get(bitmap_index)
                .ok_or_else(|| VmiError::Backend("bitmap KDMP PFN exceeds its bitmap".into()))?;
            if bitmap_byte & (1 << (pfn % 8)) == 0 {
                continue;
            }
            let end = payload
                .checked_add(PAGE_SIZE)
                .ok_or_else(|| VmiError::Backend("bitmap KDMP payload overflow".into()))?;
            let bytes = data
                .get(payload..end)
                .ok_or_else(|| VmiError::Backend("bitmap KDMP page exceeds artifact".into()))?;
            reserve_segment(&mut segments, "bitmap KDMP")?;
            segments.push(SnapshotSegment::new(
                Gpa::new(
                    pfn.checked_mul(u64::try_from(PAGE_SIZE).map_err(|_| {
                        VmiError::Backend("bitmap KDMP page size does not fit u64".into())
                    })?)
                    .ok_or_else(|| {
                        VmiError::Backend("bitmap KDMP physical address overflow".into())
                    })?,
                ),
                Arc::<[u8]>::from(bytes),
            ));
            payload = end;
            found = found
                .checked_add(1)
                .ok_or_else(|| VmiError::Backend("bitmap KDMP page count overflow".into()))?;
        }
        if found != present {
            return Err(VmiError::Backend(format!(
                "bitmap KDMP declares {present} present pages but bitmap contains {found}"
            )));
        }
        Ok(Self {
            provenance: ArtifactProvenance {
                format: "windows-kdmp-bitmap".into(),
                source: source.into(),
            },
            segments,
        })
    }

    fn from_rdmp_kdmp(source: impl Into<String>, data: &[u8]) -> Result<Self> {
        const HEADER: usize = 0x2000;
        const RANGES: usize = 0x2030;
        const PAGE_SIZE: u64 = 4096;
        if data.get(HEADER + 8..HEADER + 12) != Some(b"DUMP") {
            return Err(VmiError::Backend("invalid RDMP KDMP marker".into()));
        }
        let metadata_size = read_u64(data, HEADER + 0x10)?;
        let first_page = read_u64(data, HEADER + 0x18)?;
        if metadata_size < 0x20
            || metadata_size % 16 != 0
            || first_page
                != metadata_size
                    .checked_add(0x2020)
                    .ok_or_else(|| VmiError::Backend("RDMP KDMP metadata offset overflow".into()))?
        {
            return Err(VmiError::Backend(
                "invalid RDMP KDMP metadata layout".into(),
            ));
        }
        let range_count = usize::try_from(metadata_size / 16)
            .map_err(|_| VmiError::Backend("RDMP KDMP range table is too large".into()))?;
        let mut payload = usize::try_from(first_page)
            .map_err(|_| VmiError::Backend("RDMP KDMP payload offset is too large".into()))?;
        if payload > data.len() {
            return Err(VmiError::Backend(
                "RDMP KDMP payload exceeds artifact".into(),
            ));
        }
        let mut segments = Vec::new();
        for index in 0..range_count {
            let offset =
                RANGES
                    .checked_add(index.checked_mul(16).ok_or_else(|| {
                        VmiError::Backend("RDMP KDMP range-table overflow".into())
                    })?)
                    .ok_or_else(|| VmiError::Backend("RDMP KDMP range-table overflow".into()))?;
            let base_page = read_u64(data, offset)?;
            let page_count = read_u64(data, add_offset(offset, 8)?)?;
            if base_page == 0 {
                break;
            }
            if page_count == 0 {
                return Err(VmiError::Backend(format!(
                    "RDMP KDMP physical range {index} is empty"
                )));
            }
            let gpa = base_page
                .checked_mul(PAGE_SIZE)
                .ok_or_else(|| VmiError::Backend("RDMP KDMP physical address overflow".into()))?;
            let byte_length = page_count
                .checked_mul(PAGE_SIZE)
                .ok_or_else(|| VmiError::Backend("RDMP KDMP range length overflow".into()))?;
            let length = usize::try_from(byte_length)
                .map_err(|_| VmiError::Backend("RDMP KDMP range is too large".into()))?;
            let end = payload
                .checked_add(length)
                .ok_or_else(|| VmiError::Backend("RDMP KDMP payload overflow".into()))?;
            let bytes = data.get(payload..end).ok_or_else(|| {
                VmiError::Backend(format!("RDMP KDMP physical range {index} exceeds artifact"))
            })?;
            reserve_segment(&mut segments, "RDMP KDMP")?;
            segments.push(SnapshotSegment::new(
                Gpa::new(gpa),
                Arc::<[u8]>::from(bytes),
            ));
            payload = end;
        }
        if segments.is_empty() {
            return Err(VmiError::Backend(
                "RDMP KDMP contains no physical ranges".into(),
            ));
        }
        segments.sort_by_key(|segment| segment.range.start);
        for pair in segments.windows(2) {
            let [left, right] = pair else { continue };
            if ranges_overlap(left.range, right.range) {
                return Err(VmiError::Backend(
                    "RDMP KDMP physical ranges overlap".into(),
                ));
            }
        }
        Ok(Self {
            provenance: ArtifactProvenance {
                format: "windows-kdmp-rdmp".into(),
                source: source.into(),
            },
            segments,
        })
    }

    pub fn from_lime(source: impl Into<String>, data: &[u8]) -> Result<Self> {
        const HEADER_SIZE: usize = 32;
        const LIME_MAGIC: u32 = 0x4c69_4d45;
        let mut cursor = 0usize;
        let mut segments = Vec::new();
        while cursor < data.len() {
            let header_end = cursor
                .checked_add(HEADER_SIZE)
                .ok_or_else(|| VmiError::Backend("LiME header offset overflow".into()))?;
            if header_end > data.len() {
                return Err(VmiError::Backend("truncated LiME range header".into()));
            }
            if read_u32(data, cursor)? != LIME_MAGIC {
                return Err(VmiError::Backend(format!(
                    "invalid LiME magic at offset {cursor}"
                )));
            }
            let version = read_u32(data, add_offset(cursor, 4)?)?;
            if version != 1 {
                return Err(VmiError::Backend(format!(
                    "unsupported LiME version {version}"
                )));
            }
            let start = read_u64(data, add_offset(cursor, 8)?)?;
            let end = read_u64(data, add_offset(cursor, 16)?)?;
            let length = end
                .checked_sub(start)
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| {
                    VmiError::Backend(format!("invalid LiME range {start:#x}..={end:#x}"))
                })?;
            let length = usize::try_from(length)
                .map_err(|_| VmiError::Backend("LiME range is too large".into()))?;
            let data_end = header_end
                .checked_add(length)
                .ok_or_else(|| VmiError::Backend("LiME range offset overflow".into()))?;
            let bytes = data
                .get(header_end..data_end)
                .ok_or_else(|| VmiError::Backend("LiME range extends beyond artifact".into()))?;
            reserve_segment(&mut segments, "LiME")?;
            segments.push(SnapshotSegment::new(
                Gpa::new(start),
                Arc::<[u8]>::from(bytes),
            ));
            cursor = data_end;
        }
        if segments.is_empty() {
            return Err(VmiError::Backend("LiME artifact contains no ranges".into()));
        }
        segments.sort_by_key(|segment| segment.range.start);
        for pair in segments.windows(2) {
            let [left, right] = pair else { continue };
            if ranges_overlap(left.range, right.range) {
                return Err(VmiError::Backend("LiME physical ranges overlap".into()));
            }
        }
        Ok(Self {
            provenance: ArtifactProvenance {
                format: "lime".into(),
                source: source.into(),
            },
            segments,
        })
    }

    pub fn manifest_file(path: impl AsRef<Path>) -> Result<Self> {
        Self::manifest_file_with_limits(
            path,
            DEFAULT_MAX_MANIFEST_SEGMENTS,
            DEFAULT_MAX_MANIFEST_BYTES,
        )
    }

    pub fn manifest_file_with_limits(
        path: impl AsRef<Path>,
        max_segments: usize,
        max_total_bytes: u64,
    ) -> Result<Self> {
        let path = path.as_ref();
        if max_segments == 0 || max_total_bytes == 0 {
            return Err(VmiError::Backend(
                "manifest segment and byte limits must be non-zero".into(),
            ));
        }
        let contents = read_file_limited(path, DEFAULT_MAX_MANIFEST_SIZE, "snapshot manifest")?;
        let contents = String::from_utf8(contents).map_err(|error| {
            VmiError::Backend(format!(
                "snapshot manifest {} is not UTF-8: {error}",
                path.display()
            ))
        })?;
        let root: Value = serde_json::from_str(&contents).map_err(|error| {
            VmiError::Backend(format!("invalid snapshot manifest JSON: {error}"))
        })?;
        let root = root
            .as_object()
            .ok_or_else(|| VmiError::Backend("snapshot manifest root must be an object".into()))?;
        if root.get("version").and_then(Value::as_u64) != Some(1) {
            return Err(VmiError::Backend(
                "unsupported snapshot manifest version".into(),
            ));
        }
        let format = root
            .get("format")
            .and_then(Value::as_str)
            .ok_or_else(|| VmiError::Backend("snapshot manifest format must be a string".into()))?;
        let entries = root
            .get("segments")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                VmiError::Backend("snapshot manifest segments must be an array".into())
            })?;
        if entries.is_empty() {
            return Err(VmiError::Backend(
                "snapshot manifest contains no segments".into(),
            ));
        }
        if entries.len() > max_segments {
            return Err(VmiError::Backend(format!(
                "snapshot manifest contains {} segments, exceeding limit {max_segments}",
                entries.len()
            )));
        }
        let base = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .canonicalize()
            .map_err(|error| {
                VmiError::Backend(format!(
                    "failed to resolve manifest directory for {}: {error}",
                    path.display()
                ))
            })?;
        let mut segments = Vec::new();
        segments.try_reserve_exact(entries.len()).map_err(|error| {
            VmiError::Backend(format!(
                "failed to allocate snapshot manifest segment table: {error}"
            ))
        })?;
        let mut total_bytes = 0u64;
        for (index, entry) in entries.iter().enumerate() {
            let entry = entry.as_object().ok_or_else(|| {
                VmiError::Backend(format!(
                    "snapshot manifest segment {index} must be an object"
                ))
            })?;
            let file = entry.get("file").and_then(Value::as_str).ok_or_else(|| {
                VmiError::Backend(format!("snapshot manifest segment {index} file is missing"))
            })?;
            let candidate = PathBuf::from(file);
            if candidate.is_absolute() {
                return Err(VmiError::Backend(format!(
                    "snapshot manifest segment {index} file must be relative"
                )));
            }
            let file_path = base.join(candidate).canonicalize().map_err(|error| {
                VmiError::Backend(format!("failed to resolve manifest file {file}: {error}"))
            })?;
            if !file_path.starts_with(&base) {
                return Err(VmiError::Backend(format!(
                    "snapshot manifest segment {index} escapes the manifest directory"
                )));
            }
            let gpa = manifest_u64(entry.get("gpa"), "gpa", index)?;
            let file_offset = manifest_u64(entry.get("file_offset"), "file_offset", index)?;
            let length = manifest_u64(entry.get("length"), "length", index)?;
            if length == 0 {
                return Err(VmiError::Backend(format!(
                    "snapshot manifest segment {index} length is zero"
                )));
            }
            let len = usize::try_from(length).map_err(|_| {
                VmiError::Backend(format!(
                    "snapshot manifest segment {index} length is too large"
                ))
            })?;
            let end = file_offset.checked_add(length).ok_or_else(|| {
                VmiError::Backend(format!(
                    "snapshot manifest segment {index} file range overflow"
                ))
            })?;
            total_bytes = total_bytes.checked_add(length).ok_or_else(|| {
                VmiError::Backend("snapshot manifest total byte count overflow".into())
            })?;
            if total_bytes > max_total_bytes {
                return Err(VmiError::Backend(format!(
                    "snapshot manifest data exceeds configured limit of {max_total_bytes} bytes"
                )));
            }
            let mut source = File::open(&file_path).map_err(|error| {
                VmiError::Backend(format!(
                    "failed to open manifest file {}: {error}",
                    file_path.display()
                ))
            })?;
            if end
                > source
                    .metadata()
                    .map_err(|error| {
                        VmiError::Backend(format!(
                            "failed to inspect manifest file {}: {error}",
                            file_path.display()
                        ))
                    })?
                    .len()
            {
                return Err(VmiError::Backend(format!(
                    "snapshot manifest segment {index} exceeds {}",
                    file_path.display()
                )));
            }
            source.seek(SeekFrom::Start(file_offset)).map_err(|error| {
                VmiError::Backend(format!(
                    "failed to seek manifest file {}: {error}",
                    file_path.display()
                ))
            })?;
            let mut slice = Vec::new();
            slice.try_reserve_exact(len).map_err(|error| {
                VmiError::Backend(format!(
                    "failed to allocate snapshot manifest segment {index}: {error}"
                ))
            })?;
            slice.resize(len, 0);
            source.read_exact(&mut slice).map_err(|error| {
                VmiError::Backend(format!(
                    "failed to read manifest segment {index} from {}: {error}",
                    file_path.display(),
                ))
            })?;
            gpa.checked_add(length).ok_or_else(|| {
                VmiError::Backend(format!(
                    "snapshot manifest segment {index} GPA range overflow"
                ))
            })?;
            reserve_segment(&mut segments, "snapshot manifest")?;
            segments.push(SnapshotSegment::new(
                Gpa::new(gpa),
                Arc::<[u8]>::from(slice),
            ));
        }
        segments.sort_by_key(|segment| segment.range.start);
        for pair in segments.windows(2) {
            let [left, right] = pair else { continue };
            if ranges_overlap(left.range, right.range) {
                return Err(VmiError::Backend(
                    "snapshot manifest physical ranges overlap".into(),
                ));
            }
        }
        Ok(Self {
            provenance: ArtifactProvenance {
                format: format.into(),
                source: path.display().to_string(),
            },
            segments,
        })
    }

    pub fn segments(&self) -> &[SnapshotSegment] {
        &self.segments
    }

    pub fn read_into(&self, address: Gpa, output: &mut [u8]) -> Result<()> {
        if output.is_empty() {
            return Ok(());
        }
        let requested_length = output.len();
        let mut current = u128::from(address.raw());
        let end = current.saturating_add(u128::try_from(output.len()).unwrap_or(u128::MAX));
        if end > u128::from(u64::MAX).saturating_add(1) {
            return Err(VmiError::ReadFailed {
                address: address.raw(),
                length: output.len(),
            });
        }
        let mut covered = current;
        for segment in &self.segments {
            if covered == end {
                break;
            }
            let start = u128::from(segment.range.start.raw());
            let segment_end = start.saturating_add(u128::from(segment.range.length));
            if covered < start {
                break;
            }
            if covered < segment_end {
                covered = segment_end.min(end);
            }
        }
        if covered != end {
            return Err(VmiError::ReadFailed {
                address: address.raw(),
                length: output.len(),
            });
        }
        let mut completed = 0usize;
        for segment in &self.segments {
            let start = u128::from(segment.range.start.raw());
            let segment_end = start.saturating_add(u128::from(segment.range.length));
            if completed == output.len() {
                break;
            }
            if current < start {
                break;
            }
            if current >= segment_end {
                continue;
            }
            let relative = current
                .checked_sub(start)
                .ok_or_else(|| VmiError::ReadFailed {
                    address: address.raw(),
                    length: output.len(),
                })?;
            let offset = usize::try_from(relative).map_err(|_| VmiError::ReadFailed {
                address: address.raw(),
                length: output.len(),
            })?;
            let available = segment_end
                .checked_sub(current)
                .and_then(|length| usize::try_from(length).ok())
                .unwrap_or(usize::MAX);
            let remaining =
                output
                    .len()
                    .checked_sub(completed)
                    .ok_or_else(|| VmiError::ReadFailed {
                        address: address.raw(),
                        length: output.len(),
                    })?;
            let length = available.min(remaining);
            let output_end = completed
                .checked_add(length)
                .ok_or_else(|| VmiError::ReadFailed {
                    address: address.raw(),
                    length: output.len(),
                })?;
            let segment_end = offset
                .checked_add(length)
                .ok_or_else(|| VmiError::ReadFailed {
                    address: address.raw(),
                    length: output.len(),
                })?;
            let source =
                segment
                    .bytes()
                    .get(offset..segment_end)
                    .ok_or_else(|| VmiError::ReadFailed {
                        address: address.raw(),
                        length: output.len(),
                    })?;
            output
                .get_mut(completed..output_end)
                .ok_or_else(|| VmiError::ReadFailed {
                    address: address.raw(),
                    length: requested_length,
                })?
                .copy_from_slice(source);
            completed = output_end;
            current = current
                .checked_add(u128::try_from(length).unwrap_or(u128::MAX))
                .ok_or_else(|| VmiError::ReadFailed {
                    address: address.raw(),
                    length: output.len(),
                })?;
        }
        if completed == output.len() && current == end {
            return Ok(());
        }
        Err(VmiError::ReadFailed {
            address: address.raw(),
            length: output.len(),
        })
    }
}

fn ranges_overlap(left: MemoryRange, right: MemoryRange) -> bool {
    let left_start = u128::from(left.start.raw());
    let left_end = left_start.saturating_add(u128::from(left.length));
    let right_start = u128::from(right.start.raw());
    let right_end = right_start.saturating_add(u128::from(right.length));
    left_start < right_end && right_start < left_end
}

fn read_retry(reader: &mut impl Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    loop {
        match reader.read(buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => return result,
        }
    }
}

fn read_file_prefix(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(|error| {
        VmiError::Backend(format!("failed to open {}: {error}", path.display()))
    })?;
    let mut prefix = Vec::new();
    prefix.try_reserve_exact(maximum).map_err(|error| {
        VmiError::Backend(format!(
            "failed to allocate artifact prefix buffer: {error}"
        ))
    })?;
    let mut chunk = [0u8; 8192];
    while prefix.len() < maximum {
        let requested = maximum.saturating_sub(prefix.len()).min(chunk.len());
        let target = chunk
            .get_mut(..requested)
            .ok_or_else(|| VmiError::Backend("artifact prefix chunk is too small".into()))?;
        let count = read_retry(&mut file, target).map_err(|error| {
            VmiError::Backend(format!("failed to read {}: {error}", path.display()))
        })?;
        if count == 0 {
            break;
        }
        prefix.try_reserve(count).map_err(|error| {
            VmiError::Backend(format!("failed to grow artifact prefix buffer: {error}"))
        })?;
        let captured = chunk
            .get(..count)
            .ok_or_else(|| VmiError::Backend("artifact prefix read exceeded its buffer".into()))?;
        prefix.extend_from_slice(captured);
    }
    Ok(prefix)
}

fn reserve_segment(values: &mut Vec<SnapshotSegment>, format: &str) -> Result<()> {
    values.try_reserve(1).map_err(|error| {
        VmiError::Backend(format!("failed to grow {format} segment table: {error}"))
    })
}

fn read_file_limited(path: &Path, maximum: u64, description: &str) -> Result<Vec<u8>> {
    if maximum == 0 {
        return Err(VmiError::Backend(format!(
            "maximum {description} size must be non-zero"
        )));
    }
    let file = File::open(path).map_err(|error| {
        VmiError::Backend(format!("failed to open {}: {error}", path.display()))
    })?;
    let size = file
        .metadata()
        .map_err(|error| {
            VmiError::Backend(format!("failed to inspect {}: {error}", path.display()))
        })?
        .len();
    if size > maximum {
        return Err(VmiError::Backend(format!(
            "{description} size {size} exceeds {maximum} bytes"
        )));
    }
    let capacity = usize::try_from(size)
        .map_err(|_| VmiError::Backend(format!("{description} is too large for this host")))?;
    let mut data = Vec::new();
    data.try_reserve_exact(capacity).map_err(|error| {
        VmiError::Backend(format!("failed to allocate {description} buffer: {error}"))
    })?;
    let mut file = file;
    let capture_limit = maximum.saturating_add(1);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let captured = u64::try_from(data.len())
            .map_err(|_| VmiError::Backend(format!("{description} size does not fit u64")))?;
        let remaining = capture_limit.saturating_sub(captured);
        if remaining == 0 {
            let mut sentinel = [0u8; 1];
            if read_retry(&mut file, &mut sentinel).map_err(|error| {
                VmiError::Backend(format!("failed to read {}: {error}", path.display()))
            })? != 0
            {
                return Err(VmiError::Backend(format!(
                    "{description} grew beyond {maximum} bytes while being read"
                )));
            }
            break;
        }
        let chunk_capacity = u64::try_from(chunk.len())
            .map_err(|_| VmiError::Backend(format!("{description} chunk does not fit u64")))?;
        let requested = usize::try_from(remaining.min(chunk_capacity)).map_err(|_| {
            VmiError::Backend(format!(
                "{description} read chunk is too large for this host"
            ))
        })?;
        let target = chunk
            .get_mut(..requested)
            .ok_or_else(|| VmiError::Backend(format!("{description} read chunk is too small")))?;
        let count = read_retry(&mut file, target).map_err(|error| {
            VmiError::Backend(format!("failed to read {}: {error}", path.display()))
        })?;
        if count == 0 {
            break;
        }
        data.try_reserve(count).map_err(|error| {
            VmiError::Backend(format!("failed to grow {description} buffer: {error}"))
        })?;
        let captured = chunk.get(..count).ok_or_else(|| {
            VmiError::Backend(format!("{description} read exceeded its chunk buffer"))
        })?;
        data.extend_from_slice(captured);
        if u64::try_from(data.len()).map_or(true, |size| size > maximum) {
            return Err(VmiError::Backend(format!(
                "{description} grew beyond {maximum} bytes while being read"
            )));
        }
    }
    Ok(data)
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(read_array(data, offset)?))
}

fn add_offset(base: usize, delta: usize) -> Result<usize> {
    base.checked_add(delta)
        .ok_or_else(|| VmiError::Backend("artifact field offset overflow".into()))
}

fn decode_compressed_kdmp(
    mut decoder: impl Read,
    encoding: &str,
    max_decoded_size: u64,
) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    let capture_limit = max_decoded_size.saturating_add(1);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let captured = u64::try_from(decoded.len())
            .map_err(|_| VmiError::Backend("decoded KDMP size does not fit u64".into()))?;
        let remaining = capture_limit.saturating_sub(captured);
        if remaining == 0 {
            let mut sentinel = [0u8; 1];
            if read_retry(&mut decoder, &mut sentinel).map_err(|error| {
                VmiError::Backend(format!(
                    "failed to decode {encoding}-compressed KDMP: {error}"
                ))
            })? != 0
            {
                return Err(VmiError::Backend(format!(
                    "decoded {encoding} KDMP exceeds configured limit of {max_decoded_size} bytes"
                )));
            }
            break;
        }
        let chunk_capacity = u64::try_from(chunk.len())
            .map_err(|_| VmiError::Backend("decoded KDMP chunk does not fit u64".into()))?;
        let requested = usize::try_from(remaining.min(chunk_capacity)).map_err(|_| {
            VmiError::Backend("decoded KDMP chunk length does not fit this host".into())
        })?;
        let target = chunk
            .get_mut(..requested)
            .ok_or_else(|| VmiError::Backend("decoded KDMP chunk is too small".into()))?;
        let count = read_retry(&mut decoder, target).map_err(|error| {
            VmiError::Backend(format!(
                "failed to decode {encoding}-compressed KDMP: {error}"
            ))
        })?;
        if count == 0 {
            break;
        }
        decoded.try_reserve(count).map_err(|error| {
            VmiError::Backend(format!(
                "failed to grow decoded {encoding} KDMP buffer: {error}"
            ))
        })?;
        let captured = chunk.get(..count).ok_or_else(|| {
            VmiError::Backend("decoded KDMP read exceeded its chunk buffer".into())
        })?;
        decoded.extend_from_slice(captured);
        if u64::try_from(decoded.len()).map_or(true, |size| size > max_decoded_size) {
            return Err(VmiError::Backend(format!(
                "decoded {encoding} KDMP exceeds configured limit of {max_decoded_size} bytes"
            )));
        }
    }
    if !decoded.starts_with(b"PAGEDU64") {
        return Err(VmiError::Backend(format!(
            "{encoding} payload is not an AMD64 KDMP"
        )));
    }
    Ok(decoded)
}
fn read_u32(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(read_array(data, offset)?))
}
fn read_u64(data: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(read_array(data, offset)?))
}

fn read_array<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N]> {
    let end = offset
        .checked_add(N)
        .ok_or_else(|| VmiError::Backend("artifact field offset overflow".into()))?;
    let source = data
        .get(offset..end)
        .ok_or_else(|| VmiError::Backend("truncated artifact field".into()))?;
    let mut bytes = [0u8; N];
    bytes.copy_from_slice(source);
    Ok(bytes)
}

fn manifest_u64(value: Option<&Value>, field: &str, index: usize) -> Result<u64> {
    let value = value.ok_or_else(|| {
        VmiError::Backend(format!(
            "snapshot manifest segment {index} {field} is missing"
        ))
    })?;
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    let text = value.as_str().ok_or_else(|| {
        VmiError::Backend(format!(
            "snapshot manifest segment {index} {field} must be unsigned"
        ))
    })?;
    if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        text.parse()
    }
    .map_err(|error| {
        VmiError::Backend(format!(
            "invalid snapshot manifest segment {index} {field}: {error}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bzip2::write::BzEncoder;
    use flate2::{
        write::{GzEncoder, ZlibEncoder},
        Compression,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::{fs, io::Write};
    use xz2::write::XzEncoder;

    struct InterruptOnce<R> {
        inner: R,
        interrupted: bool,
    }

    impl<R: Read> Read for InterruptOnce<R> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            self.inner.read(buffer)
        }
    }

    #[test]
    fn bounded_reader_retries_interrupted_reads() {
        let mut reader = InterruptOnce {
            inner: &b"VMI!"[..],
            interrupted: false,
        };
        let mut output = [0; 4];
        assert_eq!(read_retry(&mut reader, &mut output).unwrap(), 4);
        assert_eq!(&output, b"VMI!");
    }

    #[test]
    fn fixed_width_reads_reject_truncation_and_offset_overflow() {
        assert_eq!(read_u16(&[0x34, 0x12], 0).unwrap(), 0x1234);
        assert!(read_u16(&[0], 0).is_err());
        assert!(read_u32(&[], usize::MAX).is_err());
        assert!(read_u64(&[], usize::MAX - 3).is_err());
    }

    #[test]
    fn metadata_and_format_detection_bound_prefix_reads() {
        let directory = manifest_dir();
        let dump = directory.join("large.dmp");
        let mut header = vec![0u8; 0x2000];
        header[..8].copy_from_slice(b"PAGEDU64");
        header[0x30..0x34].copy_from_slice(&0x8664u32.to_le_bytes());
        let mut file = File::create(&dump).unwrap();
        file.write_all(&header).unwrap();
        file.set_len(1024 * 1024 * 1024).unwrap();

        assert_eq!(KdmpMetadata::from_file(&dump).unwrap().machine_type, 0x8664);
        let unknown = directory.join("large.unknown");
        let mut file = File::create(&unknown).unwrap();
        file.write_all(b"UNKNOWN!").unwrap();
        file.set_len(1024 * 1024 * 1024).unwrap();
        assert!(SnapshotBundle::converted_core_file(&unknown).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reads_raw_ranges_and_rejects_out_of_bounds() {
        let bundle = SnapshotBundle::from_raw("fixture", Gpa::new(0x1000), Arc::from([1, 2, 3, 4]));
        let mut bytes = [0; 2];
        bundle.read_into(Gpa::new(0x1001), &mut bytes).unwrap();
        assert_eq!(bytes, [2, 3]);
        assert!(matches!(
            bundle.read_into(Gpa::new(0x1003), &mut bytes),
            Err(VmiError::ReadFailed { .. })
        ));
    }

    #[test]
    fn reads_the_final_physical_address_without_wrapping() {
        let bundle = SnapshotBundle::from_raw(
            "final-byte.raw",
            Gpa::new(u64::MAX),
            Arc::<[u8]>::from([0xa5]),
        );
        let mut byte = [0];
        bundle.read_into(Gpa::new(u64::MAX), &mut byte).unwrap();
        assert_eq!(byte, [0xa5]);

        let mut overflow = [0xaa; 2];
        assert!(matches!(
            bundle.read_into(Gpa::new(u64::MAX), &mut overflow),
            Err(VmiError::ReadFailed { .. })
        ));
        assert_eq!(overflow, [0xaa; 2]);

        assert!(SnapshotBundle::try_from_raw(
            "overflow.raw",
            Gpa::new(u64::MAX),
            Arc::<[u8]>::from([1, 2]),
        )
        .is_err());
        assert!(SnapshotBundle::try_from_raw(
            "valid-final.raw",
            Gpa::new(u64::MAX),
            Arc::<[u8]>::from([1]),
        )
        .is_ok());
    }

    #[test]
    fn widened_overlap_check_handles_the_address_space_ceiling() {
        let covering_final_byte = MemoryRange::new(Gpa::new(u64::MAX - 1), 2);
        let final_byte = MemoryRange::new(Gpa::new(u64::MAX), 1);
        let preceding_byte = MemoryRange::new(Gpa::new(u64::MAX - 2), 1);
        assert!(ranges_overlap(covering_final_byte, final_byte));
        assert!(!ranges_overlap(preceding_byte, final_byte));
    }

    #[test]
    fn reads_across_contiguous_segments_and_rejects_holes() {
        let contiguous = SnapshotBundle {
            provenance: ArtifactProvenance {
                format: "fixture".into(),
                source: "fixture".into(),
            },
            segments: vec![
                SnapshotSegment::new(Gpa::new(0x1000), Arc::<[u8]>::from([1, 2])),
                SnapshotSegment::new(Gpa::new(0x1002), Arc::<[u8]>::from([3, 4])),
            ],
        };
        let mut bytes = [0; 4];
        contiguous.read_into(Gpa::new(0x1000), &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4]);

        let mut sparse = contiguous.clone();
        sparse.segments[1].range.start = Gpa::new(0x1003);
        bytes = [0xaa; 4];
        assert!(sparse.read_into(Gpa::new(0x1000), &mut bytes).is_err());
        assert_eq!(bytes, [0xaa; 4]);
    }

    #[test]
    fn parses_elf64_load_segments_and_zero_fills_memory_tail() {
        let mut elf = vec![0u8; 132];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1u16.to_le_bytes());
        elf[64..68].copy_from_slice(&1u32.to_le_bytes());
        elf[72..80].copy_from_slice(&128u64.to_le_bytes());
        elf[88..96].copy_from_slice(&0x2000u64.to_le_bytes());
        elf[96..104].copy_from_slice(&4u64.to_le_bytes());
        elf[104..112].copy_from_slice(&8u64.to_le_bytes());
        elf[128..132].copy_from_slice(&[1, 2, 3, 4]);
        let bundle = SnapshotBundle::from_elf_vmcore("fixture.elf", &elf).unwrap();
        let mut output = [0xff; 8];
        bundle.read_into(Gpa::new(0x2000), &mut output).unwrap();
        assert_eq!(output, [1, 2, 3, 4, 0, 0, 0, 0]);
        assert_eq!(bundle.provenance.format, "elf-vmcore");
    }

    #[test]
    fn rejects_elf_file_and_decoded_memory_over_limits() {
        let mut elf = vec![0u8; 132];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1u16.to_le_bytes());
        elf[64..68].copy_from_slice(&1u32.to_le_bytes());
        elf[72..80].copy_from_slice(&128u64.to_le_bytes());
        elf[96..104].copy_from_slice(&4u64.to_le_bytes());
        elf[104..112].copy_from_slice(&(1024u64 * 1024 * 1024).to_le_bytes());
        assert!(SnapshotBundle::from_elf_with_limits("large.elf", &elf, "test", 4096).is_err());

        let directory = manifest_dir();
        let path = directory.join("sparse.elf");
        let file = File::create(&path).unwrap();
        file.set_len(1024 * 1024 * 1024).unwrap();
        assert!(SnapshotBundle::elf_vmcore_file_with_limits(&path, 4096, 4096).is_err());
        assert!(SnapshotBundle::elf_vmcore_file_with_limits(&path, 0, 4096).is_err());
        assert!(SnapshotBundle::elf_vmcore_file_with_limits(&path, 4096, 0).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_oversized_raw_lime_xen_and_kdmp_files_before_reading() {
        let directory = manifest_dir();
        let path = directory.join("sparse.bin");
        let file = File::create(&path).unwrap();
        file.set_len(1024 * 1024 * 1024).unwrap();

        assert!(SnapshotBundle::raw_file_with_limit(&path, Gpa::new(0), 4096).is_err());
        assert!(SnapshotBundle::lime_file_with_limit(&path, 4096).is_err());
        assert!(SnapshotBundle::xen_core_file_with_limits(&path, 4096, 4096).is_err());
        assert!(SnapshotBundle::kdmp_file_with_limit(&path, 4096).is_err());
        for maximum in [0, 4096] {
            assert!(SnapshotBundle::raw_file_with_limit(&path, Gpa::new(0), maximum).is_err());
            assert!(SnapshotBundle::lime_file_with_limit(&path, maximum).is_err());
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_truncated_elf_segments() {
        let mut elf = vec![0u8; 120];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1u16.to_le_bytes());
        elf[64..68].copy_from_slice(&1u32.to_le_bytes());
        elf[72..80].copy_from_slice(&200u64.to_le_bytes());
        elf[96..104].copy_from_slice(&4u64.to_le_bytes());
        elf[104..112].copy_from_slice(&4u64.to_le_bytes());
        assert!(SnapshotBundle::from_elf_vmcore("bad.elf", &elf).is_err());
    }

    #[test]
    fn normalizes_xen_core_physical_ranges_with_specific_provenance() {
        let mut elf = vec![0u8; 132];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1u16.to_le_bytes());
        elf[64..68].copy_from_slice(&1u32.to_le_bytes());
        elf[72..80].copy_from_slice(&128u64.to_le_bytes());
        elf[88..96].copy_from_slice(&0x4000u64.to_le_bytes());
        elf[96..104].copy_from_slice(&4u64.to_le_bytes());
        elf[104..112].copy_from_slice(&4u64.to_le_bytes());
        elf[128..132].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let bundle = SnapshotBundle::from_xen_core("domain.core", &elf).unwrap();
        let mut output = [0; 4];
        bundle.read_into(Gpa::new(0x4000), &mut output).unwrap();
        assert_eq!(output, [0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(bundle.provenance.format, "xen-core");
    }

    fn append_lime_range(output: &mut Vec<u8>, start: u64, bytes: &[u8]) {
        output.extend_from_slice(&0x4c69_4d45u32.to_le_bytes());
        output.extend_from_slice(&1u32.to_le_bytes());
        output.extend_from_slice(&start.to_le_bytes());
        output.extend_from_slice(&(start + bytes.len() as u64 - 1).to_le_bytes());
        output.extend_from_slice(&0u64.to_le_bytes());
        output.extend_from_slice(bytes);
    }

    #[test]
    fn parses_multiple_lime_ranges() {
        let mut lime = Vec::new();
        append_lime_range(&mut lime, 0x1000, &[1, 2, 3, 4]);
        append_lime_range(&mut lime, 0x3000, &[5, 6]);
        let bundle = SnapshotBundle::from_lime("fixture.lime", &lime).unwrap();
        assert_eq!(bundle.segments().len(), 2);
        let mut first = [0; 4];
        bundle.read_into(Gpa::new(0x1000), &mut first).unwrap();
        assert_eq!(first, [1, 2, 3, 4]);
        let mut second = [0; 2];
        bundle.read_into(Gpa::new(0x3000), &mut second).unwrap();
        assert_eq!(second, [5, 6]);
        assert!(bundle.read_into(Gpa::new(0x2000), &mut second).is_err());
        assert_eq!(bundle.provenance.format, "lime");
    }

    #[test]
    fn rejects_truncated_and_overlapping_lime_ranges() {
        let mut truncated = Vec::new();
        append_lime_range(&mut truncated, 0x1000, &[1, 2, 3]);
        truncated.pop();
        assert!(SnapshotBundle::from_lime("truncated.lime", &truncated).is_err());

        let mut overlapping = Vec::new();
        append_lime_range(&mut overlapping, 0x1000, &[0; 4]);
        append_lime_range(&mut overlapping, 0x1002, &[0; 4]);
        assert!(SnapshotBundle::from_lime("overlap.lime", &overlapping).is_err());
    }

    fn kdmp(runs: &[(u64, u64)]) -> Vec<u8> {
        let pages: u64 = runs.iter().map(|(_, count)| count).sum();
        let mut data = vec![0u8; 0x2000 + pages as usize * 4096];
        data[..8].copy_from_slice(b"PAGEDU64");
        data[0x30..0x34].copy_from_slice(&0x8664u32.to_le_bytes());
        data[0x88..0x8c].copy_from_slice(&(runs.len() as u32).to_le_bytes());
        data[0x90..0x98].copy_from_slice(&pages.to_le_bytes());
        let mut payload = 0x2000;
        for (index, (base, count)) in runs.iter().enumerate() {
            let offset = 0x98 + index * 16;
            data[offset..offset + 8].copy_from_slice(&base.to_le_bytes());
            data[offset + 8..offset + 16].copy_from_slice(&count.to_le_bytes());
            for page in 0..*count {
                data[payload..payload + 4096].fill((base + page) as u8);
                payload += 4096;
            }
        }
        data
    }

    #[test]
    fn parses_legacy_amd64_kdmp_physical_runs() {
        let data = kdmp(&[(1, 2), (8, 1)]);
        let bundle = SnapshotBundle::from_kdmp("memory.dmp", &data).unwrap();
        assert_eq!(bundle.provenance.format, "windows-kdmp");
        assert_eq!(bundle.segments().len(), 2);
        let mut first = [0; 2];
        bundle.read_into(Gpa::new(0x1000), &mut first).unwrap();
        assert_eq!(first, [1, 1]);
        let mut second_page = [0; 1];
        bundle
            .read_into(Gpa::new(0x2000), &mut second_page)
            .unwrap();
        assert_eq!(second_page, [2]);
        let mut high = [0; 1];
        bundle.read_into(Gpa::new(0x8000), &mut high).unwrap();
        assert_eq!(high, [8]);
    }

    #[test]
    fn parses_legacy_kdmp_bootstrap_metadata() {
        let mut data = kdmp(&[(1, 1)]);
        data[0x08..0x0c].copy_from_slice(&10u32.to_le_bytes());
        data[0x0c..0x10].copy_from_slice(&22621u32.to_le_bytes());
        data[0x10..0x18].copy_from_slice(&0x1aa000u64.to_le_bytes());
        data[0x18..0x20].copy_from_slice(&0xffff_f800_0100_0000u64.to_le_bytes());
        data[0x20..0x28].copy_from_slice(&0xffff_f800_0200_0000u64.to_le_bytes());
        data[0x28..0x30].copy_from_slice(&0xffff_f800_0300_0000u64.to_le_bytes());
        data[0x34..0x38].copy_from_slice(&8u32.to_le_bytes());
        data[0x38..0x3c].copy_from_slice(&0xdead_u32.to_le_bytes());
        for (index, value) in [1u64, 2, 3, 4].into_iter().enumerate() {
            let offset = 0x40 + index * 8;
            data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        data[0x80..0x88].copy_from_slice(&0xffff_f800_0400_0000u64.to_le_bytes());
        let metadata = KdmpMetadata::parse(&data).unwrap();
        assert_eq!(
            (metadata.major_version, metadata.minor_version),
            (10, 22621)
        );
        assert_eq!(metadata.directory_table_base, 0x1aa000);
        assert_eq!(metadata.processor_count, 8);
        assert_eq!(metadata.bugcheck_code, 0xdead);
        assert_eq!(metadata.bugcheck_parameters, [1, 2, 3, 4]);
        assert_eq!(metadata.active_process_head, 0xffff_f800_0300_0000);
    }

    #[test]
    fn rejects_invalid_truncated_and_overlapping_kdmp() {
        let mut invalid = kdmp(&[(1, 1)]);
        invalid[4..8].copy_from_slice(b"DUMP");
        assert!(SnapshotBundle::from_kdmp("bad.dmp", &invalid).is_err());
        let mut truncated = kdmp(&[(1, 1)]);
        truncated.pop();
        assert!(SnapshotBundle::from_kdmp("short.dmp", &truncated).is_err());
        assert!(SnapshotBundle::from_kdmp("overlap.dmp", &kdmp(&[(1, 2), (2, 1)])).is_err());
        let mut mismatch = kdmp(&[(1, 1)]);
        mismatch[0x90..0x98].copy_from_slice(&2u64.to_le_bytes());
        assert!(SnapshotBundle::from_kdmp("mismatch.dmp", &mismatch).is_err());
    }

    #[test]
    fn loads_common_compressed_kdmp_wrappers() {
        let source = kdmp(&[(1, 1)]);
        let directory = manifest_dir();
        for (name, compressed) in [
            ("memory.dmp.gz", {
                let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
                encoder.write_all(&source).unwrap();
                encoder.finish().unwrap()
            }),
            ("memory.dmp.zlib", {
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
                encoder.write_all(&source).unwrap();
                encoder.finish().unwrap()
            }),
            ("memory.dmp.bz2", {
                let mut encoder = BzEncoder::new(Vec::new(), bzip2::Compression::fast());
                encoder.write_all(&source).unwrap();
                encoder.finish().unwrap()
            }),
            ("memory.dmp.xz", {
                let mut encoder = XzEncoder::new(Vec::new(), 1);
                encoder.write_all(&source).unwrap();
                encoder.finish().unwrap()
            }),
            (
                "memory.dmp.zst",
                zstd::stream::encode_all(source.as_slice(), 1).unwrap(),
            ),
        ] {
            let path = directory.join(name);
            fs::write(&path, compressed).unwrap();
            let bundle = SnapshotBundle::kdmp_file(path).unwrap();
            let mut byte = [0];
            bundle.read_into(Gpa::new(0x1000), &mut byte).unwrap();
            assert_eq!(byte, [1]);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bounds_all_compressed_kdmp_wrappers() {
        let source = kdmp(&[(1, 1)]);
        let directory = manifest_dir();
        let wrappers = [
            ("bomb.gz", {
                let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
                encoder.write_all(&source).unwrap();
                encoder.finish().unwrap()
            }),
            ("bomb.zlib", {
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
                encoder.write_all(&source).unwrap();
                encoder.finish().unwrap()
            }),
            ("bomb.bz2", {
                let mut encoder = BzEncoder::new(Vec::new(), bzip2::Compression::fast());
                encoder.write_all(&source).unwrap();
                encoder.finish().unwrap()
            }),
            ("bomb.xz", {
                let mut encoder = XzEncoder::new(Vec::new(), 1);
                encoder.write_all(&source).unwrap();
                encoder.finish().unwrap()
            }),
            (
                "bomb.zst",
                zstd::stream::encode_all(source.as_slice(), 1).unwrap(),
            ),
        ];
        for (name, compressed) in wrappers {
            let path = directory.join(name);
            fs::write(&path, compressed).unwrap();
            assert!(SnapshotBundle::kdmp_file_with_limit(&path, source.len() as u64).is_ok());
            assert!(SnapshotBundle::kdmp_file_with_limit(&path, source.len() as u64 - 1).is_err());
            assert!(SnapshotBundle::kdmp_file_with_limit(&path, 0).is_err());
        }
        fs::remove_dir_all(directory).unwrap();
    }

    fn bitmap_kdmp(pfns: &[u64], page_count: u64) -> Vec<u8> {
        let bitmap_len = page_count.div_ceil(8) as usize;
        let first_page = (0x2038 + bitmap_len + 0xfff) & !0xfff;
        let mut data = vec![0u8; first_page + pfns.len() * 4096];
        data[..8].copy_from_slice(b"PAGEDU64");
        data[0x30..0x34].copy_from_slice(&0x8664u32.to_le_bytes());
        data[0x2000..0x2008].copy_from_slice(b"SDMPDUMP");
        data[0x2020..0x2028].copy_from_slice(&(first_page as u64).to_le_bytes());
        data[0x2028..0x2030].copy_from_slice(&(pfns.len() as u64).to_le_bytes());
        data[0x2030..0x2038].copy_from_slice(&page_count.to_le_bytes());
        for (index, pfn) in pfns.iter().copied().enumerate() {
            data[0x2038 + pfn as usize / 8] |= 1 << (pfn % 8);
            data[first_page + index * 4096..first_page + (index + 1) * 4096].fill(pfn as u8);
        }
        data
    }

    #[test]
    fn parses_bitmap_kdmp_sparse_physical_pages() {
        let data = bitmap_kdmp(&[1, 3, 10], 11);
        let bundle = SnapshotBundle::from_kdmp("bitmap.dmp", &data).unwrap();
        assert_eq!(bundle.provenance.format, "windows-kdmp-bitmap");
        assert_eq!(bundle.segments().len(), 3);
        let mut byte = [0];
        bundle.read_into(Gpa::new(0xa000), &mut byte).unwrap();
        assert_eq!(byte, [10]);
        assert!(bundle.read_into(Gpa::new(0x2000), &mut byte).is_err());

        let adjacent = bitmap_kdmp(&[3, 4], 5);
        let bundle = SnapshotBundle::from_kdmp("adjacent.dmp", &adjacent).unwrap();
        let mut boundary = [0; 2];
        bundle.read_into(Gpa::new(0x3fff), &mut boundary).unwrap();
        assert_eq!(boundary, [3, 4]);
    }

    #[test]
    fn rejects_corrupt_bitmap_kdmp() {
        let mut mismatch = bitmap_kdmp(&[1], 2);
        mismatch[0x2028..0x2030].copy_from_slice(&2u64.to_le_bytes());
        assert!(SnapshotBundle::from_kdmp("mismatch.dmp", &mismatch).is_err());
        let mut truncated = bitmap_kdmp(&[1], 2);
        truncated.pop();
        assert!(SnapshotBundle::from_kdmp("truncated.dmp", &truncated).is_err());
    }

    fn rdmp_kdmp(ranges: &[(u64, u64)]) -> Vec<u8> {
        let metadata_size = 0x20 + ranges.len() as u64 * 16;
        let first_page = 0x2020 + metadata_size;
        let pages: u64 = ranges.iter().map(|(_, count)| count).sum();
        let mut data = vec![0u8; first_page as usize + pages as usize * 4096];
        data[..8].copy_from_slice(b"PAGEDU64");
        data[0x30..0x34].copy_from_slice(&0x8664u32.to_le_bytes());
        data[0x2000..0x2004].copy_from_slice(&0x40u32.to_le_bytes());
        data[0x2004..0x200c].copy_from_slice(b"RDMPDUMP");
        data[0x2010..0x2018].copy_from_slice(&metadata_size.to_le_bytes());
        data[0x2018..0x2020].copy_from_slice(&first_page.to_le_bytes());
        let mut payload = first_page as usize;
        for (index, (base, count)) in ranges.iter().copied().enumerate() {
            let offset = 0x2030 + index * 16;
            data[offset..offset + 8].copy_from_slice(&base.to_le_bytes());
            data[offset + 8..offset + 16].copy_from_slice(&count.to_le_bytes());
            for page in 0..count {
                data[payload..payload + 4096].fill((base + page) as u8);
                payload += 4096;
            }
        }
        data
    }

    #[test]
    fn parses_rdmp_kdmp_physical_ranges() {
        let data = rdmp_kdmp(&[(2, 2), (9, 1)]);
        let bundle = SnapshotBundle::from_kdmp("active.dmp", &data).unwrap();
        assert_eq!(bundle.provenance.format, "windows-kdmp-rdmp");
        let mut byte = [0];
        bundle.read_into(Gpa::new(0x3000), &mut byte).unwrap();
        assert_eq!(byte, [3]);
        bundle.read_into(Gpa::new(0x9000), &mut byte).unwrap();
        assert_eq!(byte, [9]);
    }

    #[test]
    fn rejects_corrupt_rdmp_kdmp() {
        let mut truncated = rdmp_kdmp(&[(2, 1)]);
        truncated.pop();
        assert!(SnapshotBundle::from_kdmp("short.dmp", &truncated).is_err());
        let overlapping = rdmp_kdmp(&[(2, 2), (3, 1)]);
        assert!(SnapshotBundle::from_kdmp("overlap.dmp", &overlapping).is_err());
    }

    fn manifest_dir() -> PathBuf {
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("vmi-artifact-{}-{suffix}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn loads_multi_file_snapshot_manifest() {
        let directory = manifest_dir();
        fs::write(directory.join("low.bin"), [0, 1, 2, 3, 4]).unwrap();
        fs::write(directory.join("high.bin"), [9, 8, 7, 6]).unwrap();
        let manifest = directory.join("snapshot.json");
        fs::write(
            &manifest,
            r#"{
            "version": 1,
            "format": "firecracker-snapshot",
            "segments": [
                {"file":"high.bin","file_offset":1,"length":2,"gpa":"0x3000"},
                {"file":"low.bin","file_offset":2,"length":3,"gpa":"0x1000"}
            ]
        }"#,
        )
        .unwrap();
        let bundle = SnapshotBundle::manifest_file(&manifest).unwrap();
        assert_eq!(bundle.provenance.format, "firecracker-snapshot");
        assert_eq!(bundle.segments()[0].range.start, Gpa::new(0x1000));
        let mut low = [0; 3];
        bundle.read_into(Gpa::new(0x1000), &mut low).unwrap();
        assert_eq!(low, [2, 3, 4]);
        let mut high = [0; 2];
        bundle.read_into(Gpa::new(0x3000), &mut high).unwrap();
        assert_eq!(high, [8, 7]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_invalid_snapshot_manifests() {
        let directory = manifest_dir();
        fs::write(directory.join("memory.bin"), [0; 8]).unwrap();
        let manifest = directory.join("snapshot.json");
        fs::write(&manifest, r#"{"version":2,"format":"x","segments":[]}"#).unwrap();
        assert!(SnapshotBundle::manifest_file(&manifest).is_err());
        fs::write(&manifest, r#"{"version":1,"format":"x","segments":[{"file":"memory.bin","file_offset":7,"length":2,"gpa":0}]}"#).unwrap();
        assert!(SnapshotBundle::manifest_file(&manifest).is_err());
        fs::write(&manifest, r#"{"version":1,"format":"x","segments":[{"file":"memory.bin","file_offset":0,"length":4,"gpa":0},{"file":"memory.bin","file_offset":4,"length":4,"gpa":2}]}"#).unwrap();
        assert!(SnapshotBundle::manifest_file(&manifest).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn confines_manifest_files_to_their_directory() {
        let root = manifest_dir();
        let directory = root.join("snapshot");
        fs::create_dir(&directory).unwrap();
        let outside = root.join("outside.bin");
        fs::write(&outside, [1, 2, 3, 4]).unwrap();
        let manifest = directory.join("snapshot.json");
        let write_manifest = |file: &str| {
            fs::write(
                &manifest,
                serde_json::to_vec(&serde_json::json!({
                    "version": 1,
                    "format": "confined",
                    "segments": [{
                        "file": file,
                        "file_offset": 0,
                        "length": 4,
                        "gpa": 0
                    }]
                }))
                .unwrap(),
            )
            .unwrap();
        };
        write_manifest("../outside.bin");
        assert!(SnapshotBundle::manifest_file(&manifest).is_err());
        write_manifest(outside.to_str().unwrap());
        assert!(SnapshotBundle::manifest_file(&manifest).is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, directory.join("escape.bin")).unwrap();
            write_manifest("escape.bin");
            assert!(SnapshotBundle::manifest_file(&manifest).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_only_manifest_slices_and_enforces_resource_limits() {
        let directory = manifest_dir();
        let memory = directory.join("sparse.bin");
        let mut file = File::create(&memory).unwrap();
        file.set_len(1024 * 1024 * 1024).unwrap();
        file.seek(SeekFrom::Start(1024 * 1024 * 1024 - 4)).unwrap();
        file.write_all(&[1, 2, 3, 4]).unwrap();
        drop(file);
        let manifest = directory.join("snapshot.json");
        fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "format": "sparse-test",
                "segments": [
                    {"file":"sparse.bin", "file_offset": 1024_u64 * 1024 * 1024 - 4, "length":2, "gpa":0x1000},
                    {"file":"sparse.bin", "file_offset": 1024_u64 * 1024 * 1024 - 2, "length":2, "gpa":0x2000}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let bundle = SnapshotBundle::manifest_file_with_limits(&manifest, 2, 4).unwrap();
        let mut first = [0; 2];
        bundle.read_into(Gpa::new(0x1000), &mut first).unwrap();
        assert_eq!(first, [1, 2]);
        assert!(SnapshotBundle::manifest_file_with_limits(&manifest, 1, 4).is_err());
        assert!(SnapshotBundle::manifest_file_with_limits(&manifest, 2, 3).is_err());
        assert!(SnapshotBundle::manifest_file_with_limits(&manifest, 0, 4).is_err());
        assert!(SnapshotBundle::manifest_file_with_limits(&manifest, 2, 0).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
