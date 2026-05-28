//! Central Directory entry for P4K v2 ("JiJi") format.
//!
//! Unlike v1, v2 archives use a fixed-size per-entry record stored
//! contiguously starting at `Eocd2Record::central_directory_record_offset`.
//! Each entry is exactly [`CDR_V2_ENTRY_SIZE`] bytes; names live in a
//! separate name table that begins at
//! `Eocd2Record::central_directory_record_text_offset`.
//!
//! In v2 there are NO local file headers preceding the compressed
//! payload — `offset_to_file_data` points directly at the compressed
//! bytes. See `CCigPakFileEntry::GetLocalFileRecordSize`, which
//! returns 0 when `m_nVersion != 1`.
//!
//! Field offsets are derived from
//! `CCigPakFile::UpdateAndWriteCDR_v2` (writer),
//! `CCigPakFileEntry::CCigPakFileEntry(rPakFile, rCDRE, ...)`
//! (in-memory constructor), and `CCigPakFile::LoadPakFile_v2`
//! (reader) in the official CigDataPatcher decompilation.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Fixed on-disk size of a single v2 CDR entry, in bytes.
pub const CDR_V2_ENTRY_SIZE: usize = 0xCC;
/// Byte offset of `offset_to_filename` within a v2 CDR entry.
pub const CDR_V2_OFFSET_TO_FILENAME_OFFSET: usize = 0x22;

/// On-disk layout of a P4K v2 Central Directory Record entry.
///
/// The struct is `#[repr(C, packed)]` because the `u64` numeric
/// fields land at unaligned offsets (e.g. `compressed_size` at byte
/// 0x0A). The unusual offsets come from the writer's hand-rolled
/// 16-byte SIMD copies of the same packed in-memory struct used by
/// the v1 ZIP central-directory writer.
///
/// Layout (every byte accounted for):
///
/// ```text
/// 0x00  u16      compression_method
/// 0x02  u16      last_mod_file_time     -- DOS time
/// 0x04  u16      last_mod_file_date     -- DOS date
/// 0x06  u32      crc32
/// 0x0A  u64      compressed_size        -- packed (unaligned)
/// 0x12  u64      uncompressed_size      -- packed (unaligned)
/// 0x1A  u64      offset_to_file_data    -- absolute payload offset
/// 0x22  u64      offset_to_filename     -- relative to name table start
/// 0x2A  u8[128]  signature              -- RSA-1024 signature (zero if unsigned)
/// 0xAA  u16      encryption_flag        -- 0 = plain, 1 = encrypted
/// 0xAC  u8[32]   sha256                 -- SHA-256 of compressed payload
/// 0xCC           end
/// ```
#[derive(Debug, Clone, Copy, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C, packed)]
pub struct CentralDirectoryHeaderV2 {
    /// Compression method (same enum as v1: 0 = Store, 8 = raw deflate,
    /// 93/100 = Zstd, 101 = zlib-wrapped deflate).
    pub compression_method: u16,
    /// DOS modification time.
    pub last_mod_file_time: u16,
    /// DOS modification date.
    pub last_mod_file_date: u16,
    /// CIG CRC32C of the uncompressed payload.
    pub crc32: u32,
    /// Compressed size in bytes (stored unaligned).
    pub compressed_size: u64,
    /// Uncompressed size in bytes (stored unaligned).
    pub uncompressed_size: u64,
    /// Absolute offset of compressed payload bytes within the file
    /// (no local file header precedes the data in v2).
    pub offset_to_file_data: u64,
    /// Byte offset of this entry's null-terminated name within the
    /// archive's name table.
    pub offset_to_filename: u64,
    /// RSA-1024 signature of the entry. All zeros if the archive is
    /// unsigned.
    pub signature: [u8; 128],
    /// Per-entry encryption flag. `1` = AES-encrypted payload.
    pub encryption_flag: u16,
    /// SHA-256 of the (compressed, post-encryption) payload bytes.
    pub sha256: [u8; 32],
}

impl CentralDirectoryHeaderV2 {
    /// Whether the entry's payload is AES-encrypted.
    #[inline]
    pub fn is_encrypted(&self) -> bool {
        let flag = self.encryption_flag;
        flag == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_layout_matches_decompilation() {
        // These offsets are pinned by the official CigDataPatcher
        // writer (`UpdateAndWriteCDR_v2`) and the in-memory copy in
        // `CCigPakFileEntry::CCigPakFileEntry(..., rCDRE, ...)`.
        assert_eq!(
            std::mem::size_of::<CentralDirectoryHeaderV2>(),
            CDR_V2_ENTRY_SIZE
        );
        assert_eq!(
            std::mem::offset_of!(CentralDirectoryHeaderV2, compression_method),
            0x00
        );
        assert_eq!(
            std::mem::offset_of!(CentralDirectoryHeaderV2, last_mod_file_time),
            0x02
        );
        assert_eq!(
            std::mem::offset_of!(CentralDirectoryHeaderV2, last_mod_file_date),
            0x04
        );
        assert_eq!(std::mem::offset_of!(CentralDirectoryHeaderV2, crc32), 0x06);
        assert_eq!(
            std::mem::offset_of!(CentralDirectoryHeaderV2, compressed_size),
            0x0A
        );
        assert_eq!(
            std::mem::offset_of!(CentralDirectoryHeaderV2, uncompressed_size),
            0x12
        );
        assert_eq!(
            std::mem::offset_of!(CentralDirectoryHeaderV2, offset_to_file_data),
            0x1A
        );
        assert_eq!(
            std::mem::offset_of!(CentralDirectoryHeaderV2, offset_to_filename),
            CDR_V2_OFFSET_TO_FILENAME_OFFSET
        );
        assert_eq!(
            std::mem::offset_of!(CentralDirectoryHeaderV2, signature),
            0x2A
        );
        assert_eq!(
            std::mem::offset_of!(CentralDirectoryHeaderV2, encryption_flag),
            0xAA
        );
        assert_eq!(std::mem::offset_of!(CentralDirectoryHeaderV2, sha256), 0xAC);
    }

    #[test]
    fn encryption_flag_decoding() {
        let bytes = [0u8; CDR_V2_ENTRY_SIZE];
        let mut entry: CentralDirectoryHeaderV2 =
            zerocopy::FromBytes::read_from_bytes(&bytes[..]).unwrap();
        assert!(!entry.is_encrypted());
        entry.encryption_flag = 0;
        assert!(!entry.is_encrypted());
        entry.encryption_flag = 1;
        assert!(entry.is_encrypted());
        entry.encryption_flag = 3;
        assert!(!entry.is_encrypted());
        entry.encryption_flag = 2;
        assert!(!entry.is_encrypted());
    }
}
