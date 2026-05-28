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
    /// CIG CRC32C of uncompressed data
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
    /// P4K ZIP64 total extra field size, including 4-byte field header.
    ///
    /// The official verifier checks `*(u16 *)(field + 2) == 0x20`
    /// and then reads three `u64` values plus a trailing `u32`.
    pub const ZIP64_TOTAL_SIZE: u16 = 0x20;
    /// P4K custom field (purpose unknown).
    pub const P4K_5000: u16 = 0x5000;
    /// Total byte size of field 0x5000, including field header.
    pub const P4K_5000_TOTAL_SIZE: u16 = 0x84;
    /// P4K encryption flag field.
    pub const P4K_5002: u16 = 0x5002;
    /// Total byte size of field 0x5002, including field header.
    pub const P4K_5002_TOTAL_SIZE: u16 = 0x06;
    /// P4K custom field (purpose unknown).
    pub const P4K_5003: u16 = 0x5003;
    /// Total byte size of field 0x5003, including field header.
    pub const P4K_5003_TOTAL_SIZE: u16 = 0x24;
}

/// P4K v1 ZIP64 extra field.
///
/// This intentionally uses P4K's observed field-size convention:
/// the `size` field is the total field size including `id` and
/// `size`, not the ZIP APPNOTE payload length convention. The
/// decompiled verifier checks:
///
/// ```text
/// *(u16 *)(field + 0) == 0x0001
/// *(u16 *)(field + 2) == 0x20
/// *(u32 *)(field + 0x1C) == 0
/// ```
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct P4kZip64ExtraField {
    pub id: u16,
    pub size: u16,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    pub local_header_offset: u64,
    pub disk_number_start: u32,
}

/// P4K v1 field `0x5000`: RSA-1024 signature.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct P4kSignatureExtraField {
    pub id: u16,
    pub size: u16,
    pub signature: [u8; 128],
}

/// P4K v1 field `0x5002`: encryption flag.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct P4kEncryptionExtraField {
    pub id: u16,
    pub size: u16,
    pub encryption: u16,
}

/// P4K v1 field `0x5003`: SHA-256 of compressed payload bytes.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct P4kSha256ExtraField {
    pub id: u16,
    pub size: u16,
    pub sha256: [u8; 32],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p4k_v1_extra_field_layouts_match_decompilation() {
        assert_eq!(
            std::mem::size_of::<P4kZip64ExtraField>(),
            extra_field::ZIP64_TOTAL_SIZE as usize
        );
        assert_eq!(
            std::mem::size_of::<P4kSignatureExtraField>(),
            extra_field::P4K_5000_TOTAL_SIZE as usize
        );
        assert_eq!(
            std::mem::size_of::<P4kEncryptionExtraField>(),
            extra_field::P4K_5002_TOTAL_SIZE as usize
        );
        assert_eq!(
            std::mem::size_of::<P4kSha256ExtraField>(),
            extra_field::P4K_5003_TOTAL_SIZE as usize
        );

        assert_eq!(std::mem::offset_of!(P4kZip64ExtraField, id), 0x00);
        assert_eq!(std::mem::offset_of!(P4kZip64ExtraField, size), 0x02);
        assert_eq!(
            std::mem::offset_of!(P4kZip64ExtraField, uncompressed_size),
            0x04
        );
        assert_eq!(
            std::mem::offset_of!(P4kZip64ExtraField, compressed_size),
            0x0C
        );
        assert_eq!(
            std::mem::offset_of!(P4kZip64ExtraField, local_header_offset),
            0x14
        );
        assert_eq!(
            std::mem::offset_of!(P4kZip64ExtraField, disk_number_start),
            0x1C
        );

        assert_eq!(
            std::mem::offset_of!(P4kSignatureExtraField, signature),
            0x04
        );
        assert_eq!(
            std::mem::offset_of!(P4kEncryptionExtraField, encryption),
            0x04
        );
        assert_eq!(std::mem::offset_of!(P4kSha256ExtraField, sha256), 0x04);
    }
}
