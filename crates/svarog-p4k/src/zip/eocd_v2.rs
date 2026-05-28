//! End of Central Directory Record for P4K v2 ("JiJi") format.
//!
//! The v2 format replaces the traditional ZIP64 EOCD/EOCD64 chain
//! with a single fixed-size 175-byte trailer record. The record lives
//! at the very end of the file (after any sector-alignment padding
//! consumed by the EoFBuffer write).
//!
//! Detection: the last 6 bytes of the record contain
//! `version` (`u16` = 2) followed by `magic` (`u32` = `0x696A694A`,
//! ASCII "JiJi" little-endian).
//!
//! The structure mirrors the writer in
//! `CCigPakFile::UpdateAndWriteCDR_v2` and the reader in
//! `CCigPakFile::LoadPakFile_v2` from the official CigDataPatcher tool.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Total fixed size of the v2 EOCDR record in bytes.
pub const EOCD_V2_SIZE: usize = 0xAF;

/// "JiJi" magic stored as little-endian `u32` (`0x4A 0x69 0x4A 0x69`).
pub const EOCD_V2_MAGIC: u32 = 0x696A694A;

/// Version value embedded in the trailer for P4K v2 files.
pub const EOCD_V2_VERSION: u16 = 2;

/// End of Central Directory Record (P4K v2 / "JiJi" format).
///
/// All multi-byte fields are little-endian and densely packed (no
/// padding). The record is always exactly [`EOCD_V2_SIZE`] bytes.
///
/// The five `reserved_*` slots are always written as zero by the
/// official writer.
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct Eocd2Record {
    /// Total number of CDR entries in the archive.
    pub num_file_entries: u64,
    /// Reserved (writer leaves this zero).
    pub reserved_08: u64,
    /// Absolute byte offset of the first CDR entry (`m_nEndOfFileBlockOffset`).
    pub end_of_file_block_offset: u64,
    /// Total size of the CDR entry array in bytes (`0xCC * num_file_entries`).
    pub cdr_size: u64,
    /// Reserved.
    pub reserved_20: u64,
    /// Absolute offset of the name table
    /// (== `end_of_file_block_offset + cdr_size`).
    pub name_table_abs_offset: u64,
    /// Total byte length of all entry names, including null terminators.
    pub total_name_length: u64,
    /// Reserved.
    pub reserved_38: u64,
    /// Absolute byte offset just past the last byte of payload data.
    pub end_of_payload: u64,
    /// Number of `cig_zip64::freelist_block` records (16 bytes each)
    /// stored in the install block.
    pub num_freelist_blocks: u64,
    /// Reserved.
    pub reserved_50: u64,
    /// Reserved.
    pub reserved_58: u64,
    /// Sector size used when the archive was written.
    pub physical_sector_size: u64,
    /// Always `1` in files written by the official tool.
    pub flag_68: u8,
    /// Two SHA-256 digests stored back-to-back (64 bytes total).
    /// The first 32 bytes are the manifest hash.
    pub manifest_sha256: [u8; 64],
    /// Version field (must equal [`EOCD_V2_VERSION`]).
    pub version: u16,
    /// Magic value (must equal [`EOCD_V2_MAGIC`]).
    pub magic: u32,
}

impl Eocd2Record {
    /// Confirm the magic and version match a v2 P4K file.
    #[inline]
    pub fn is_valid(&self) -> bool {
        let magic = self.magic;
        let version = self.version;
        magic == EOCD_V2_MAGIC && version == EOCD_V2_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_layout_matches_decompilation() {
        // The CigDataPatcher writer accesses the record through
        // offsets `v17 + 0xFFFFFFXX` (signed -0xAF + field offset).
        // These const checks pin every field to its decompiled offset.
        assert_eq!(std::mem::size_of::<Eocd2Record>(), EOCD_V2_SIZE);
        assert_eq!(std::mem::offset_of!(Eocd2Record, num_file_entries), 0x00);
        assert_eq!(std::mem::offset_of!(Eocd2Record, reserved_08), 0x08);
        assert_eq!(
            std::mem::offset_of!(Eocd2Record, end_of_file_block_offset),
            0x10
        );
        assert_eq!(std::mem::offset_of!(Eocd2Record, cdr_size), 0x18);
        assert_eq!(std::mem::offset_of!(Eocd2Record, reserved_20), 0x20);
        assert_eq!(
            std::mem::offset_of!(Eocd2Record, name_table_abs_offset),
            0x28
        );
        assert_eq!(std::mem::offset_of!(Eocd2Record, total_name_length), 0x30);
        assert_eq!(std::mem::offset_of!(Eocd2Record, reserved_38), 0x38);
        assert_eq!(std::mem::offset_of!(Eocd2Record, end_of_payload), 0x40);
        assert_eq!(std::mem::offset_of!(Eocd2Record, num_freelist_blocks), 0x48);
        assert_eq!(std::mem::offset_of!(Eocd2Record, reserved_50), 0x50);
        assert_eq!(std::mem::offset_of!(Eocd2Record, reserved_58), 0x58);
        assert_eq!(
            std::mem::offset_of!(Eocd2Record, physical_sector_size),
            0x60
        );
        assert_eq!(std::mem::offset_of!(Eocd2Record, flag_68), 0x68);
        assert_eq!(std::mem::offset_of!(Eocd2Record, manifest_sha256), 0x69);
        assert_eq!(std::mem::offset_of!(Eocd2Record, version), 0xA9);
        assert_eq!(std::mem::offset_of!(Eocd2Record, magic), 0xAB);
    }

    #[test]
    fn valid_when_magic_and_version_match() {
        let bytes = [0u8; EOCD_V2_SIZE];
        let mut record: Eocd2Record = zerocopy::FromBytes::read_from_bytes(&bytes[..]).unwrap();
        record.magic = EOCD_V2_MAGIC;
        record.version = EOCD_V2_VERSION;
        assert!(record.is_valid());

        record.version = 1;
        assert!(!record.is_valid());

        record.version = EOCD_V2_VERSION;
        record.magic = 0xDEADBEEF;
        assert!(!record.is_valid());
    }
}
