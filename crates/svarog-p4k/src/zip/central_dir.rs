//! Central Directory Header structures.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Central Directory File Header (without signature).
///
/// This structure describes a single file entry in the archive's
/// central directory. The 4-byte signature (0x02014b50) is read
/// separately before this struct.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct CentralDirectoryHeader {
    /// Version made by
    pub version_made_by: u16,
    /// Version needed to extract
    pub version_needed: u16,
    /// General purpose bit flag
    pub flags: u16,
    /// Compression method
    pub compression_method: u16,
    /// File last modification time and date (DOS format)
    pub last_modified: u32,
    /// CRC-32 of uncompressed data
    pub crc32: u32,
    /// Compressed size
    pub compressed_size: u32,
    /// Uncompressed size
    pub uncompressed_size: u32,
    /// File name length
    pub file_name_length: u16,
    /// Extra field length
    pub extra_field_length: u16,
    /// File comment length
    pub file_comment_length: u16,
    /// Disk number where file starts
    pub disk_number_start: u16,
    /// Internal file attributes
    pub internal_attrs: u16,
    /// External file attributes
    pub external_attrs: u32,
    /// Relative offset of local file header
    pub local_header_offset: u32,
}

impl CentralDirectoryHeader {
    /// Central Directory signature bytes.
    pub const MAGIC: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];

    /// Central Directory signature as u32.
    pub const SIGNATURE: u32 = 0x02014b50;

    /// Total variable-length data size following this header.
    pub fn variable_data_size(&self) -> usize {
        self.file_name_length as usize
            + self.extra_field_length as usize
            + self.file_comment_length as usize
    }
}

/// P4K-specific extra field IDs.
pub mod extra_field {
    /// ZIP64 extended information extra field.
    pub const ZIP64: u16 = 0x0001;
    /// P4K custom field (purpose unknown).
    pub const P4K_5000: u16 = 0x5000;
    /// P4K encryption flag field.
    pub const P4K_5002: u16 = 0x5002;
    /// P4K custom field (purpose unknown).
    pub const P4K_5003: u16 = 0x5003;
}
