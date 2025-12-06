//! End of Central Directory (EOCD) structures.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// End of Central Directory Record (without signature).
///
/// This is the standard ZIP EOCD record found at the end of the archive.
/// The 4-byte signature (0x06054b50) is read separately before this struct.
/// For ZIP64 archives, some fields will contain 0xFFFF or 0xFFFFFFFF
/// to indicate that the actual values are in the ZIP64 EOCD record.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct EocdRecord {
    /// Number of this disk
    pub disk_number: u16,
    /// Disk where central directory starts
    pub central_dir_disk: u16,
    /// Number of central directory records on this disk
    pub central_dir_count_disk: u16,
    /// Total number of central directory records
    pub central_dir_count_total: u16,
    /// Size of central directory (bytes)
    pub central_dir_size: u32,
    /// Offset of start of central directory
    pub central_dir_offset: u32,
    /// Comment length
    pub comment_length: u16,
}

impl EocdRecord {
    /// EOCD signature bytes.
    pub const MAGIC: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];

    /// EOCD signature as u32.
    pub const SIGNATURE: u32 = 0x06054b50;

    /// Check if this archive uses ZIP64 extensions.
    ///
    /// Returns true if any of the fields contain sentinel values
    /// indicating ZIP64 format.
    pub fn is_zip64(&self) -> bool {
        self.central_dir_count_total == 0xFFFF
            || self.central_dir_offset == 0xFFFFFFFF
            || self.central_dir_size == 0xFFFFFFFF
    }
}

/// ZIP64 End of Central Directory Locator (without signature).
///
/// This record points to the ZIP64 EOCD record.
/// The 4-byte signature (0x07064b50) is read separately before this struct.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct Eocd64Locator {
    /// Disk number containing ZIP64 EOCD
    pub zip64_eocd_disk: u32,
    /// Offset of ZIP64 EOCD record
    pub zip64_eocd_offset: u64,
    /// Total number of disks
    pub total_disks: u32,
}

impl Eocd64Locator {
    /// ZIP64 EOCD Locator signature bytes.
    pub const MAGIC: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];

    /// ZIP64 EOCD Locator signature as u32.
    pub const SIGNATURE: u32 = 0x07064b50;
}

/// ZIP64 End of Central Directory Record (without signature).
///
/// The 4-byte signature (0x06064b50) is read separately before this struct.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct Eocd64Record {
    /// Size of this record (not including signature or this field)
    pub record_size: u64,
    /// Version made by
    pub version_made_by: u16,
    /// Version needed to extract
    pub version_needed: u16,
    /// This disk number
    pub disk_number: u32,
    /// Disk where central directory starts
    pub central_dir_disk: u32,
    /// Number of central directory records on this disk
    pub central_dir_count_disk: u64,
    /// Total number of central directory records
    pub central_dir_count_total: u64,
    /// Size of central directory (bytes)
    pub central_dir_size: u64,
    /// Offset of start of central directory
    pub central_dir_offset: u64,
}

impl Eocd64Record {
    /// ZIP64 EOCD signature bytes.
    pub const MAGIC: [u8; 4] = [0x50, 0x4b, 0x06, 0x06];

    /// ZIP64 EOCD signature as u32.
    pub const SIGNATURE: u32 = 0x06064b50;
}
