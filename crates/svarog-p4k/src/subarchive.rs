//! P4K subarchive trailer helpers.
//!
//! CigDataPatcher has a separate `cig_p4k_subarchive` subsystem for
//! `.p4k_subarchive.zst` payloads. These helpers pin the dump-backed
//! EOCDR-section facts without folding subarchives into the main v1/v2
//! archive reader.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::{zip::CompressionMethod, Error, Result};

/// Size returned by `cig_p4k_subarchive::GetSubArchiveEOCDRSize`.
pub const SUBARCHIVE_EOCDR_SIZE: usize = 0x1062;

const SUBARCHIVE_MARKER_OFFSET: usize = 0x38;
const SUBARCHIVE_MARKER: u16 = 0x5005;
const SUBARCHIVE_VERSION_OFFSET: usize = 0x3c;
const SUBARCHIVE_FLAGS_OFFSET: usize = 0x40;
const SUBARCHIVE_ENTRY_COUNT_OFFSET: usize = 0x20;
const SUBARCHIVE_CDR_SIZE_OFFSET: usize = 0x28;
const SUBARCHIVE_CDR_OFFSET_OFFSET: usize = 0x30;
const SUBARCHIVE_NAME_BYTES_OFFSET: usize = 0xc8;
const SUBARCHIVE_CDR_FIXED_NAME_LEN_OFFSET: usize = 0x1c;
const SUBARCHIVE_CDR_RECORD_OVERHEAD: usize = 0xf6;
const SUBARCHIVE_CDR_UNCOMPRESSED_SIZE_BASE: usize = 0x32;
const SUBARCHIVE_CDR_COMPRESSED_SIZE_BASE: usize = 0x3a;
const SUBARCHIVE_CDR_PAYLOAD_RELATIVE_BASE: usize = 0x42;
const SUBARCHIVE_V1_CRC_OFFSET: usize = 0x10;
const SUBARCHIVE_V1_SIGNATURE_BASE: usize = 0x52;
const SUBARCHIVE_V1_SHA256_BASE: usize = 0xd6;
const SUBARCHIVE_V2_CRC_BASE: usize = 0x52;
const SUBARCHIVE_V2_SHA256_BASE: usize = 0x56;
const SUBARCHIVE_V2_SIGNATURE_BASE: usize = 0x76;
const SUBARCHIVE_SIGNATURE_SIZE: usize = 128;
const SUBARCHIVE_SHA256_SIZE: usize = 32;

/// Status from `cig_p4k_subarchive::IsValidSubArchive`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubArchiveStatus {
    Successful,
    ErrorIncompleteSubArchive,
    ErrorNotASubArchive,
}

/// Subarchive CDR metadata read from the EOCDR section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubArchiveCdrInfo {
    pub number_of_entries: u64,
    pub cdr_offset: u64,
    pub cdr_size: u64,
    pub cdr_size_including_eocdr: u64,
    pub total_name_bytes_including_nul: u64,
    pub version: u32,
}

/// One CDR entry from `EnumerateSubArchiveCDREntries`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubArchiveCdrEntry {
    pub name: String,
    pub compression_method: CompressionMethod,
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub offset_to_file_data: u64,
    pub last_mod_file_time: u16,
    pub last_mod_file_date: u16,
    pub signature: [u8; SUBARCHIVE_SIGNATURE_SIZE],
    pub sha256: [u8; SUBARCHIVE_SHA256_SIZE],
}

/// Parsed P4K subarchive metadata and CDR entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubArchive {
    pub cdr_info: SubArchiveCdrInfo,
    pub entries: Vec<SubArchiveCdrEntry>,
}

/// Equivalent to `GetEOCDRBeginOffset`, but checked for undersized input.
pub fn subarchive_eocdr_begin_offset(subarchive_size: u64) -> Option<u64> {
    subarchive_size.checked_sub(SUBARCHIVE_EOCDR_SIZE as u64)
}

/// Equivalent to `GetSubArchiveEOCDRSize`.
pub fn subarchive_eocdr_size() -> usize {
    SUBARCHIVE_EOCDR_SIZE
}

/// Equivalent to `IsValidSubArchive`.
pub fn validate_subarchive_eocdr(section: &[u8]) -> SubArchiveStatus {
    if section.len() < SUBARCHIVE_EOCDR_SIZE {
        return SubArchiveStatus::ErrorNotASubArchive;
    }
    if read_u16(section, SUBARCHIVE_MARKER_OFFSET) != Some(SUBARCHIVE_MARKER) {
        return SubArchiveStatus::ErrorNotASubArchive;
    }
    if section[SUBARCHIVE_FLAGS_OFFSET] & 1 != 0 {
        SubArchiveStatus::ErrorIncompleteSubArchive
    } else {
        SubArchiveStatus::Successful
    }
}

/// Equivalent to `GetSubArchiveCDRInformation`, with bounds checks.
pub fn parse_subarchive_cdr_info(section: &[u8]) -> Result<SubArchiveCdrInfo> {
    match validate_subarchive_eocdr(section) {
        SubArchiveStatus::Successful => {}
        SubArchiveStatus::ErrorIncompleteSubArchive => {
            return Err(Error::MalformedSubArchive(
                "incomplete P4K subarchive".to_string(),
            ));
        }
        SubArchiveStatus::ErrorNotASubArchive => {
            return Err(Error::MalformedSubArchive(
                "not a P4K subarchive EOCDR section".to_string(),
            ));
        }
    }

    let cdr_size = read_u64_required(section, SUBARCHIVE_CDR_SIZE_OFFSET)?;
    Ok(SubArchiveCdrInfo {
        number_of_entries: read_u64_required(section, SUBARCHIVE_ENTRY_COUNT_OFFSET)?,
        cdr_offset: read_u64_required(section, SUBARCHIVE_CDR_OFFSET_OFFSET)?,
        cdr_size,
        cdr_size_including_eocdr: cdr_size
            .checked_add(SUBARCHIVE_EOCDR_SIZE as u64)
            .ok_or_else(|| {
                Error::MalformedSubArchive("subarchive CDR size overflow".to_string())
            })?,
        total_name_bytes_including_nul: read_u64_required(section, SUBARCHIVE_NAME_BYTES_OFFSET)?,
        version: read_u32_required(section, SUBARCHIVE_VERSION_OFFSET)?,
    })
}

/// Parse a complete P4K subarchive byte slice.
///
/// This follows the dump's `GetEOCDRBeginOffset` convention: the EOCDR
/// section begins exactly `0x1062` bytes before EOF.
pub fn parse_subarchive(data: &[u8]) -> Result<SubArchive> {
    let eocdr_start = data
        .len()
        .checked_sub(SUBARCHIVE_EOCDR_SIZE)
        .ok_or_else(|| {
            Error::MalformedSubArchive(format!(
                "subarchive has {} bytes, need at least {}",
                data.len(),
                SUBARCHIVE_EOCDR_SIZE
            ))
        })?;
    let eocdr_section = &data[eocdr_start..];
    let cdr_info = parse_subarchive_cdr_info(eocdr_section)?;
    let cdr_start = usize::try_from(cdr_info.cdr_offset).map_err(|_| {
        Error::MalformedSubArchive("subarchive CDR offset overflows usize".to_string())
    })?;
    let cdr_size = usize::try_from(cdr_info.cdr_size).map_err(|_| {
        Error::MalformedSubArchive("subarchive CDR size overflows usize".to_string())
    })?;
    let cdr_end = cdr_start
        .checked_add(cdr_size)
        .ok_or_else(|| Error::MalformedSubArchive("subarchive CDR range overflow".to_string()))?;
    if cdr_end != eocdr_start {
        return Err(Error::MalformedSubArchive(format!(
            "subarchive CDR ends at {cdr_end}, expected EOCDR start {eocdr_start}"
        )));
    }
    let cdr = data.get(cdr_start..cdr_end).ok_or_else(|| {
        Error::MalformedSubArchive(format!(
            "subarchive CDR range [{cdr_start}, {cdr_end}) extends past file size {}",
            data.len()
        ))
    })?;
    let entries = parse_subarchive_cdr_entries(cdr, cdr_info.version as u64)?;
    if entries.len() as u64 != cdr_info.number_of_entries {
        return Err(Error::MalformedSubArchive(format!(
            "subarchive CDR entry count {}, expected {}",
            entries.len(),
            cdr_info.number_of_entries
        )));
    }

    Ok(SubArchive { cdr_info, entries })
}

/// Parse a P4K subarchive from a seekable reader without reading payload bytes.
///
/// The dump's subarchive helpers locate all metadata from the fixed EOCDR
/// section at EOF plus the declared CDR range. Payload bytes before the CDR are
/// not needed to enumerate entries, so this path seeks directly to the EOCDR
/// and CDR instead of requiring a full in-memory subarchive buffer.
pub fn parse_subarchive_from_reader<R: Read + Seek>(reader: &mut R) -> Result<SubArchive> {
    let file_size = reader.seek(SeekFrom::End(0))?;
    let eocdr_start = subarchive_eocdr_begin_offset(file_size).ok_or_else(|| {
        Error::MalformedSubArchive(format!(
            "subarchive has {file_size} bytes, need at least {SUBARCHIVE_EOCDR_SIZE}"
        ))
    })?;

    reader.seek(SeekFrom::Start(eocdr_start))?;
    let mut eocdr_section = vec![0u8; SUBARCHIVE_EOCDR_SIZE];
    reader.read_exact(&mut eocdr_section)?;
    let cdr_info = parse_subarchive_cdr_info(&eocdr_section)?;

    let cdr_end = cdr_info
        .cdr_offset
        .checked_add(cdr_info.cdr_size)
        .ok_or_else(|| Error::MalformedSubArchive("subarchive CDR range overflow".to_string()))?;
    if cdr_end != eocdr_start {
        return Err(Error::MalformedSubArchive(format!(
            "subarchive CDR ends at {cdr_end}, expected EOCDR start {eocdr_start}"
        )));
    }

    let cdr_size = usize::try_from(cdr_info.cdr_size).map_err(|_| {
        Error::MalformedSubArchive("subarchive CDR size overflows usize".to_string())
    })?;
    reader.seek(SeekFrom::Start(cdr_info.cdr_offset))?;
    let mut cdr = vec![0u8; cdr_size];
    reader.read_exact(&mut cdr)?;

    let entries = parse_subarchive_cdr_entries(&cdr, cdr_info.version as u64)?;
    if entries.len() as u64 != cdr_info.number_of_entries {
        return Err(Error::MalformedSubArchive(format!(
            "subarchive CDR entry count {}, expected {}",
            entries.len(),
            cdr_info.number_of_entries
        )));
    }

    Ok(SubArchive { cdr_info, entries })
}

/// Parse a P4K subarchive file without reading payload bytes.
pub fn parse_subarchive_file<P: AsRef<Path>>(path: P) -> Result<SubArchive> {
    let mut file = File::open(path)?;
    parse_subarchive_from_reader(&mut file)
}

/// Equivalent to `EnumerateSubArchiveCDREntries`, returned as a vector.
pub fn parse_subarchive_cdr_entries(cdr: &[u8], version: u64) -> Result<Vec<SubArchiveCdrEntry>> {
    let mut entries = Vec::new();
    let mut offset = 0usize;
    while offset < cdr.len() {
        let name_len =
            read_u16_required(cdr, offset + SUBARCHIVE_CDR_FIXED_NAME_LEN_OFFSET)? as usize;
        let record_len = name_len
            .checked_add(SUBARCHIVE_CDR_RECORD_OVERHEAD)
            .ok_or_else(|| {
                Error::MalformedSubArchive("subarchive CDR record overflow".to_string())
            })?;
        let record_end = offset.checked_add(record_len).ok_or_else(|| {
            Error::MalformedSubArchive("subarchive CDR record range overflow".to_string())
        })?;
        let record = cdr.get(offset..record_end).ok_or_else(|| {
            Error::MalformedSubArchive(format!(
                "subarchive CDR record at {offset:#x} extends past CDR size {}",
                cdr.len()
            ))
        })?;

        let name_start = 0x2e;
        let name_end = name_start + name_len;
        let name = std::str::from_utf8(record.get(name_start..name_end).ok_or_else(|| {
            Error::MalformedSubArchive("subarchive CDR name extends past record".to_string())
        })?)
        .map_err(|err| {
            Error::MalformedSubArchive(format!("subarchive CDR name is not UTF-8: {err}"))
        })?
        .to_string();

        let data_base = name_len;
        let (crc32, signature, sha256) = match version {
            1 => (
                read_u32_required(record, SUBARCHIVE_V1_CRC_OFFSET)?,
                read_array_required::<SUBARCHIVE_SIGNATURE_SIZE>(
                    record,
                    data_base + SUBARCHIVE_V1_SIGNATURE_BASE,
                )?,
                read_array_required::<SUBARCHIVE_SHA256_SIZE>(
                    record,
                    data_base + SUBARCHIVE_V1_SHA256_BASE,
                )?,
            ),
            2 => (
                read_u32_required(record, data_base + SUBARCHIVE_V2_CRC_BASE)?,
                read_array_required::<SUBARCHIVE_SIGNATURE_SIZE>(
                    record,
                    data_base + SUBARCHIVE_V2_SIGNATURE_BASE,
                )?,
                read_array_required::<SUBARCHIVE_SHA256_SIZE>(
                    record,
                    data_base + SUBARCHIVE_V2_SHA256_BASE,
                )?,
            ),
            other => {
                return Err(Error::MalformedSubArchive(format!(
                    "unsupported P4K subarchive version {other}"
                )))
            }
        };
        let payload_relative =
            read_u64_required(record, data_base + SUBARCHIVE_CDR_PAYLOAD_RELATIVE_BASE)?;
        let offset_to_file_data = (data_base as u64)
            .checked_add(payload_relative)
            .and_then(|value| value.checked_add(0x3e))
            .ok_or_else(|| {
                Error::MalformedSubArchive("subarchive payload offset overflow".to_string())
            })?;

        entries.push(SubArchiveCdrEntry {
            name,
            compression_method: CompressionMethod::ZstdDeprecated,
            crc32,
            compressed_size: read_u64_required(
                record,
                data_base + SUBARCHIVE_CDR_COMPRESSED_SIZE_BASE,
            )?,
            uncompressed_size: read_u64_required(
                record,
                data_base + SUBARCHIVE_CDR_UNCOMPRESSED_SIZE_BASE,
            )?,
            offset_to_file_data,
            last_mod_file_time: read_u16_required(record, 0x0c)?,
            last_mod_file_date: read_u16_required(record, 0x0e)?,
            signature,
            sha256,
        });
        offset = record_end;
    }
    Ok(entries)
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes
        .get(offset..offset + 2)
        .map(|slice| u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u16_required(bytes: &[u8], offset: usize) -> Result<u16> {
    let slice = bytes.get(offset..offset + 2).ok_or_else(|| {
        Error::MalformedSubArchive(format!("subarchive EOCDR missing u16 at {offset:#x}"))
    })?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_required(bytes: &[u8], offset: usize) -> Result<u32> {
    let slice = bytes.get(offset..offset + 4).ok_or_else(|| {
        Error::MalformedSubArchive(format!("subarchive EOCDR missing u32 at {offset:#x}"))
    })?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64_required(bytes: &[u8], offset: usize) -> Result<u64> {
    let slice = bytes.get(offset..offset + 8).ok_or_else(|| {
        Error::MalformedSubArchive(format!("subarchive EOCDR missing u64 at {offset:#x}"))
    })?;
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

fn read_array_required<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    let slice = bytes.get(offset..offset + N).ok_or_else(|| {
        Error::MalformedSubArchive(format!(
            "subarchive byte array at {offset:#x} with length {N} extends past input"
        ))
    })?;
    let mut out = [0u8; N];
    out.copy_from_slice(slice);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn sample_section() -> [u8; SUBARCHIVE_EOCDR_SIZE] {
        let mut section = [0u8; SUBARCHIVE_EOCDR_SIZE];
        section[SUBARCHIVE_MARKER_OFFSET..SUBARCHIVE_MARKER_OFFSET + 2]
            .copy_from_slice(&SUBARCHIVE_MARKER.to_le_bytes());
        section[SUBARCHIVE_VERSION_OFFSET..SUBARCHIVE_VERSION_OFFSET + 4]
            .copy_from_slice(&2u32.to_le_bytes());
        section[SUBARCHIVE_ENTRY_COUNT_OFFSET..SUBARCHIVE_ENTRY_COUNT_OFFSET + 8]
            .copy_from_slice(&3u64.to_le_bytes());
        section[SUBARCHIVE_CDR_SIZE_OFFSET..SUBARCHIVE_CDR_SIZE_OFFSET + 8]
            .copy_from_slice(&0x456u64.to_le_bytes());
        section[SUBARCHIVE_CDR_OFFSET_OFFSET..SUBARCHIVE_CDR_OFFSET_OFFSET + 8]
            .copy_from_slice(&0x123u64.to_le_bytes());
        section[SUBARCHIVE_NAME_BYTES_OFFSET..SUBARCHIVE_NAME_BYTES_OFFSET + 8]
            .copy_from_slice(&0x78u64.to_le_bytes());
        section
    }

    #[test]
    fn subarchive_eocdr_offsets_match_dump() {
        assert_eq!(subarchive_eocdr_size(), 0x1062);
        assert_eq!(subarchive_eocdr_begin_offset(0x2000), Some(0xf9e));
        assert_eq!(subarchive_eocdr_begin_offset(0x100), None);
    }

    #[test]
    fn subarchive_eocdr_status_matches_dump_marker_and_flag() {
        let mut section = sample_section();
        assert_eq!(
            validate_subarchive_eocdr(&section),
            SubArchiveStatus::Successful
        );

        section[SUBARCHIVE_FLAGS_OFFSET] = 1;
        assert_eq!(
            validate_subarchive_eocdr(&section),
            SubArchiveStatus::ErrorIncompleteSubArchive
        );

        section[SUBARCHIVE_MARKER_OFFSET..SUBARCHIVE_MARKER_OFFSET + 2]
            .copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            validate_subarchive_eocdr(&section),
            SubArchiveStatus::ErrorNotASubArchive
        );
    }

    #[test]
    fn subarchive_cdr_info_offsets_match_dump() {
        let info = parse_subarchive_cdr_info(&sample_section()).unwrap();
        assert_eq!(info.number_of_entries, 3);
        assert_eq!(info.cdr_offset, 0x123);
        assert_eq!(info.cdr_size, 0x456);
        assert_eq!(
            info.cdr_size_including_eocdr,
            0x456 + SUBARCHIVE_EOCDR_SIZE as u64
        );
        assert_eq!(info.total_name_bytes_including_nul, 0x78);
        assert_eq!(info.version, 2);
    }

    #[test]
    fn subarchive_v1_cdr_entry_offsets_match_dump() {
        let mut cdr = vec![0u8; b"file.bin".len() + SUBARCHIVE_CDR_RECORD_OVERHEAD];
        fill_common_cdr_entry(&mut cdr, b"file.bin");
        cdr[SUBARCHIVE_V1_CRC_OFFSET..SUBARCHIVE_V1_CRC_OFFSET + 4]
            .copy_from_slice(&0x1122_3344u32.to_le_bytes());
        cdr[b"file.bin".len() + SUBARCHIVE_V1_SIGNATURE_BASE] = 0xaa;
        cdr[b"file.bin".len() + SUBARCHIVE_V1_SHA256_BASE] = 0xbb;

        let entries = parse_subarchive_cdr_entries(&cdr, 1).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "file.bin");
        assert_eq!(
            entries[0].compression_method,
            CompressionMethod::ZstdDeprecated
        );
        assert_eq!(entries[0].crc32, 0x1122_3344);
        assert_eq!(entries[0].compressed_size, 0x56);
        assert_eq!(entries[0].uncompressed_size, 0x78);
        assert_eq!(
            entries[0].offset_to_file_data,
            0x9a + b"file.bin".len() as u64 + 0x3e
        );
        assert_eq!(entries[0].last_mod_file_time, 0x1234);
        assert_eq!(entries[0].last_mod_file_date, 0x5678);
        assert_eq!(entries[0].signature[0], 0xaa);
        assert_eq!(entries[0].sha256[0], 0xbb);
    }

    #[test]
    fn subarchive_v2_cdr_entry_offsets_match_dump() {
        let mut cdr = vec![0u8; b"file.bin".len() + SUBARCHIVE_CDR_RECORD_OVERHEAD];
        fill_common_cdr_entry(&mut cdr, b"file.bin");
        let base = b"file.bin".len();
        cdr[base + SUBARCHIVE_V2_CRC_BASE..base + SUBARCHIVE_V2_CRC_BASE + 4]
            .copy_from_slice(&0x5566_7788u32.to_le_bytes());
        cdr[base + SUBARCHIVE_V2_SHA256_BASE] = 0xcc;
        cdr[base + SUBARCHIVE_V2_SIGNATURE_BASE] = 0xdd;

        let entries = parse_subarchive_cdr_entries(&cdr, 2).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].crc32, 0x5566_7788);
        assert_eq!(entries[0].sha256[0], 0xcc);
        assert_eq!(entries[0].signature[0], 0xdd);
    }

    #[test]
    fn subarchive_payload_offset_formula_does_not_add_cdr_record_offset() {
        let mut first = vec![0u8; b"one.bin".len() + SUBARCHIVE_CDR_RECORD_OVERHEAD];
        fill_common_cdr_entry(&mut first, b"one.bin");
        first[SUBARCHIVE_V1_CRC_OFFSET..SUBARCHIVE_V1_CRC_OFFSET + 4]
            .copy_from_slice(&1u32.to_le_bytes());

        let mut second = vec![0u8; b"two.bin".len() + SUBARCHIVE_CDR_RECORD_OVERHEAD];
        fill_common_cdr_entry(&mut second, b"two.bin");
        second[SUBARCHIVE_V1_CRC_OFFSET..SUBARCHIVE_V1_CRC_OFFSET + 4]
            .copy_from_slice(&2u32.to_le_bytes());

        let second_expected = 0x9a + b"two.bin".len() as u64 + 0x3e;
        let cdr = [first, second].concat();
        let entries = parse_subarchive_cdr_entries(&cdr, 1).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].name, "two.bin");
        assert_eq!(entries[1].offset_to_file_data, second_expected);
    }

    #[test]
    fn parse_complete_subarchive_validates_cdr_range_and_entry_count() {
        let mut cdr = vec![0u8; b"file.bin".len() + SUBARCHIVE_CDR_RECORD_OVERHEAD];
        fill_common_cdr_entry(&mut cdr, b"file.bin");
        cdr[SUBARCHIVE_V1_CRC_OFFSET..SUBARCHIVE_V1_CRC_OFFSET + 4]
            .copy_from_slice(&0x1122_3344u32.to_le_bytes());

        let cdr_offset = 0x20usize;
        let mut file = vec![0u8; cdr_offset];
        file.extend_from_slice(&cdr);
        let cdr_end = file.len();
        let mut eocdr = sample_section();
        eocdr[SUBARCHIVE_VERSION_OFFSET..SUBARCHIVE_VERSION_OFFSET + 4]
            .copy_from_slice(&1u32.to_le_bytes());
        eocdr[SUBARCHIVE_ENTRY_COUNT_OFFSET..SUBARCHIVE_ENTRY_COUNT_OFFSET + 8]
            .copy_from_slice(&1u64.to_le_bytes());
        eocdr[SUBARCHIVE_CDR_SIZE_OFFSET..SUBARCHIVE_CDR_SIZE_OFFSET + 8]
            .copy_from_slice(&(cdr.len() as u64).to_le_bytes());
        eocdr[SUBARCHIVE_CDR_OFFSET_OFFSET..SUBARCHIVE_CDR_OFFSET_OFFSET + 8]
            .copy_from_slice(&(cdr_offset as u64).to_le_bytes());
        file.extend_from_slice(&eocdr);
        assert_eq!(cdr_end, file.len() - SUBARCHIVE_EOCDR_SIZE);

        let subarchive = parse_subarchive(&file).unwrap();
        assert_eq!(subarchive.cdr_info.number_of_entries, 1);
        assert_eq!(subarchive.cdr_info.cdr_offset, cdr_offset as u64);
        assert_eq!(subarchive.entries[0].name, "file.bin");

        let mut reader = Cursor::new(file.clone());
        let streamed = parse_subarchive_from_reader(&mut reader).unwrap();
        assert_eq!(streamed, subarchive);

        let mut bad_count = file.clone();
        let eocdr_start = bad_count.len() - SUBARCHIVE_EOCDR_SIZE;
        bad_count[eocdr_start + SUBARCHIVE_ENTRY_COUNT_OFFSET
            ..eocdr_start + SUBARCHIVE_ENTRY_COUNT_OFFSET + 8]
            .copy_from_slice(&2u64.to_le_bytes());
        let err = parse_subarchive(&bad_count).unwrap_err();
        assert!(
            err.to_string().contains("entry count"),
            "expected subarchive entry-count rejection, got {err}"
        );

        let mut bad_range = file;
        let eocdr_start = bad_range.len() - SUBARCHIVE_EOCDR_SIZE;
        bad_range[eocdr_start + SUBARCHIVE_CDR_SIZE_OFFSET
            ..eocdr_start + SUBARCHIVE_CDR_SIZE_OFFSET + 8]
            .copy_from_slice(&((cdr.len() - 1) as u64).to_le_bytes());
        let err = parse_subarchive(&bad_range).unwrap_err();
        assert!(
            err.to_string().contains("CDR ends"),
            "expected subarchive CDR range rejection, got {err}"
        );
    }

    fn fill_common_cdr_entry(cdr: &mut [u8], name: &[u8]) {
        cdr[0x0c..0x0e].copy_from_slice(&0x1234u16.to_le_bytes());
        cdr[0x0e..0x10].copy_from_slice(&0x5678u16.to_le_bytes());
        cdr[SUBARCHIVE_CDR_FIXED_NAME_LEN_OFFSET..SUBARCHIVE_CDR_FIXED_NAME_LEN_OFFSET + 2]
            .copy_from_slice(&(name.len() as u16).to_le_bytes());
        cdr[0x2e..0x2e + name.len()].copy_from_slice(name);
        let base = name.len();
        cdr[base + SUBARCHIVE_CDR_UNCOMPRESSED_SIZE_BASE
            ..base + SUBARCHIVE_CDR_UNCOMPRESSED_SIZE_BASE + 8]
            .copy_from_slice(&0x78u64.to_le_bytes());
        cdr[base + SUBARCHIVE_CDR_COMPRESSED_SIZE_BASE
            ..base + SUBARCHIVE_CDR_COMPRESSED_SIZE_BASE + 8]
            .copy_from_slice(&0x56u64.to_le_bytes());
        cdr[base + SUBARCHIVE_CDR_PAYLOAD_RELATIVE_BASE
            ..base + SUBARCHIVE_CDR_PAYLOAD_RELATIVE_BASE + 8]
            .copy_from_slice(&0x9au64.to_le_bytes());
    }

    /// Opt-in real-data test. Set `SVAROG_P4K_SUBARCHIVE_TEST_FILE` to a
    /// `.p4k_subarchive.zst` sample; this uses the seeked parser so even large
    /// subarchives avoid loading payload bytes.
    #[test]
    fn real_world_subarchive_file_parses() {
        let Ok(path) = std::env::var("SVAROG_P4K_SUBARCHIVE_TEST_FILE") else {
            eprintln!("skipping: SVAROG_P4K_SUBARCHIVE_TEST_FILE is unset");
            return;
        };
        if !Path::new(&path).is_file() {
            eprintln!("skipping: SVAROG_P4K_SUBARCHIVE_TEST_FILE points at missing path");
            return;
        }

        let subarchive = parse_subarchive_file(&path).unwrap();
        eprintln!(
            "opened {path}: version={} entries={} cdr_size={}",
            subarchive.cdr_info.version,
            subarchive.entries.len(),
            subarchive.cdr_info.cdr_size
        );
        assert_eq!(
            subarchive.entries.len() as u64,
            subarchive.cdr_info.number_of_entries
        );
    }
}
