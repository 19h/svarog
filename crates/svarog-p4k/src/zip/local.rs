//! Local File Header structures.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Local File Header.
///
/// This structure precedes the actual file data in the archive.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct LocalFileHeader {
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
}

impl LocalFileHeader {
    /// Local File Header signature bytes.
    pub const MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

    /// Local File Header signature as u32.
    pub const SIGNATURE: u32 = 0x04034b50;

    /// Extended Local File Header signature (used in P4K).
    pub const SIGNATURE_EXTENDED: u32 = 0x14034b50;

    /// Total variable-length data size following this header.
    pub fn variable_data_size(&self) -> usize {
        self.file_name_length as usize + self.extra_field_length as usize
    }
}
